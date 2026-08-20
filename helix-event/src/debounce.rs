//! Utilities for declaring an async (usually debounced) hook

use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
use futures_executor::block_on;
use tokio::sync::mpsc::{self, error::TrySendError, Sender};

// wasm32-wasip1 can't drive `tokio::time` at all (see `AsyncHook::spawn`'s wasm32 arm below),
// but `AsyncHook::handle_event`'s signature still needs *some* `Instant` type to exist - the
// two are interchangeable for callers, since neither target's `Hook::handle_event` impl does
// arithmetic tokio's `Instant` supports but `std`'s doesn't.
#[cfg(not(target_arch = "wasm32"))]
pub use tokio::time::Instant;
#[cfg(target_arch = "wasm32")]
pub use std::time::Instant;

/// Async hooks provide a convenient framework for implementing (debounced)
/// async event handlers. Most synchronous event hooks will likely need to
/// debounce their events, coordinate multiple different hooks and potentially
/// track some state. `AsyncHooks` facilitate these use cases by running as
/// a background tokio task that waits for events (usually an enum) to be
/// sent through a channel.
pub trait AsyncHook: Sync + Send + 'static + Sized {
    type Event: Sync + Send + 'static;
    /// Called immediately whenever an event is received, this function can
    /// consume the event immediately or debounce it. In case of debouncing,
    /// it can either define a new debounce timeout or continue the current one
    fn handle_event(&mut self, event: Self::Event, timeout: Option<Instant>) -> Option<Instant>;

    /// Called whenever the debounce timeline is reached
    fn finish_debounce(&mut self);

    fn spawn(self) -> mpsc::Sender<Self::Event> {
        // the capacity doesn't matter too much here, unless the cpu is totally overwhelmed
        // the cap will never be reached since we always immediately drain the channel
        // so it should only be reached in case of total CPU overload.
        // However, a bounded channel is much more efficient so it's nice to use here
        let (tx, rx) = mpsc::channel(128);
        // wasm32-wasip1 has no `tokio::time` (the debounce timeout `run` waits on) or working
        // `tokio::spawn` to begin with - same as the "not inside a runtime" case below (which
        // exists for unit tests), events sent into `tx` are just never picked up. Debounced
        // hooks are all LSP-adjacent today so this isn't a regression in practice, but if that
        // changes, this is the place a wasm32-appropriate driver would go.
        #[cfg(not(target_arch = "wasm32"))]
        // only spawn worker if we are inside runtime to avoid having to spawn a runtime for unrelated unit tests
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::spawn(run(self, rx));
        }
        #[cfg(target_arch = "wasm32")]
        let _ = rx;
        tx
    }
}

#[cfg(not(target_arch = "wasm32"))]
async fn run<Hook: AsyncHook>(mut hook: Hook, mut rx: mpsc::Receiver<Hook::Event>) {
    let mut deadline = None;
    loop {
        let event = match deadline {
            Some(deadline_) => {
                let res = tokio::time::timeout_at(deadline_, rx.recv()).await;
                match res {
                    Ok(event) => event,
                    Err(_) => {
                        hook.finish_debounce();
                        deadline = None;
                        continue;
                    }
                }
            }
            None => rx.recv().await,
        };
        let Some(event) = event else {
            break;
        };
        deadline = hook.handle_event(event, deadline);
    }
}

pub fn send_blocking<T>(tx: &Sender<T>, data: T) {
    // block_on has some overhead and in practice the channel should basically
    // never be full anyway so first try sending without blocking
    if let Err(TrySendError::Full(data)) = tx.try_send(data) {
        // set a timeout so that we just drop a message instead of freezing the editor in the worst case
        #[cfg(not(target_arch = "wasm32"))]
        let _ = block_on(tx.send_timeout(data, Duration::from_millis(10)));
        // No `tokio::time` on wasm32 to time a wait out with - the channel being full at all is
        // already the rare/overload case this exists for, so just drop the message.
        #[cfg(target_arch = "wasm32")]
        let _ = data;
    }
}
