//! A queue of async messages/errors that will be shown in the editor

use std::borrow::Cow;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

use crate::{runtime_local, send_blocking};
use once_cell::sync::OnceCell;
use tokio::sync::mpsc::{Receiver, Sender};

/// Describes the severity level of a [`StatusMessage`].
#[derive(Debug, Clone, Copy, Eq, PartialEq, PartialOrd, Ord)]
pub enum Severity {
    Hint,
    Info,
    Warning,
    Error,
}

pub struct StatusMessage {
    pub severity: Severity,
    pub message: Cow<'static, str>,
}

impl From<anyhow::Error> for StatusMessage {
    fn from(err: anyhow::Error) -> Self {
        StatusMessage {
            severity: Severity::Error,
            message: err.to_string().into(),
        }
    }
}

impl From<&'static str> for StatusMessage {
    fn from(msg: &'static str) -> Self {
        StatusMessage {
            severity: Severity::Info,
            message: msg.into(),
        }
    }
}

runtime_local! {
    static MESSAGES: OnceCell<Sender<StatusMessage>> = OnceCell::new();
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn report(msg: impl Into<StatusMessage>) {
    // if the error channel overflows just ignore it
    let _ = MESSAGES
        .wait()
        .send_timeout(msg.into(), Duration::from_millis(10))
        .await;
}

// No `tokio::time` on wasm32 to time a `send_timeout` out with - just a best-effort, drop it
// if the (128-deep) channel happens to be full.
#[cfg(target_arch = "wasm32")]
pub async fn report(msg: impl Into<StatusMessage>) {
    let _ = MESSAGES.wait().try_send(msg.into());
}

pub fn report_blocking(msg: impl Into<StatusMessage>) {
    let messages = MESSAGES.wait();
    send_blocking(messages, msg.into())
}

/// Must be called once during editor startup exactly once
/// before any of the messages in this module can be used
///
/// # Panics
/// If called multiple times
pub fn setup() -> Receiver<StatusMessage> {
    let (tx, rx) = tokio::sync::mpsc::channel(128);
    let _ = MESSAGES.set(tx);
    rx
}
