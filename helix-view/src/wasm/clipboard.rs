//! Browser-backed [`ClipboardProvider`](super::super::clipboard::ClipboardProvider) for the
//! wasm32 build. There's no synchronous "read the system clipboard" browser API (the async
//! `navigator.clipboard` API is permission-gated and can't be awaited from these synchronous
//! methods - there's no bridge from a JS `Promise` into a poll of a wasm-side future). Instead:
//! the JS host listens for the browser's native `paste` event (Ctrl+V/Cmd+V), which hands over
//! clipboard text synchronously via `event.clipboardData`, and forwards it here via
//! `set_pasted_text` - so `"+p` pastes whatever was most recently delivered by an actual paste
//! gesture. Writes go the other way: `set_contents` stashes the text for helix-wasm's FFI layer
//! to hand to `navigator.clipboard.writeText` after the triggering key event returns.

use std::borrow::Cow;
use std::cell::RefCell;

use serde::{Deserialize, Serialize};

use crate::clipboard::{ClipboardType, Result};

thread_local! {
    static PASTED: RefCell<String> = RefCell::new(String::new());
    static PENDING_COPY: RefCell<Option<String>> = RefCell::new(None);
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum ClipboardProvider {
    #[default]
    Browser,
}

impl ClipboardProvider {
    pub fn detect() -> Self {
        Self::Browser
    }

    pub fn name(&self) -> Cow<'_, str> {
        "browser".into()
    }

    pub fn get_contents(&self, _clipboard_type: &ClipboardType) -> Result<String> {
        Ok(PASTED.with(|p| p.borrow().clone()))
    }

    pub fn set_contents(&self, content: &str, _clipboard_type: ClipboardType) -> Result<()> {
        PENDING_COPY.with(|p| *p.borrow_mut() = Some(content.to_owned()));
        Ok(())
    }
}

/// Called by helix-wasm's `hx_paste` export when the JS host's `paste` event handler
/// delivers clipboard text.
pub fn set_pasted_text(text: String) {
    PASTED.with(|p| *p.borrow_mut() = text);
}

/// Called by helix-wasm's `hx_key` export after handling a key event, to pick up any
/// clipboard write `set_contents` queued during it.
pub fn take_pending_copy() -> Option<String> {
    PENDING_COPY.with(|p| p.borrow_mut().take())
}
