//! Lets the wasm32 build's `open_url` (`gf` on a URL, etc.) hand an external URL to
//! helix-wasm's FFI layer, which forwards it to the JS host to open via `window.open`.
//! Mirrors the pending-download queue in [`super::download`] - queued here and picked up by
//! `hx_key` after the triggering key event returns.

use std::cell::RefCell;

thread_local! {
    static PENDING_OPEN_URL: RefCell<Option<String>> = RefCell::new(None);
}

/// Called by `open_url` to queue an external URL for the JS host to open.
pub fn queue_open_url(url: String) {
    PENDING_OPEN_URL.with(|p| *p.borrow_mut() = Some(url));
}

/// Called by helix-wasm's `hx_key` export after handling a key event, to pick up any URL
/// queued during it.
pub fn take_pending_open_url() -> Option<String> {
    PENDING_OPEN_URL.with(|p| p.borrow_mut().take())
}
