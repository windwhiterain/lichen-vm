//! The stdio LSP server, generic over the compiled program `P`.
//!
//! This is the transport half of the editor tooling: a thin
//! [`tower_lsp::LanguageServer`] over the shared [`Doc`](crate::analysis::Doc)
//! frontend.  It is generic over the program collector `P`, so the same server
//! serves the shipping vocabulary and a plugin-composed one — the package
//! manager builds a plugin-built `lichen-language-server` whose `main` calls
//! [`main`] with its composed `LangProgram`, and the LSP then understands the
//! plugin's leaves for diagnostics / hover / go-to-definition.
//!
//! `Doc` is deliberately **not** held here: it transitively owns raw pointers
//! into the frontend arena (via the diagnostic type), so it is `!Send`, which
//! `tower-lsp` (whose `LanguageServer` is `Send + Sync`) cannot store.  Instead
//! we keep the source text and re-run the frontend on demand in a blocking task,
//! extracting only the `Send` results (diagnostics / hover / definition), just
//! as the non-generic server did.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use lichen_compute::{ComputeOperator, ComputeValue};
use lichen_highlevel::program::{TypeOperator, ValueType};
use lichen_language::LangProgramShape;
use lichen_language::program::GcdOp;
use lichen_utils::extend::AsEnum;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use crate::analysis::Doc;
use crate::lsp::semantic_token_legend;

/// The server state: one source buffer per open document URI.
///
/// [`Doc`] is `!Send` (see the module docs), so only the source texts are held;
/// the frontend is re-run per request in a blocking task.
pub struct Backend<P: LangProgramShape> {
    client: Client,
    sources: Mutex<HashMap<Url, String>>,
    // `fn() -> P` keeps the type parameter without requiring `P: Send + Sync`
    // (the composed value/operator leaves carry raw arena pointers, so `P`
    // itself is not `Send`); a function-pointer phantom is always `Send + Sync`.
    _program: std::marker::PhantomData<fn() -> P>,
}

impl<P> Backend<P>
where
    P: LangProgramShape,
    P::Value: ValueType + AsEnum<ComputeValue> + From<ComputeValue> + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + From<ComputeOperator> + 'static,
{
    fn new(client: Client) -> Self {
        Backend {
            client,
            sources: Mutex::new(HashMap::new()),
            _program: std::marker::PhantomData,
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
            Doc::<P>::new_with_base(text, base.as_deref()).lsp_diagnostics()
        })
        .await
        .expect("compile lichen source")
    }

    /// Store the new source for `uri` and publish its diagnostics.
    async fn update_document(&self, uri: Url, text: String) {
        let base = Self::uri_base(&uri);
        self.sources
            .lock()
            .unwrap()
            .insert(uri.clone(), text.clone());
        let diagnostics = Self::compile_diagnostics(text, base).await;
        self.publish(uri, diagnostics, None).await;
    }

    async fn publish(&self, uri: Url, diagnostics: Vec<Diagnostic>, version: Option<i32>) {
        self.client
            .publish_diagnostics(uri, diagnostics, version)
            .await;
    }
}

#[tower_lsp::async_trait]
impl<P> LanguageServer for Backend<P>
where
    P: LangProgramShape,
    P::Value: ValueType + AsEnum<ComputeValue> + From<ComputeValue> + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + From<ComputeOperator> + 'static,
{
    async fn initialize(&self, _params: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                // Complete the names in scope at the cursor (the same scope set
                // that powers an unresolved name's "did you mean" candidates).
                completion_provider: Some(CompletionOptions::default()),
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
            Doc::<P>::new_with_base(text, base.as_deref()).hover_at(position)
        })
        .await
        .expect("compile lichen source");
        let hover = result.map(|(contents, range)| Hover {
            contents: HoverContents::Scalar(MarkedString::String(contents)),
            range: Some(range),
        });
        Ok(hover)
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let Some(text) = self.sources.lock().unwrap().get(&uri).cloned() else {
            return Ok(None);
        };
        let base = Self::uri_base(&uri);
        let range = tokio::task::spawn_blocking(move || {
            Doc::<P>::new_with_base(text, base.as_deref()).definition_at(position)
        })
        .await
        .expect("compile lichen source");
        let response = range.map(|range| {
            GotoDefinitionResponse::Scalar(Location {
                uri: uri.clone(),
                range,
            })
        });
        Ok(response)
    }

    /// Complete the names in scope at the cursor — the same scope knowledge
    /// that names an unresolved name's "did you mean" candidates in a
    /// diagnostic.
    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let Some(text) = self.sources.lock().unwrap().get(&uri).cloned() else {
            return Ok(None);
        };
        let base = Self::uri_base(&uri);
        let items = tokio::task::spawn_blocking(move || {
            Doc::<P>::new_with_base(text, base.as_deref()).completion_at(position)
        })
        .await
        .expect("compile lichen source");
        Ok(Some(items.into()))
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let uri = params.text_document.uri;
        let Some(text) = self.sources.lock().unwrap().get(&uri).cloned() else {
            return Ok(None);
        };
        // The frontend classifies every token into a `SemanticTokens` payload
        // (delta-encoded, with the legend indices).  `Doc` is `!Send`, so the
        // `spawn_blocking` returns the fully-encoded (Send) exchange object.
        let base = Self::uri_base(&uri);
        let tokens = tokio::task::spawn_blocking(move || {
            Doc::<P>::new_with_base(text, base.as_deref()).semantic_tokens_lsp()
        })
        .await
        .expect("compile lichen source");
        Ok(Some(SemanticTokensResult::Tokens(tokens)))
    }
}

/// Run the stdio LSP server for the composed program `P` (shipping or
/// plugin-built).  This builds the runtime itself so the generated crate's
/// `main` needs no `#[tokio::main]`; the frontend is synchronous per document,
/// so requests are serialized via `concurrency_level(1)`.
pub fn main<P>()
where
    P: LangProgramShape,
    P::Value: ValueType + AsEnum<ComputeValue> + From<ComputeValue> + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + From<ComputeOperator> + 'static,
{
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("cannot build the LSP tokio runtime");
    rt.block_on(async move {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();

        let (service, socket) = LspService::new(Backend::<P>::new);
        Server::new(stdin, stdout, socket)
            .concurrency_level(1)
            .serve(service)
            .await;
    });
}
