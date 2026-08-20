// The FFI glue below drives `helix_view::input::KeyEvent`/`helix_term::application::Application`
// entry points that only exist on `wasm32-wasip1` (no terminal, no tokio executor).
// This crate has no meaningful native build, so everything but `main` lives behind this cfg.
#[cfg(target_arch = "wasm32")]
mod app {
    use std::cell::RefCell;
    use std::os::raw::c_int;
    use std::str::FromStr;
    use std::sync::Once;

    use helix_core::syntax::{self, config::Configuration};
    use helix_loader::workspace_trust::{self, WorkspaceTrust};
    use helix_term::{application::Application, args::Args, config::Config};
    use helix_view::{
        graphics::{Color, CursorKind, Modifier},
        input::KeyEvent,
    };

    /// Max length (bytes) of a key notation string (e.g. "S-A-C-left") the JS host can
    /// write into `KEY_BUF` before calling `hx_key`.
    const KEY_BUF_CAPACITY: usize = 64;

thread_local! {
    static APP: RefCell<Option<Application>> = RefCell::new(None);
    static FRAME: RefCell<Vec<u8>> = RefCell::new(Vec::new());
    static CURSOR: RefCell<(i32, i32)> = RefCell::new((-1, -1));
    static CURSOR_KIND: RefCell<CursorKind> = RefCell::new(CursorKind::Hidden);
    static KEY_BUF: RefCell<[u8; KEY_BUF_CAPACITY]> = RefCell::new([0; KEY_BUF_CAPACITY]);
    // Clipboard writes (`"+y` etc.) queued by `helix_view::clipboard` during `hx_key`, for
    // the JS host to hand to `navigator.clipboard.writeText` afterwards.
    static COPY_BUF: RefCell<Vec<u8>> = RefCell::new(Vec::new());
    // Scratch buffer the JS host writes clipboard text into (on a browser `paste` event)
    // before calling `hx_paste`; unlike KEY_BUF this is unbounded since pasted text has no
    // fixed max length, so it's grown to fit via `hx_paste_alloc` instead of a fixed array.
    static PASTE_BUF: RefCell<Vec<u8>> = RefCell::new(Vec::new());
}

fn cursor_kind_id(kind: CursorKind) -> c_int {
    match kind {
        CursorKind::Block => 0,
        CursorKind::Bar => 1,
        CursorKind::Underline => 2,
        CursorKind::Hidden => 3,
    }
}

static PANIC_HOOK: Once = Once::new();

fn with_app<R>(f: impl FnOnce(&mut Application) -> R) -> Option<R> {
    APP.with(|cell| cell.borrow_mut().as_mut().map(f))
}

fn build_application(cols: u16, rows: u16) -> anyhow::Result<Application> {
    // Same embedded `languages.toml` native Helix uses, so file-type detection, comment
    // tokens, indentation etc. all come along for free; `get_language` in helix-loader
    // only actually has a grammar to hand back for "rust" (see helix-loader/src/grammar.rs),
    // so every other configured language is otherwise inert here, same as native Helix
    // when a grammar isn't built.
    let lang_config: Configuration = helix_loader::config::default_lang_config().try_into()?;
    let lang_loader = syntax::Loader::new(lang_config)?;
    let workspace_trust = WorkspaceTrust::new(workspace_trust::Config::default());
    let mut app = Application::new(Args::default(), Config::default(), lang_loader, workspace_trust)?;
    app.resize(cols, rows);

    // There's no file path in the browser for language auto-detection to key off of, so
    // explicitly set the language on the single empty buffer `Application::new` creates.
    let loader = app.editor.syn_loader.load();
    if let Some(doc) = app.editor.documents_mut().next() {
        doc.set_language_by_language_id("rust", &loader)?;
    }

    Ok(app)
}

/// Standard xterm 16-color palette, indexed 0-15.
fn ansi16_rgb(i: u8) -> (u8, u8, u8) {
    const TABLE: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 0, 0),
        (0, 205, 0),
        (205, 205, 0),
        (0, 0, 238),
        (205, 0, 205),
        (0, 205, 205),
        (229, 229, 229),
        (127, 127, 127),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (92, 92, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    TABLE[i as usize]
}

/// Standard xterm 256-color cube/grayscale-ramp formula for indices 16-255.
fn indexed_rgb(i: u8) -> (u8, u8, u8) {
    match i {
        0..=15 => ansi16_rgb(i),
        16..=231 => {
            let i = i - 16;
            let scale = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            (scale(i / 36), scale((i % 36) / 6), scale(i % 6))
        }
        232..=255 => {
            let v = 8 + (i - 232) * 10;
            (v, v, v)
        }
    }
}

fn color_to_rgb(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Reset => (0, 0, 0),
        Color::Black => (0, 0, 0),
        Color::Red => (205, 0, 0),
        Color::Green => (0, 205, 0),
        Color::Yellow => (205, 205, 0),
        Color::Blue => (0, 0, 238),
        Color::Magenta => (205, 0, 205),
        Color::Cyan => (0, 205, 205),
        Color::Gray => (229, 229, 229),
        Color::LightRed => (255, 0, 0),
        Color::LightGreen => (0, 255, 0),
        Color::LightYellow => (255, 255, 0),
        Color::LightBlue => (92, 92, 255),
        Color::LightMagenta => (255, 0, 255),
        Color::LightCyan => (0, 255, 255),
        Color::LightGray => (127, 127, 127),
        Color::White => (255, 255, 255),
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(i) => indexed_rgb(i),
    }
}

fn pack_rgb((r, g, b): (u8, u8, u8)) -> u32 {
    (r as u32) << 16 | (g as u32) << 8 | b as u32
}

#[no_mangle]
pub extern "C" fn hx_init(cols: u32, rows: u32) -> c_int {
    PANIC_HOOK.call_once(|| {
        std::panic::set_hook(Box::new(|info| eprintln!("helix-wasm panicked: {info}")));
    });
    match build_application(cols as u16, rows as u16) {
        Ok(app) => {
            APP.with(|cell| *cell.borrow_mut() = Some(app));
            0
        }
        Err(err) => {
            eprintln!("helix-wasm: failed to initialize editor: {err:#}");
            -1
        }
    }
}

#[no_mangle]
pub extern "C" fn hx_resize(cols: u32, rows: u32) {
    with_app(|app| app.resize(cols as u16, rows as u16));
}

#[no_mangle]
pub extern "C" fn hx_key_buf_ptr() -> *mut u8 {
    KEY_BUF.with(|buf| buf.borrow_mut().as_mut_ptr())
}

#[no_mangle]
pub extern "C" fn hx_key_buf_capacity() -> usize {
    KEY_BUF_CAPACITY
}

/// Reads `len` UTF-8 bytes of a key notation string (e.g. "a", "ret", "C-w", "S-A-left")
/// that the caller has written into the buffer at `hx_key_buf_ptr()` (up to
/// `hx_key_buf_capacity()` bytes), then feeds it to the editor as a key event.
#[no_mangle]
pub extern "C" fn hx_key(len: u32) {
    let key = KEY_BUF.with(|buf| {
        let buf = buf.borrow();
        let len = (len as usize).min(KEY_BUF_CAPACITY);
        std::str::from_utf8(&buf[..len]).map(str::to_owned)
    });
    let Ok(key) = key else {
        return;
    };
    let Ok(event) = KeyEvent::from_str(&key) else {
        return;
    };
    with_app(|app| app.handle_key(event));

    if let Some(text) = helix_view::clipboard::take_pending_copy() {
        COPY_BUF.with(|buf| *buf.borrow_mut() = text.into_bytes());
    }
}

/// Non-zero if a clipboard write (e.g. `"+y`) happened during the last `hx_key` call and
/// is waiting to be handed to `navigator.clipboard.writeText`; read it via
/// `hx_copy_ptr`/`hx_copy_len`, then call `hx_copy_clear`.
#[no_mangle]
pub extern "C" fn hx_copy_len() -> usize {
    COPY_BUF.with(|buf| buf.borrow().len())
}

#[no_mangle]
pub extern "C" fn hx_copy_ptr() -> *const u8 {
    COPY_BUF.with(|buf| buf.borrow().as_ptr())
}

#[no_mangle]
pub extern "C" fn hx_copy_clear() {
    COPY_BUF.with(|buf| buf.borrow_mut().clear());
}

/// Grows the scratch buffer read by `hx_paste` to `len` bytes and returns a pointer for the
/// caller to write UTF-8 clipboard text into (e.g. from a browser `paste` event's
/// `clipboardData`) before calling `hx_paste(len)`.
#[no_mangle]
pub extern "C" fn hx_paste_alloc(len: u32) -> *mut u8 {
    PASTE_BUF.with(|buf| {
        let mut buf = buf.borrow_mut();
        buf.clear();
        buf.resize(len as usize, 0);
        buf.as_mut_ptr()
    })
}

/// Reads `len` UTF-8 bytes from the buffer at `hx_paste_alloc`'s pointer and makes them the
/// contents `"+p`/`"+P` (and other system-clipboard reads) will paste until the next call.
#[no_mangle]
pub extern "C" fn hx_paste(len: u32) {
    let text = PASTE_BUF.with(|buf| {
        let buf = buf.borrow();
        let len = (len as usize).min(buf.len());
        std::str::from_utf8(&buf[..len]).map(str::to_owned)
    });
    if let Ok(text) = text {
        helix_view::clipboard::set_pasted_text(text);
    }
}

/// Renders a frame and packs it into the buffer read by `hx_frame_ptr`/`hx_frame_len`: 12 bytes
/// per cell (codepoint, fg as 0x00RRGGBB, bg as 0x00RRGGBB, all little-endian u32), row-major.
#[no_mangle]
pub extern "C" fn hx_render() {
    with_app(|app| {
        let backend = app.render_frame();
        let buffer = backend.buffer();

        let mut packed = Vec::with_capacity(buffer.content.len() * 12);
        for cell in &buffer.content {
            let ch = cell.symbol.chars().next().unwrap_or(' ') as u32;
            let (fg, bg) = (color_to_rgb(cell.fg), color_to_rgb(cell.bg));
            // The block cursor (and other reverse-video styles) is baked into the buffer as a
            // `reversed` modifier rather than explicit colors, since that's how a real terminal
            // renders it; we have no terminal to do that for us, so swap the colors ourselves.
            let (fg, bg) = if cell.modifier.contains(Modifier::REVERSED) {
                (bg, fg)
            } else {
                (fg, bg)
            };
            packed.extend_from_slice(&ch.to_le_bytes());
            packed.extend_from_slice(&pack_rgb(fg).to_le_bytes());
            packed.extend_from_slice(&pack_rgb(bg).to_le_bytes());
        }
        FRAME.with(|frame| *frame.borrow_mut() = packed);

        let cursor = backend
            .cursor_position()
            .map(|(x, y)| (x as i32, y as i32))
            .unwrap_or((-1, -1));
        CURSOR.with(|c| *c.borrow_mut() = cursor);
        CURSOR_KIND.with(|k| *k.borrow_mut() = backend.cursor_kind());
    });
}

#[no_mangle]
pub extern "C" fn hx_frame_ptr() -> *const u8 {
    FRAME.with(|f| f.borrow().as_ptr())
}

#[no_mangle]
pub extern "C" fn hx_frame_len() -> usize {
    FRAME.with(|f| f.borrow().len())
}

#[no_mangle]
pub extern "C" fn hx_cursor_col() -> c_int {
    CURSOR.with(|c| c.borrow().0)
}

#[no_mangle]
pub extern "C" fn hx_cursor_row() -> c_int {
    CURSOR.with(|c| c.borrow().1)
}

/// 0 = block, 1 = bar, 2 = underline, 3 = hidden.
#[no_mangle]
pub extern "C" fn hx_cursor_kind() -> c_int {
    CURSOR_KIND.with(|k| cursor_kind_id(*k.borrow()))
}
}

fn main() {}
