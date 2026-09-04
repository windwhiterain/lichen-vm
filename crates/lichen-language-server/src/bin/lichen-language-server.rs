//! The `lichen-language-server`: an LSP server for Lichen over stdio.
//!
//! It is a thin [`tower_lsp::LanguageServer`] over the tooling library. All of
//! the actual editor behavior — span↔position conversion, name resolution, the
//! shared frontend artefacts — lives in `lichen-language-server`'s library, so
//! the same behavior is available to other editors (via the Zed extension
//! importing the same library) without duplicating it here. `tower-lsp` supplies
//! the JSON-RPC framing, request dispatch, cancellation and error handling; this
//! binary only decides how to answer each editor request.
//!
//! Run it directly, or install it as the LSP binary that a Zed extension launches:
//!
//! ```text
//! cargo run -p lichen-language-server --bin lichen-language-server
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use lichen_language_server::analysis::Doc;
use lichen_language_server::lsp::semantic_token_legend;

/// The server state: one source buffer per open document URI.
///
/// [`Doc`] is deliberately **not** held here: it transitively owns raw pointers
/// into the frontend arena (via the diagnostic type), so it is `!Send`, which
/// `tower-lsp` (whose `LanguageServer` is `Send + Sync`) cannot store. Instead
/// we keep the source text and re-run the frontend on demand in a blocking task,
/// extracting only the `Send` results (diagnostics / hover / definition). Making
/// the frontend artefacts `Send` (a follow-up) would let us cache `Doc` here.
struct Backend {
    client: Client,
    sources: Mutex<HashMap<Url, String>>,
}

impl Backend {
    fn new(client: Client) -> Self {
        Backend {
            client,
            sources: Mutex::new(HashMap::new()),
        }
    }

    /// The file-system path a `file://` document URI points at, so relative
    /// `@import` paths resolve against the file's directory.  `None` for a
    /// non-file URI (an unsaved / untitled buffer) — the fallback behaviour is
    /// to resolve imports against the current directory.
    fn uri_base(uri: &Url) -> Option<PathBuf> {
        uri.to_file_path().ok()
    }

    /// Re-parse + check `text` off the async runtime and return the resulting
    /// LSP diagnostics (a `Send` value).  `base` is the file's path (from the
    /// document URI) used to resolve relative `@import` lines.
    async fn compile_diagnostics(text: String, base: Option<PathBuf>) -> Vec<Diagnostic> {
        tokio::task::spawn_blocking(move || {
            Doc::new_with_base(text, base.as_deref()).lsp_diagnostics()
        })
        .await
        .expect("compile lichen source")
    }

    /// Store the new source for `uri` and publish its diagnostics.
    async fn update_document(&self, uri: Url, text: String) {
        let base = Self::uri_base(&uri);
        self.sources.lock().unwrap().insert(uri.clone(), text.clone());
        let diagnostics = Self::compile_diagnostics(text, base).await;
        self.publish(uri, diagnostics, None).await;
    }

    async fn publish(&self, uri: Url, diagnostics: Vec<Diagnostic>, version: Option<i32>) {
        self.client.publish_diagnostics(uri, diagnostics, version).await;
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _params: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                // Lichen's own parser drives highlighting, so Zed can run with
                // (or without) the tree-sitter grammar — the semantic tokens
                // carry the color when the grammar is absent.
                semantic_tokens_provider: Some(
                    SemanticTokensOptions {
                        legend: semantic_token_legend(),
                        range: None,
                        full: Some(SemanticTokensFullOptions::Bool(true)),
                        ..Default::default()
                    }
                    .into(),
                ),
                ..Default::default()
            },
            server_info: None,
        })
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = params.text_document.text;
        self.update_document(uri, text).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        // Full document sync: the last content change replaces the whole text.
        let Some(last) = params.content_changes.into_iter().last() else {
            return;
        };
        self.update_document(uri, last.text).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.sources.lock().unwrap().remove(&uri);
        // Clear the now-stale diagnostics for the closed document.
        self.publish(uri, Vec::new(), None).await;
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let Some(text) = self.sources.lock().unwrap().get(&uri).cloned() else {
            return Ok(None);
        };
        let base = Self::uri_base(&uri);
        let result = tokio::task::spawn_blocking(move || {
            Doc::new_with_base(text, base.as_deref()).hover_at(position)
        })
        .await
        .expect("compile lichen source");
        let hover = result.map(|(contents, range)| Hover {
            contents: HoverContents::Scalar(MarkedString::String(contents)),
            range: Some(range),
        });
        Ok(hover)
    }

    async fn goto_definition(&self, params: GotoDefinitionParams) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let Some(text) = self.sources.lock().unwrap().get(&uri).cloned() else {
            return Ok(None);
        };
        let base = Self::uri_base(&uri);
        let range = tokio::task::spawn_blocking(move || {
            Doc::new_with_base(text, base.as_deref()).definition_at(position)
        })
        .await
        .expect("compile lichen source");
        let response = range.map(|range| GotoDefinitionResponse::Scalar(Location {
            uri: uri.clone(),
            range,
        }));
        Ok(response)
    }

    async fn semantic_tokens_full(&self, params: SemanticTokensParams) -> Result<Option<SemanticTokensResult>> {
        let uri = params.text_document.uri;
        let Some(text) = self.sources.lock().unwrap().get(&uri).cloned() else {
            return Ok(None);
        };
        // The frontend classifies every token into a `SemanticTokens` payload
        // (delta-encoded, with the legend indices).  `Doc` is `!Send`, so the
        // `spawn_blocking` returns the fully-encoded (Send) exchange object.
        let base = Self::uri_base(&uri);
        let tokens = tokio::task::spawn_blocking(move || {
            Doc::new_with_base(text, base.as_deref()).semantic_tokens_lsp()
        })
        .await
        .expect("compile lichen source");
        Ok(Some(SemanticTokensResult::Tokens(tokens)))
    }
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket)
        // The frontend is synchronous per document; serialize requests so
        // document state updates and reads stay ordered.
        .concurrency_level(1)
        .serve(service)
        .await;
}
