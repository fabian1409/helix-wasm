import WASI from "./vendor/browser_wasi_shim/wasi.js";
import { Fd } from "./vendor/browser_wasi_shim/fd.js";
import { ConsoleStdout, Directory, File, PreopenDirectory } from "./vendor/browser_wasi_shim/fs_mem.js";

// Linked in index.html via Google Fonts; falls back to the platform default monospace font
// if that's blocked (offline, ad blocker, etc.) - see the `document.fonts.load` call in
// main() below.
const FONT_FAMILY = '"Fira Code", monospace';
const FONT_SIZE = 16;
// Canvas has no line-height concept - drawing at 1 row per FONT_SIZE px (i.e. treating the
// font size as the line height too, which is what a plain `ctx.font = "16px …"` gives you)
// packs rows as tightly as the font's own em-box, with none of the extra leading a real
// terminal adds on top of its font's natural metrics. 1.2x is the common terminal-emulator
// default for that extra breathing room.
const LINE_HEIGHT = 1.2;
const CELL_H = Math.round(FONT_SIZE * LINE_HEIGHT);
// Not a fixed cell width like CELL_H - Fira Code's actual glyph advance width at FONT_SIZE
// isn't necessarily an integer, or 8px, so this is measured once in main() before the first
// layout/draw instead of hardcoded, and every cell position derives from it.
let CELL_W = 8;
// Canvas has no line-height concept for a single `fillText` call either -
// `textBaseline: "top"` pins a glyph to the font's ascent metric, not to CELL_H, and a
// font's ascent+descent commonly exceeds its own em size (Fira Code's does) even before
// LINE_HEIGHT is added on top - so glyphs drawn that way clip against the row above. This is
// the alphabetic-baseline y offset (from a cell's top edge) that centers a glyph in CELL_H
// instead, using real font metrics - also measured once in main(), see there.
let TEXT_BASELINE_Y = CELL_H * 0.8;
// A little breathing room above the first row, purely cosmetic (flush-to-the-top-edge text
// looked cramped) - filled with the theme's background the same way the leftover margin at
// the right/bottom edge is, see draw()'s margin fill.
const PADDING_TOP = 8;
let COLS = 0, ROWS = 0;

const NAMED_KEYS = {
  Enter: "ret",
  Backspace: "backspace",
  ArrowLeft: "left",
  ArrowRight: "right",
  ArrowUp: "up",
  ArrowDown: "down",
  Home: "home",
  End: "end",
  PageUp: "pageup",
  PageDown: "pagedown",
  Tab: "tab",
  Delete: "del",
  Insert: "ins",
  Escape: "esc",
  " ": "space",
};

// Maps a browser KeyboardEvent to Helix's own key notation (e.g. "a", "ret", "C-w"),
// which `helix_view::input::KeyEvent`'s `FromStr` impl parses on the Rust side.
function keyToNotation(e) {
  if (["Control", "Alt", "Shift", "Meta"].includes(e.key)) return null;

  let base = NAMED_KEYS[e.key];
  if (base === undefined) {
    if (e.key.length !== 1) return null;
    base = e.key;
  }

  let notation = base;
  // Shift is already reflected in the character case for printable keys (e.g. "A" vs
  // "a"), so only add "S-" for named keys.
  if (e.shiftKey && base.length > 1) notation = "S-" + notation;
  if (e.altKey) notation = "A-" + notation;
  if (e.ctrlKey) notation = "C-" + notation;
  return notation;
}

function rgb(packed) {
  return `rgb(${(packed >> 16) & 0xff},${(packed >> 8) & 0xff},${packed & 0xff})`;
}

// Loads and runs the wasm32-wasip1 module's `_start` (which runs Rust's runtime init and
// our no-op `main`, then exits) against a minimal WASI host, and returns its `exports` -
// still fully callable afterward, since "exiting" only unwinds the `_start` call, not the
// wasm instance itself.
//
// One preopened directory at `/` gives the guest a real (session-only, in-memory)
// filesystem: config.toml/languages.toml overrides (see `helix_loader::config_dir()` etc.
// and `build_application` in helix-wasm/src/main.rs) live at the root right alongside
// whatever files the user drops in (see `handleDrop` below) - there's no separate
// config/cache/data/workspace split. Nothing here persists across a reload. `rootDir` is
// kept around so JS can seed `File` objects directly (bypassing WASI syscalls) for
// drag-and-drop.
async function loadHelixWasm() {
  const rootDir = new Directory(new Map());

  const wasi = new WASI(
    [],
    // `HOME` is only used to build paths (config/cache dirs); nothing here touches a real
    // filesystem, so any value satisfies `std::env::home_dir()` (used by helix-loader's
    // config/cache/data dir lookups) without those paths ever needing to actually exist.
    ["HOME=/home/helix"],
    [
      new Fd(), // stdin: unused, default Fd methods (ERRNO_NOTSUP) are fine.
      ConsoleStdout.lineBuffered((line) => console.log(line)),
      ConsoleStdout.lineBuffered((line) => console.error(line)),
      new PreopenDirectory("/", rootDir.contents),
    ],
    // The shim's debug logging defaults to *on* if this isn't passed at all.
    { debug: false },
  );

  const { instance } = await WebAssembly.instantiateStreaming(fetch("./helix_wasm.wasm"), {
    wasi_snapshot_preview1: wasi.wasiImport,
  });

  wasi.start(instance);
  return { exports: instance.exports, rootDir };
}

// Writes `notation`'s UTF-8 bytes into the scratch buffer `hx_key_buf_ptr` exposes (see
// `KEY_BUF` in helix-wasm/src/main.rs) and calls `hx_key` to consume them. Memory's
// backing ArrayBuffer must be re-read on every call, since growing wasm memory replaces it.
// Also flushes any clipboard write (`"+y` etc.) the key event queued, since there's no
// synchronous browser API for that direction either - see hx_copy_len in main.rs.
function sendKey(exports, notation) {
  const bytes = new TextEncoder().encode(notation);
  if (bytes.length > exports.hx_key_buf_capacity()) return;
  const ptr = exports.hx_key_buf_ptr();
  new Uint8Array(exports.memory.buffer, ptr, bytes.length).set(bytes);
  exports.hx_key(bytes.length);
  flushClipboardCopy(exports);
  flushDownload(exports);
  flushOpenUrl(exports);
}

function flushClipboardCopy(exports) {
  const len = exports.hx_copy_len();
  if (len === 0) return;
  const ptr = exports.hx_copy_ptr();
  const bytes = new Uint8Array(exports.memory.buffer, ptr, len).slice();
  exports.hx_copy_clear();
  navigator.clipboard.writeText(new TextDecoder().decode(bytes)).catch((err) => {
    console.error("helix-wasm: writing to the system clipboard failed:", err);
  });
}

// Saves a file queued by `:download` (see `hx_download_len` in helix-wasm/src/main.rs) by
// building a Blob and clicking a throwaway `<a download>` link - the standard way to trigger
// a browser "Save As" without a real server response.
function flushDownload(exports) {
  const len = exports.hx_download_len();
  if (len === 0) return;
  const bytes = new Uint8Array(exports.memory.buffer, exports.hx_download_ptr(), len).slice();
  const nameLen = exports.hx_download_name_len();
  const nameBytes = new Uint8Array(exports.memory.buffer, exports.hx_download_name_ptr(), nameLen).slice();
  const name = new TextDecoder().decode(nameBytes);
  exports.hx_download_clear();

  const url = URL.createObjectURL(new Blob([bytes]));
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.click();
  URL.revokeObjectURL(url);
}

// Opens a URL queued by `gf` on a link (see `hx_open_url_len` in helix-wasm/src/main.rs) in a
// new tab. `noopener,noreferrer` keeps the opened page from reaching back into this one via
// `window.opener`.
function flushOpenUrl(exports) {
  const len = exports.hx_open_url_len();
  if (len === 0) return;
  const bytes = new Uint8Array(exports.memory.buffer, exports.hx_open_url_ptr(), len).slice();
  exports.hx_open_url_clear();
  window.open(new TextDecoder().decode(bytes), "_blank", "noopener,noreferrer");
}

// There's no synchronous "read the clipboard" browser API, so `"+p` can only ever paste
// whatever text a real paste gesture (Ctrl+V/Cmd+V) most recently delivered here - see the
// comment on helix-view's wasm32 ClipboardProvider for the full reasoning.
function handlePaste(exports, e) {
  const text = e.clipboardData?.getData("text/plain");
  if (!text) return;
  const bytes = new TextEncoder().encode(text);
  const ptr = exports.hx_paste_alloc(bytes.length);
  new Uint8Array(exports.memory.buffer, ptr, bytes.length).set(bytes);
  exports.hx_paste(bytes.length);
  e.preventDefault();
}

// Writes `path`'s UTF-8 bytes into the scratch buffer `hx_open_path_alloc` grows to fit and
// calls `hx_open_path` to open it in the current view. `path` must already exist in the
// virtual filesystem (see `insertFile`) - this is what `handleDrop` calls after seeding a file.
function openPath(exports, path) {
  const bytes = new TextEncoder().encode(path);
  const ptr = exports.hx_open_path_alloc(bytes.length);
  new Uint8Array(exports.memory.buffer, ptr, bytes.length).set(bytes);
  exports.hx_open_path(bytes.length);
}

// Inserts `bytes` at `relPath` (e.g. "src/main.rs") under `rootDir`, creating any
// intermediate directories, and returns the absolute WASI path.
function insertFile(rootDir, relPath, bytes) {
  const parts = relPath.split("/").filter(Boolean);
  let dir = rootDir;
  for (let i = 0; i < parts.length - 1; i++) {
    let next = dir.contents.get(parts[i]);
    if (!(next instanceof Directory)) {
      next = new Directory(new Map());
      dir.contents.set(parts[i], next);
    }
    dir = next;
  }
  dir.contents.set(parts[parts.length - 1], new File(bytes));
  return "/" + parts.join("/");
}

// Recursively reads a dropped `FileSystemEntry` (a plain file, or a directory - from
// dragging a whole folder in, Chromium/Firefox only) into a flat list of
// [relative path, bytes] pairs `handleDrop` can hand to `insertFile`.
function readEntry(entry, prefix) {
  return new Promise((resolve) => {
    if (entry.isFile) {
      entry.file((file) => {
        file.arrayBuffer().then((buf) => resolve([[prefix + entry.name, new Uint8Array(buf)]]));
      });
    } else if (entry.isDirectory) {
      const reader = entry.createReader();
      const children = [];
      const readBatch = () => {
        reader.readEntries((batch) => {
          if (batch.length === 0) {
            Promise.all(children.map((child) => readEntry(child, prefix + entry.name + "/"))).then(
              (lists) => resolve(lists.flat()),
            );
            return;
          }
          children.push(...batch);
          readBatch();
        });
      };
      readBatch();
    } else {
      resolve([]);
    }
  });
}

// Seeds every file dropped onto the page into the virtual filesystem's root and opens each
// in the current view (replacing whatever's focused there - dropping more than one file at
// once just leaves the last one open).
async function handleDrop(exports, rootDir, e) {
  e.preventDefault();

  const items = Array.from(e.dataTransfer.items || []);
  const entries = items.map((item) => item.webkitGetAsEntry?.()).filter(Boolean);

  let files;
  if (entries.length > 0) {
    files = (await Promise.all(entries.map((entry) => readEntry(entry, "")))).flat();
  } else {
    // Fallback for browsers without `webkitGetAsEntry` - flat files only, no folder drop.
    files = await Promise.all(
      Array.from(e.dataTransfer.files).map(async (f) => [f.name, new Uint8Array(await f.arrayBuffer())]),
    );
  }

  for (const [relPath, bytes] of files) {
    openPath(exports, insertFile(rootDir, relPath, bytes));
  }
}

async function main() {
  const { exports, rootDir } = await loadHelixWasm();

  const canvas = document.getElementById("screen");
  const ctx = canvas.getContext("2d");
  // Browsers only fire `paste`/`copy` reliably on editable elements (inputs, textareas,
  // contenteditable) - a bare, even focusable, <canvas> doesn't get them in most browsers.
  // Terminal emulators (xterm.js etc.) work around this with an always-focused, invisible
  // textarea that receives keyboard/clipboard events instead of the canvas; we do the same.
  const inputSink = document.getElementById("input-sink");

  // Canvas text doesn't trigger webfont loading the way CSS does (nothing here ever sets
  // `font-family: "Fira Code"` in a stylesheet rule), so without this the very first frame -
  // and the cell-width measurement below - would silently render with the fallback font
  // instead. Swallow failures (offline, blocked, etc.) and fall through to that same
  // fallback, already in FONT_FAMILY's stack.
  try {
    await document.fonts.load(`${FONT_SIZE}px ${FONT_FAMILY}`);
  } catch {
    // ignored
  }
  ctx.font = `${FONT_SIZE}px ${FONT_FAMILY}`;
  // Fira Code's advance width at this size isn't necessarily an integer, or the 8px this
  // grid used to hardcode - measure it once instead of guessing, so cells and glyphs always
  // agree regardless of font/size.
  const metrics = ctx.measureText("0");
  CELL_W = Math.max(1, Math.round(metrics.width));
  // `fontBoundingBoxAscent`/`Descent` reflect the font's real vertical metrics (unlike
  // `actualBoundingBox*`, which vary per glyph); fall back to a fixed ratio on the rare
  // engine that lacks them.
  if (metrics.fontBoundingBoxAscent !== undefined) {
    const ascent = metrics.fontBoundingBoxAscent;
    const descent = metrics.fontBoundingBoxDescent;
    TEXT_BASELINE_Y = ascent + (CELL_H - (ascent + descent)) / 2;
  }

  // Sizes the canvas' backing store to the *window*, not just an exact multiple of the cell
  // grid (so text stays crisp on hi-DPI displays, and there's no gap between the canvas and
  // the window edge for the page's own background to show through - see `draw`'s margin fill
  // below for the rest of that fix) and recomputes how many full cells fit; draw() re-reads
  // COLS/ROWS on every call, so it just works.
  function layoutCanvas() {
    const dpr = window.devicePixelRatio || 1;
    COLS = Math.max(1, Math.floor(window.innerWidth / CELL_W));
    ROWS = Math.max(1, Math.floor((window.innerHeight - PADDING_TOP) / CELL_H));

    canvas.width = window.innerWidth * dpr;
    canvas.height = window.innerHeight * dpr;
    canvas.style.width = `${window.innerWidth}px`;
    canvas.style.height = `${window.innerHeight}px`;

    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.font = `${FONT_SIZE}px ${FONT_FAMILY}`;
    ctx.textBaseline = "alphabetic";
  }

  function draw() {
    exports.hx_render();
    const ptr = exports.hx_frame_ptr();
    const len = exports.hx_frame_len();
    const cells = new Uint32Array(exports.memory.buffer, ptr, len / 4);
    const cellCount = cells.length / 3;

    // COLS*CELL_W/ROWS*CELL_H (the actual cell grid) is usually smaller than the canvas
    // (the full window, see layoutCanvas) by up to one cell's worth of leftover space at the
    // right/bottom edge. Paint that margin with the last cell's background (plain buffer
    // background, not gutter/cursor-line) instead of leaving it transparent, so it blends
    // with the theme instead of showing the page's own background through a seam.
    if (cellCount > 0) {
      ctx.fillStyle = rgb(cells[(cellCount - 1) * 3 + 2]);
      ctx.fillRect(0, 0, window.innerWidth, window.innerHeight);
    }
    for (let i = 0; i < cellCount; i++) {
      const ch = cells[i * 3];
      const fg = cells[i * 3 + 1];
      const bg = cells[i * 3 + 2];
      const x = (i % COLS) * CELL_W;
      const y = Math.floor(i / COLS) * CELL_H + PADDING_TOP;
      ctx.fillStyle = rgb(bg);
      ctx.fillRect(x, y, CELL_W, CELL_H);
      if (ch !== 0 && ch !== 32) {
        ctx.fillStyle = rgb(fg);
        ctx.fillText(String.fromCodePoint(ch), x, y + TEXT_BASELINE_Y);
      }
    }

    const col = exports.hx_cursor_col();
    const row = exports.hx_cursor_row();
    const kind = exports.hx_cursor_kind();
    // 0 = block, 1 = bar, 2 = underline, 3 = hidden (see hx_cursor_kind in main.rs).
    if (col >= 0 && row >= 0 && kind !== 3) {
      const x = col * CELL_W;
      const y = row * CELL_H + PADDING_TOP;
      ctx.fillStyle = "rgba(255,255,255,0.6)";
      if (kind === 0) ctx.fillRect(x, y, CELL_W, CELL_H);
      else if (kind === 2) ctx.fillRect(x, y + CELL_H - 2, CELL_W, 2);
      else ctx.fillRect(x, y, 2, CELL_H);
    }
  }

  layoutCanvas();
  exports.hx_init(COLS, ROWS);
  draw();
  inputSink.focus();

  window.addEventListener("resize", () => {
    layoutCanvas();
    exports.hx_resize(COLS, ROWS);
    draw();
  });

  // Clicking the canvas should still type into the editor, so redirect focus to the
  // (invisible) element that actually receives keyboard/clipboard events.
  canvas.addEventListener("mousedown", () => inputSink.focus());

  // Kept on `window` (not inputSink) so typing keeps working even if focus doesn't land on
  // inputSink for some reason - keydown bubbles to window regardless of focus target, but
  // `paste` (below) genuinely needs a focused editable element in most browsers, which is
  // the only thing inputSink is for.
  window.addEventListener("keydown", (e) => {
    // Ctrl/Cmd+V isn't bound to anything in Helix's own keymap outside a window-management
    // submap, so forwarding it as a key event would just hit insert mode's fallback of
    // inserting the literal "v" character. It exists here purely to trigger the browser's
    // native paste - so hand focus to inputSink right now (synchronously, so it's actually
    // focused by the time the OS delivers the paste this same keypress triggers) and leave
    // the event alone entirely, instead of forwarding or preventing it.
    // (keyToNotation doesn't read e.metaKey, so this checks the raw event instead.)
    if (e.key === "v" && (e.ctrlKey || e.metaKey)) {
      inputSink.focus();
      return;
    }

    const notation = keyToNotation(e);
    if (notation === null) return;
    sendKey(exports, notation);
    draw();
    e.preventDefault();
  });

  // Fires on Ctrl+V/Cmd+V while inputSink is focused; doesn't paste into the buffer by
  // itself, it just primes `"+p"`/`"+P"` with what was pasted (see handlePaste above).
  // Clear the textarea's own value afterward so it never accumulates stray content.
  inputSink.addEventListener("paste", (e) => {
    handlePaste(exports, e);
    inputSink.value = "";
  });

  // Drag a file (or, in Chromium/Firefox, a whole folder) onto the page to open it - see
  // `handleDrop`. `dragover` must call `preventDefault` too, or the browser refuses the drop.
  window.addEventListener("dragover", (e) => e.preventDefault());
  window.addEventListener("drop", (e) => {
    handleDrop(exports, rootDir, e).then(draw);
  });

  // Drives `hx_tick` (see main.rs) independent of key events - the local job executor,
  // config-reload etc., and the idle timer otherwise only ever advance when a key happens to
  // be pressed. Paused while the tab isn't visible so a backgrounded tab doesn't keep ticking.
  let tickTimer = null;
  function startTicking() {
    if (tickTimer !== null) return;
    tickTimer = setInterval(() => {
      exports.hx_tick();
      draw();
    }, 250);
  }
  function stopTicking() {
    clearInterval(tickTimer);
    tickTimer = null;
  }
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") startTicking();
    else stopTicking();
  });
  if (document.visibilityState === "visible") startTicking();
}

main();
