import WASI from "./vendor/browser_wasi_shim/wasi.js";
import { Fd } from "./vendor/browser_wasi_shim/fd.js";
import { ConsoleStdout } from "./vendor/browser_wasi_shim/fs_mem.js";

const CELL_W = 8, CELL_H = 16;
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
async function loadHelixWasm() {
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
    ],
  );

  const { instance } = await WebAssembly.instantiateStreaming(fetch("./helix_wasm.wasm"), {
    wasi_snapshot_preview1: wasi.wasiImport,
  });

  wasi.start(instance);
  return instance.exports;
}

// Writes `notation`'s UTF-8 bytes into the scratch buffer `hx_key_buf_ptr` exposes (see
// `KEY_BUF` in helix-wasm/src/main.rs) and calls `hx_key` to consume them. Memory's
// backing ArrayBuffer must be re-read on every call, since growing wasm memory replaces it.
function sendKey(exports, notation) {
  const bytes = new TextEncoder().encode(notation);
  if (bytes.length > exports.hx_key_buf_capacity()) return;
  const ptr = exports.hx_key_buf_ptr();
  new Uint8Array(exports.memory.buffer, ptr, bytes.length).set(bytes);
  exports.hx_key(bytes.length);
}

async function main() {
  const exports = await loadHelixWasm();

  const canvas = document.getElementById("screen");
  const ctx = canvas.getContext("2d");

  // Sizes the canvas' backing store to the window at the device's actual pixel
  // density (so text stays crisp on hi-DPI displays) and recomputes how many
  // cells fit; draw() re-reads COLS/ROWS on every call, so it just works.
  function layoutCanvas() {
    const dpr = window.devicePixelRatio || 1;
    COLS = Math.max(1, Math.floor(window.innerWidth / CELL_W));
    ROWS = Math.max(1, Math.floor(window.innerHeight / CELL_H));

    canvas.width = COLS * CELL_W * dpr;
    canvas.height = ROWS * CELL_H * dpr;
    canvas.style.width = `${COLS * CELL_W}px`;
    canvas.style.height = `${ROWS * CELL_H}px`;

    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.font = `${CELL_H}px monospace`;
    ctx.textBaseline = "top";
  }

  function draw() {
    exports.hx_render();
    const ptr = exports.hx_frame_ptr();
    const len = exports.hx_frame_len();
    const cells = new Uint32Array(exports.memory.buffer, ptr, len / 4);
    const cellCount = cells.length / 3;

    ctx.clearRect(0, 0, COLS * CELL_W, ROWS * CELL_H);
    for (let i = 0; i < cellCount; i++) {
      const ch = cells[i * 3];
      const fg = cells[i * 3 + 1];
      const bg = cells[i * 3 + 2];
      const x = (i % COLS) * CELL_W;
      const y = Math.floor(i / COLS) * CELL_H;
      ctx.fillStyle = rgb(bg);
      ctx.fillRect(x, y, CELL_W, CELL_H);
      if (ch !== 0 && ch !== 32) {
        ctx.fillStyle = rgb(fg);
        ctx.fillText(String.fromCodePoint(ch), x, y);
      }
    }

    const col = exports.hx_cursor_col();
    const row = exports.hx_cursor_row();
    const kind = exports.hx_cursor_kind();
    // 0 = block, 1 = bar, 2 = underline, 3 = hidden (see hx_cursor_kind in main.rs).
    if (col >= 0 && row >= 0 && kind !== 3) {
      const x = col * CELL_W;
      const y = row * CELL_H;
      ctx.fillStyle = "rgba(255,255,255,0.6)";
      if (kind === 0) ctx.fillRect(x, y, CELL_W, CELL_H);
      else if (kind === 2) ctx.fillRect(x, y + CELL_H - 2, CELL_W, 2);
      else ctx.fillRect(x, y, 2, CELL_H);
    }
  }

  layoutCanvas();
  exports.hx_init(COLS, ROWS);
  draw();

  window.addEventListener("resize", () => {
    layoutCanvas();
    exports.hx_resize(COLS, ROWS);
    draw();
  });

  window.addEventListener("keydown", (e) => {
    const notation = keyToNotation(e);
    if (notation === null) return;
    sendKey(exports, notation);
    draw();
    e.preventDefault();
  });
}

main();
