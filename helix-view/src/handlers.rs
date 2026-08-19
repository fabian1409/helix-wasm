#[cfg(feature = "lsp")]
use completion::{CompletionEvent, CompletionHandler};
#[cfg(feature = "lsp")]
use helix_event::send_blocking;
use tokio::sync::mpsc::Sender;

#[cfg(feature = "lsp")]
use crate::handlers::lsp::SignatureHelpInvoked;
#[cfg(feature = "lsp")]
use crate::{DocumentId, Editor, ViewId};

#[cfg(feature = "lsp")]
pub mod completion;
#[cfg(feature = "dap")]
pub mod dap;
#[cfg(feature = "lsp")]
pub mod diagnostics;
#[cfg(feature = "lsp")]
pub mod lsp;
pub mod word_index;

#[derive(Debug)]
pub enum AutoSaveEvent {
    DocumentChanged { save_after: u64 },
    LeftInsertMode,
}

pub struct Handlers {
    // only public because most of the actual implementation is in helix-term right now :/
    #[cfg(feature = "lsp")]
    pub completions: CompletionHandler,
    #[cfg(feature = "lsp")]
    pub signature_hints: Sender<lsp::SignatureHelpEvent>,
    #[cfg(not(target_arch = "wasm32"))]
    pub auto_save: Sender<AutoSaveEvent>,
    #[cfg(feature = "lsp")]
    pub document_colors: Sender<lsp::DocumentColorsEvent>,
    #[cfg(feature = "lsp")]
    pub document_links: Sender<lsp::DocumentLinksEvent>,
    pub word_index: word_index::Handler,
    #[cfg(feature = "lsp")]
    pub pull_diagnostics: Sender<lsp::PullDiagnosticsEvent>,
    #[cfg(feature = "lsp")]
    pub pull_all_documents_diagnostics: Sender<lsp::PullAllDocumentsDiagnosticsEvent>,
    #[cfg(feature = "lsp")]
    pub code_action_hint: Sender<lsp::CodeActionHintEvent>,
}

impl Handlers {
    /// Manually trigger completion (c-x)
    #[cfg(feature = "lsp")]
    pub fn trigger_completions(&self, trigger_pos: usize, doc: DocumentId, view: ViewId) {
        self.completions.event(CompletionEvent::ManualTrigger {
            cursor: trigger_pos,
            doc,
            view,
        });
    }

    #[cfg(feature = "lsp")]
    pub fn trigger_signature_help(&self, invocation: SignatureHelpInvoked, editor: &Editor) {
        let event = match invocation {
            SignatureHelpInvoked::Automatic => {
                if !editor.config().lsp.auto_signature_help {
                    return;
                }
                lsp::SignatureHelpEvent::Trigger
            }
            SignatureHelpInvoked::Manual => lsp::SignatureHelpEvent::Invoked,
        };
        send_blocking(&self.signature_hints, event)
    }

    pub fn word_index(&self) -> &word_index::WordIndex {
        &self.word_index.index
    }
}

pub fn register_hooks(handlers: &Handlers) {
    #[cfg(feature = "lsp")]
    lsp::register_hooks(handlers);
    word_index::register_hooks(handlers);
}
