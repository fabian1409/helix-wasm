# Helix in the browser (`helix-wasm`)

This fork compiles Helix to `wasm32-wasip1` and runs it fully client-side: a canvas-rendered
terminal UI, driven by real keyboard/paste/drop events, with no server component. This document
describes the current architecture — how the pieces fit together and why they're built the way
they are.

## Crate layout

- **`helix-wasm`** — a small bin crate (`src/main.rs`) that owns the entire wasm target. It
  builds an `Application` from `helix-term` and exposes a hand-rolled C-ABI (`extern "C"`,
  `#[no_mangle]`) surface for a JS host to drive — no `wasm-bindgen`/`web-sys` anywhere in this
  build.
- **`helix-wasm/www`** — the JS host and the thing you actually serve. `index.html` + `index.js`
  load the compiled module against a minimal WASI shim, translate DOM events into calls across
  the C-ABI boundary, and paint the returned cell grid onto a `<canvas>`.
- **`helix-core` / `helix-view` / `helix-term`** — the same crates native Helix uses, built with
  `default-features = false` for wasm (no `lsp`, `dap`, or `git` features), plus `#[cfg(target_arch
  = "wasm32")]` branches wherever the browser needs a different implementation.
- **`helix-tui`** — gets a `WasmBackend` (`helix-tui/src/backend/wasm.rs`, behind the
  `wasm-backend` feature), a headless backend that renders into an in-memory `Buffer` instead of a
  real terminal, sitting alongside `crossterm.rs`/`termina.rs`/`test.rs` as another backend impl.
- **`helix-view::wasm`** — the module wasm-only editor-side glue lives under (see
  [Bridging state back to JS](#bridging-state-back-to-js) below).

## Building and running

```
cargo xtask wasi-sdk-install   # once: downloads a pinned clang+wasi-libc toolchain into wasi-sdk/
cargo xtask wasm               # builds helix-wasm, copies the .wasm into helix-wasm/www/
```

`cargo xtask wasm` builds with `--profile wasm` (`Cargo.toml`): inherits `release`, adds
`lto = "fat"`, `codegen-units = 1`, `strip = true`, `opt-level = "z"` — every byte here ships over
the network, so this optimizes for size over speed. `helix-wasm/www/` is both the source for the
hand-written JS/HTML and the directory you serve as-is; there's no separate assembled `dist/`.

## The C-ABI boundary

`helix-wasm/src/main.rs` keeps all editor state in thread-locals (`APP`, `FRAME`, `CURSOR`, scratch
buffers) and exposes it through a fixed set of exports:

| Export | Purpose |
|---|---|
| `hx_init(cols, rows)` | Builds the `Application` (loads config/theme/languages from the virtual fs) |
| `hx_resize(cols, rows)` | Window resize |
| `hx_tick()` | Advances the job executor, config/status queues, idle timer — called on a JS timer, independent of keys |
| `hx_key_buf_ptr/capacity`, `hx_key(len)` | Feeds a key-notation string (e.g. `"C-w"`) as a key event |
| `hx_open_path_alloc(len)`, `hx_open_path(len)` | Opens a WASI path in the current view (used for drag-and-drop) |
| `hx_render()`, `hx_frame_ptr/len` | Renders a frame and packs it as 12 bytes/cell (codepoint, fg `0xRRGGBB`, bg `0xRRGGBB`) |
| `hx_cursor_col/row/kind` | Cursor position and shape (0=block, 1=bar, 2=underline, 3=hidden) |
| `hx_copy_len/ptr/clear` | Clipboard write queued during the last `hx_key` |
| `hx_paste_alloc(len)`, `hx_paste(len)` | Feeds clipboard text in from a browser `paste` event |
| `hx_download_len/ptr`, `hx_download_name_len/ptr`, `hx_download_clear` | File queued by `:write`/`:download` |
| `hx_open_url_len/ptr/clear` | External URL queued by `gf` on a link |

There's no async bridge from JS into wasm: everything above is a plain synchronous call. Reads and
writes memory directly through `exports.memory.buffer` at the pointers these functions return —
that buffer must be re-read on every call, since growing wasm memory replaces it.

### Bridging state back to JS

Several editor actions (clipboard writes, file downloads, opening a URL) need a browser API that
only JS can call, and there's no way to call into JS synchronously from a running command. The
fix is the same shape every time: an `helix_view::wasm::*` submodule holds a thread-local
`Option<T>`, the command queues into it, and `hx_key` picks it up right after
`app.handle_key(event)` returns, staging it into a buffer the JS host reads via the ptr/len/clear
exports above. `sendKey` in `index.js` flushes all three (`flushClipboardCopy`,
`flushDownload`, `flushOpenUrl`) after every key event.

This lives in `helix-view/src/wasm.rs` (gated by `#[cfg(target_arch = "wasm32")]` on its `pub mod
wasm;` declaration in `lib.rs`, so it compiles to nothing on native):

- `wasm::clipboard` — the wasm `ClipboardProvider` (see [Clipboard](#clipboard))
- `wasm::download` — `queue_download`/`take_pending_download`, used by `:write`/`:download`
- `wasm::open_url` — `queue_open_url`/`take_pending_open_url`, used by `gf` on a URL

`helix_view::clipboard::ClipboardProvider` re-exports `wasm::clipboard`'s type under
`#[cfg(target_arch = "wasm32")]` so callers keep using `helix_view::clipboard::*` regardless of
target; `download`/`open_url` are called as `helix_view::wasm::download::*` /
`helix_view::wasm::open_url::*` directly from `helix-term`.

## Filesystem

There's exactly one WASI preopen, at `/`, backed by an in-memory, session-only filesystem
(`browser_wasi_shim`, vendored under `helix-wasm/www/vendor/`) — nothing here touches a real disk
and nothing persists across a page reload. `helix_loader::config_dir()`/`cache_dir()`/`data_dir()`
all resolve to `/.config` on wasm32 (there's no config/cache/data split in a single-directory
virtual fs, and `std::env::home_dir()` never resolves on `wasm32-wasip1` regardless of `$HOME`) —
kept out of `/` itself, dot-prefixed, so it doesn't clutter the file picker/explorer. Dropping a
`config.toml`/`languages.toml` at `/.config` therefore takes effect exactly like a real Helix
config file would. Dragging a file (or, in Chromium/Firefox, a whole folder) onto the page seeds
it into `/` and opens it (`handleDrop` → `hx_open_path`), unaffected by any of this.

Before `hx_init` runs, `index.js`'s `seedRuntimeFiles` fetches `www/runtime.tar.gz` — a gzipped
tarball `cargo xtask wasm`'s `build_runtime_archive` packs all of `runtime/queries/` and
`runtime/themes/` into (not just the statically-linked grammars' queries - there's no reason to
curate now that they're read from the virtual fs like native Helix reads them from disk) —
decompresses it with the browser's native `DecompressionStream("gzip")`, unpacks the resulting
USTAR bytes with a small hand-rolled `parseTar`, and writes every entry into
`/.config/runtime/**`, matching where `helix_loader::runtime_dirs()` and the theme `Loader`
already look.

## Async model

wasm32-wasip1 has no tokio reactor, so most of the concurrency native Helix relies on doesn't
exist here:

- `Jobs` (`helix-term/src/job.rs`) drives non-waited jobs through a `futures_executor::LocalPool`
  instead of `tokio::spawn`. `Application::wasm_tick` (`helix-term/src/application.rs`) is the only
  thing that polls it (`poll_wasm` → `run_until_stalled`), and also drains the callback/status
  channels and checks the idle timer directly instead of `tokio::select!`. It runs after every key
  event (`handle_key`) and from `tick` (driven by `hx_tick`), so idle-timeout behavior and job
  completions surface even without a keystroke.
- `render()` never actually awaits anything on wasm, so a trivial `futures_executor::block_on` is
  enough to drive it.
- Debouncing (`helix-event::debounce`) and the completion word index similarly fall back to
  polling/executor-agnostic paths rather than `tokio::time::sleep`.

## Rendering and input

`WasmBackend` renders into an in-memory `Buffer`; `hx_render` packs that buffer into 12
bytes/cell and `index.js`'s `draw()` blits it onto a 2D canvas context, one `fillRect` +
`fillText` per cell. Font metrics (cell width, baseline offset) are measured once from the
loaded "Fira Code" font (Google Fonts, with a `monospace` fallback if blocked) rather than
hardcoded, so layout stays correct regardless of what actually renders. `Color::Reset` (an
unstyled cell's fg/bg — no ambient terminal default exists here) falls back to the theme's own
`ui.text`/`ui.background`; the reversed-video modifier used for e.g. the block cursor swaps fg/bg
manually since there's no real terminal to do it. Keyboard events are translated to Helix's own
key notation (`keyToNotation`) and fed through `hx_key`; a dedicated invisible, focused `<textarea>`
(`#input-sink`) exists solely because most browsers only fire `paste`/`copy` on an editable
element, not a bare canvas.

## Clipboard

There's no synchronous "read the system clipboard" browser API (`navigator.clipboard` is async
and permission-gated, with no way to bridge a JS `Promise` into a poll of a wasm-side future).
So reads and writes go through different paths:

- **Paste** (`"+p`): the JS host listens for the browser's native `paste` event, hands the text
  over via `event.clipboardData` (synchronous), and forwards it into wasm via
  `hx_paste_alloc`/`hx_paste`. `"+p` always pastes whatever the most recent real paste gesture
  delivered.
- **Yank** (`"+y`): `ClipboardProvider::set_contents` queues the text; `hx_key` picks it up into
  `COPY_BUF`, and `flushClipboardCopy` hands it to `navigator.clipboard.writeText` after the
  triggering key event returns.

## Saving files

`:write`/`:download` encode the buffer and call `helix_view::wasm::download::queue_download`.
`hx_key` stages the bytes/name into `DOWNLOAD_BUF`/`DOWNLOAD_NAME_BUF`; `flushDownload` builds a
`Blob` and clicks a throwaway `<a download>` — the standard way to trigger a browser "Save As"
without a real server response.

## Opening external URLs

`gf` on a link (`open_url` in `helix-term/src/commands.rs`) calls
`helix_view::wasm::open_url::queue_open_url` instead of spawning `xdg-open`/`open` (which native
does via the `open` crate in `helix-term/src/lib.rs`). `hx_key` stages the URL into
`OPEN_URL_BUF`; `flushOpenUrl` calls `window.open(url, "_blank", "noopener,noreferrer")` right
after the triggering keypress, so it isn't blocked as an unsolicited popup.

## Language and syntax support

There's no `dlopen` on wasm32 and nothing to fetch/compile a grammar from at runtime in a browser,
so `helix_loader::grammar::get_language` statically links a fixed, curated set instead of loading
`.so`/`.dylib` grammars: **Rust, TOML, JSON, JavaScript, TypeScript, Bash, Markdown (+
Markdown-inline)**. Their query files (`highlights.scm` etc.) are read from `/.config/runtime/`
the same way native Helix reads `runtime/queries/` (`load_runtime_file` has no wasm32-specific
arm anymore) — see [Filesystem](#filesystem) for how they get there before `hx_init` runs. The
single empty buffer `Application::new` creates on startup is left with no language set — there's
no file path in the browser to auto-detect one from, and forcing a specific language would be an
arbitrary choice.

The custom `runtime/themes/custom.toml` (an onedark-based theme, still embedded into the binary
via `CUSTOM_THEME_DATA` in `helix-view/src/theme.rs`) is used as the default whenever a
dropped-in `config.toml` doesn't set its own `theme`. All of `runtime/themes/` is copied into
`/.config/runtime/` the same way the query files are, so a `config.toml` can select any of
Helix's built-in themes by name, not just the embedded `custom` one.

## What works vs. what's stubbed

Most editor commands and pickers work unmodified against the virtual filesystem: the file/dir
picker, file explorer, `gf`/`goto_file`, global search, and the workspace symbol picker all walk
the WASI root the same way they'd walk a real workspace (search/symbol-picker walks run serially
on wasm — `wasip1` has no threads for `ignore`'s `build_parallel()`). The write/quit command
family (`:w`, `:wq`, `:x`, `:wa`, `:xa`, `:q`, `:q!`, ...) has wasm32-specific implementations in
`helix-term/src/commands/typed.rs` that skip the native LSP-write-flush step
(`cx.block_try_flush_writes()`) but otherwise behave the same; `:reload`, `:reload-all`,
`:config-open`, `:config-open-workspace`, `:log-open`, and `:move`/`:move!` all work too.

Stubbed out entirely (report a "not supported in this build" error instead):

- **Shell commands** (`shell_pipe`, `shell_insert_output`, etc.) — spawning a real process is
  impossible in a browser regardless of LSP/DAP.
- **`changed_file_picker`** — needs git status, which this build doesn't have (`git` feature is
  off).

LSP and DAP are compiled out entirely (`default-features = false` on `helix-view`/`helix-term`
for the wasm build) rather than stubbed piecemeal — there's no language server or debug adapter
process to spawn in a browser.

## Driving loop (JS side)

`index.js`'s `main()` loads the module, measures font metrics, calls `hx_init`, and wires up:
`keydown` → `sendKey` → `draw()`; `paste` → `handlePaste`; `drop`/`dragover` → `handleDrop`;
`resize` → `hx_resize` + `draw()`. A `setInterval(… , 250)` timer calls `hx_tick()` + `draw()`
independent of key events (paused via the Page Visibility API while the tab isn't visible), so
the idle timer and any in-flight job can still make progress without a keystroke.
