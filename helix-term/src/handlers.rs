use std::sync::Arc;

use arc_swap::ArcSwap;
#[cfg(feature = "lsp")]
use diagnostics::PullAllDocumentsDiagnosticHandler;
#[cfg(any(feature = "lsp", not(target_arch = "wasm32")))]
use helix_event::AsyncHook;

use crate::config::Config;
use crate::events;
#[cfg(not(target_arch = "wasm32"))]
use crate::handlers::auto_save::AutoSaveHandler;
#[cfg(feature = "lsp")]
use crate::handlers::diagnostics::PullDiagnosticsHandler;
#[cfg(feature = "lsp")]
use crate::handlers::signature_help::SignatureHelpHandler;

pub use helix_view::handlers::{word_index, Handlers};

#[cfg(feature = "lsp")]
use self::document_colors::DocumentColorsHandler;
#[cfg(feature = "lsp")]
use self::document_links::DocumentLinksHandler;

#[cfg(not(target_arch = "wasm32"))]
mod auto_save;
#[cfg(feature = "lsp")]
mod code_action_hint;
#[cfg(feature = "lsp")]
pub mod completion;
#[cfg(feature = "lsp")]
pub mod diagnostics;
#[cfg(feature = "lsp")]
mod document_colors;
#[cfg(feature = "lsp")]
mod document_highlight;
#[cfg(feature = "lsp")]
mod document_links;
mod prompt;
#[cfg(feature = "lsp")]
mod signature_help;
mod snippet;
mod workspace_trust;

pub fn setup(config: Arc<ArcSwap<Config>>) -> Handlers {
    events::register();

    #[cfg(feature = "lsp")]
    let event_tx = completion::CompletionHandler::new(config).spawn();
    #[cfg(feature = "lsp")]
    let signature_hints = SignatureHelpHandler::new().spawn();
    #[cfg(not(target_arch = "wasm32"))]
    let auto_save = AutoSaveHandler::new().spawn();
    #[cfg(feature = "lsp")]
    let code_action_hint = code_action_hint::Handler::default().spawn();
    #[cfg(feature = "lsp")]
    let document_colors = DocumentColorsHandler::default().spawn();
    #[cfg(feature = "lsp")]
    let document_links = DocumentLinksHandler::default().spawn();
    let word_index = word_index::Handler::spawn();
    #[cfg(feature = "lsp")]
    let pull_diagnostics = PullDiagnosticsHandler::default().spawn();
    #[cfg(feature = "lsp")]
    let pull_all_documents_diagnostics = PullAllDocumentsDiagnosticHandler::default().spawn();
    #[cfg(not(feature = "lsp"))]
    let _ = config;

    let handlers = Handlers {
        #[cfg(feature = "lsp")]
        completions: helix_view::handlers::completion::CompletionHandler::new(event_tx),
        #[cfg(feature = "lsp")]
        signature_hints,
        #[cfg(not(target_arch = "wasm32"))]
        auto_save,
        #[cfg(feature = "lsp")]
        document_colors,
        #[cfg(feature = "lsp")]
        document_links,
        word_index,
        #[cfg(feature = "lsp")]
        pull_diagnostics,
        #[cfg(feature = "lsp")]
        pull_all_documents_diagnostics,
        #[cfg(feature = "lsp")]
        code_action_hint,
    };

    helix_view::handlers::register_hooks(&handlers);
    #[cfg(feature = "lsp")]
    {
        completion::register_hooks(&handlers);
        signature_help::register_hooks(&handlers);
        document_highlight::register_hooks(&handlers);
        code_action_hint::register_hooks(&handlers);
        diagnostics::register_hooks(&handlers);
        document_colors::register_hooks(&handlers);
        document_links::register_hooks(&handlers);
    }
    #[cfg(not(target_arch = "wasm32"))]
    auto_save::register_hooks(&handlers);
    snippet::register_hooks(&handlers);
    prompt::register_hooks(&handlers);
    workspace_trust::register_hooks(&handlers);
    handlers
}
