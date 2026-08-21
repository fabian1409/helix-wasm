//! Lets the wasm32 build's `:download` command hand file bytes to helix-wasm's FFI layer,
//! which forwards them to the JS host to save as a browser download. Mirrors the pending-copy
//! queue in [`crate::clipboard`] - there's no synchronous way to trigger a browser download
//! from inside a command, so the bytes are queued here and picked up by `hx_key` after the
//! triggering key event returns.
#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;

thread_local! {
    static PENDING_DOWNLOAD: RefCell<Option<(String, Vec<u8>)>> = RefCell::new(None);
}

/// Called by the `:download` command to queue a file for the JS host to save.
pub fn queue_download(name: String, bytes: Vec<u8>) {
    PENDING_DOWNLOAD.with(|p| *p.borrow_mut() = Some((name, bytes)));
}

/// Called by helix-wasm's `hx_key` export after handling a key event, to pick up any download
/// queued during it.
pub fn take_pending_download() -> Option<(String, Vec<u8>)> {
    PENDING_DOWNLOAD.with(|p| p.borrow_mut().take())
}
