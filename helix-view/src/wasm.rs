//! wasm32-only glue for helix-wasm's FFI layer. There's generally no synchronous browser API
//! to act on state an editor command produces mid-keystroke (write the clipboard, save a file,
//! open a URL), so each submodule queues it in a thread-local for the JS host to pick up after
//! the triggering `hx_key` call returns.

pub mod clipboard;
pub mod download;
pub mod open_url;
