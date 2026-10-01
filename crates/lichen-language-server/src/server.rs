//! The stdio LSP server, generic over the compiled program `P`.
//!
//! This is the transport half of the editor tooling: a thin
//! [`tower_lsp::LanguageServer`] over the shared frontend ([`Doc`] /
//! [`DocIndex`](crate::analysis::DocIndex)).  It is generic over the program
//! collector `P`, so the same server serves the shipping vocabulary and a
//! plugin-composed one — the package manager builds a plugin-built
//! `lichen-language-server` whose `main` calls [`main`] with its composed
//! `LangProgram`, and the LSP then understands the plugin's leaves for
//! diagnostics / hover / go-to-definition.
//!
//! `Doc` is deliberately **not** held here: it transitively owns raw pointers
//! into the frontend arena (via the checker's diagnostic type), so it is
//! `!Send`, which `tower-lsp` (whose `LanguageServer` is `Send + Sync`) cannot
//! store.  The *extracted indexes* are `Send`, so what the server holds per
//! document is a [`DocIndex`](crate::analysis::DocIndex): one full frontend run
//! per source text, shared by every hover / definition / completion / semantic
//! tokens request that text receives.
//!
//! Two mechanisms keep a burst of keystrokes from starting one full frontend
//! run per keystroke (`docs/notes/code-audit.md`, `P1-17`):
//!
//! - a **debounce**: an edit is analyzed only after [`EDIT_DEBOUNCE`] without a
//!   newer edit, so a burst collapses to its last text;
//! - a **generation gate**: an analysis (however it was scheduled) runs and
//!   publishes only while the text it was launched for is still the document's
//!   current text, so a superseded analysis stops instead of completing.
//!
//! Neither can abort a frontend run that has already started: `tower-lsp`
//! answers `$/cancelRequest` by dropping the request future, and a
//! `tokio::task::spawn_blocking` closure is detached from its join handle, so
//! the run finishes and its result is discarded.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lichen_compute::{ComputeOperator, ComputeValue};
use lichen_highlevel::program::{TypeOperator, ValueType};
use lichen_language::LangProgramShape;
use lichen_language::diag::Stage;
use lichen_language::package::PackageStore;
use lichen_language::persist::{ArtifactCodec, DeviceRegistry, Hash, sha256};
use lichen_language::program::GcdOp;
use lichen_utils::extend::AsEnum;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use crate::analysis::{Doc, DocIndex};
use crate::home::LichenHome;
use crate::lsp::semantic_token_legend;

/// How long an edit waits for the next one before its document is analyzed.
/// A burst of keystrokes collapses to its last edit — one frontend run — rather
/// than one run per keystroke.
const EDIT_DEBOUNCE: Duration = Duration::from_millis(150);

/// The server state: one open document per URI, and the index of the text that
/// document currently holds.
///
/// [`Doc`] is `!Send` (see the module docs), so it is built and dropped inside
/// the analysis; the `Send` index it produced is what is kept.
pub struct Backend<P: LangProgramShape> {
    inner: Arc<Inner<P>>,
}

struct Inner<P: LangProgramShape> {
    client: Client,
    /// The open documents: their current text, that text's hash (the identity a
    /// cache entry is keyed by) and its generation (which edits supersede).
    documents: Mutex<HashMap<Url, Document>>,
    /// The last analysis per open document.  One entry per document, so the
    /// cache is bounded by the number of open documents: a keystroke replaces
    /// the entry rather than adding one.
    indexes: Mutex<HashMap<Url, CachedIndex>>,
    home: Arc<LichenHome>,
    /// The device registry this server keeps open across requests.  A run takes
    /// it, so two overlapping runs cannot share one handle (the second opens a
    /// fresh registry, exactly as every request did before); the run returns it
    /// for the next one.  See [`PackageStore::with_device`].
    device: Mutex<Option<DeviceRegistry>>,
    /// The only way a generation is minted: strictly increasing, never reused,
    /// so an analysis of a closed-then-reopened document can never match the
    /// generation of an older analysis of the same URI.
    generations: AtomicU64,
    // `fn() -> P` keeps the type parameter without requiring `P: Send + Sync`
    // (the composed value/operator leaves carry raw arena pointers, so `P`
    // itself is not `Send`); a function-pointer phantom is always `Send + Sync`.
    _program: std::marker::PhantomData<fn() -> P>,
}

/// One open document: its current text plus the identity and generation of that
/// text.
struct Document {
    text: String,
    /// The sha256 of `text`.  The client's document version is not tracked (the
    /// server neither receives it into the cache nor publishes it), and a
    /// version is only as injective as the client; the text's own hash is
    /// injective by construction, so it is what a cache entry is keyed by.
    hash: Hash,
    generation: u64,
}

/// The analysis of one document's current text.
struct CachedIndex {
    /// The hash of the text this index was built from.
    text: Hash,
    /// Every imported package file the run loaded, with the sha256 of its bytes
    /// at analysis time.  A hit requires all of them to be unchanged: the
    /// index's answers depend on the imported modules, and caches on disk are
    /// shared with other processes (`docs/notes/artifact-cache.md`).
    dependencies: Vec<(PathBuf, Hash)>,
    index: Arc<DocIndex>,
}

impl<P> Inner<P>
where
    P: LangProgramShape,
    P::Value: ValueType + AsEnum<ComputeValue> + From<ComputeValue> + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + From<ComputeOperator> + 'static,
{
    /// The document's current text with its hash and generation.
    fn current(&self, uri: &Url) -> Option<(String, Hash, u64)> {
        self.documents
            .lock()
            .unwrap()
            .get(uri)
            .map(|document| (document.text.clone(), document.hash, document.generation))
    }

    /// Record `text` as the document's current text and return the generation
    /// that text is, superseding every older one.
    fn set_text(&self, uri: Url, text: String) -> u64 {
        let generation = self.generations.fetch_add(1, Ordering::Relaxed) + 1;
        let hash = sha256(text.as_bytes());
        self.documents.lock().unwrap().insert(
            uri,
            Document {
                text,
                hash,
                generation,
            },
        );
        generation
    }

    /// Whether `generation` is still the document's current one.
    fn is_current(&self, uri: &Url, generation: u64) -> bool {
        self.documents
            .lock()
            .unwrap()
            .get(uri)
            .is_some_and(|document| document.generation == generation)
    }

    /// The analysis of `text` for `uri`: the cached index when the cache holds
    /// this text with every dependency unchanged, else one full frontend run.
    async fn index_for(&self, uri: &Url, text: String, hash: Hash) -> Arc<DocIndex> {
        if let Some(index) = self.cached_index(uri, &hash) {
            return index;
        }
        let (index, dependencies) = self.run_frontend(uri, text).await;
        match dependencies {
            Some(dependencies) => {
                // Only the document's own current text is kept: a run whose text
                // an edit has already superseded would overwrite a newer entry
                // and cost the next request a run it did not need.
                let documents = self.documents.lock().unwrap();
                let current = documents.get(uri);
                let mut indexes = self.indexes.lock().unwrap();
                if current.is_some_and(|document| document.hash == hash) {
                    indexes.insert(
                        uri.clone(),
                        CachedIndex {
                            text: hash,
                            dependencies,
                            index: Arc::clone(&index),
                        },
                    );
                }
            }
            // Not cacheable (see `imported_files` / `cacheable`): the next
            // request for this text runs the frontend again, as it does today.
            None => {}
        }
        index
    }

    /// The cached index for `uri` when it holds `hash` and every dependency it
    /// was built from still has the bytes it was built from.
    fn cached_index(&self, uri: &Url, hash: &Hash) -> Option<Arc<DocIndex>> {
        let (index, dependencies) = {
            let indexes = self.indexes.lock().unwrap();
            let entry = indexes.get(uri)?;
            if &entry.text != hash {
                return None;
            }
            (Arc::clone(&entry.index), entry.dependencies.clone())
        };
        if dependencies_unchanged(&dependencies) {
            return Some(index);
        }
        // A dependency moved under this entry: drop it rather than answer from
        // an analysis the next request would redo anyway.
        let mut indexes = self.indexes.lock().unwrap();
        if indexes.get(uri).is_some_and(|entry| &entry.text == hash) {
            indexes.remove(uri);
        }
        None
    }

    /// Run the frontend for `text` off the async runtime, reusing this server's
    /// device registry, and return the index plus the imported files it was
    /// built from (`None` when the analysis must not be cached).
    async fn run_frontend(
        &self,
        uri: &Url,
        text: String,
    ) -> (Arc<DocIndex>, Option<Vec<(PathBuf, Hash)>>) {
        let base = uri.to_file_path().ok();
        let cache_root = self.home.cache_root().to_path_buf();
        let device = self.device.lock().unwrap().take();
        let (index, dependencies, device) = tokio::task::spawn_blocking(move || {
            let mut store = open_store::<P>(device, cache_root);
            let doc = Doc::<P>::new_with_store(text, base.as_deref(), &mut store);
            let index = doc.index();
            let dependencies = cacheable(&doc).then(|| imported_files(&store)).flatten();
            let device = store.into_device();
            (index, dependencies, device)
        })
        .await
        .expect("compile lichen source");
        let mut pool = self.device.lock().unwrap();
        if pool.is_none() {
            *pool = device;
        }
        (index, dependencies)
    }

    /// Analyze the document's current text and publish its diagnostics, unless
    /// a newer edit superseded this analysis while the frontend ran.
    async fn analyze_and_publish(self: &Arc<Self>, uri: Url, generation: u64) {
        let Some((text, hash, current)) = self.current(&uri) else {
            return;
        };
        if current != generation {
            return;
        }
        let index = self.index_for(&uri, text, hash).await;
        if !self.is_current(&uri, generation) {
            return;
        }
        self.client
            .publish_diagnostics(uri, index.lsp_diagnostics(), None)
            .await;
    }
}

impl<P> Backend<P>
where
    P: LangProgramShape,
    P::Value: ValueType + AsEnum<ComputeValue> + From<ComputeValue> + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + From<ComputeOperator> + 'static,
{
    fn new(client: Client, cache_root: PathBuf) -> Self {
        // `cache_root` is this vocabulary's `compilers/<plugin-set-key>` slot
        // (the shipping server uses the empty plugin set's slot).  It is the
        // root the reused device registry is opened at, so the settled imported
        // packages are cached on disk and shared cross-process with the
        // `lichen` compiler.  See `docs/notes/liche-lsp-home.md`.
        let home = Arc::new(LichenHome::at(cache_root));
        home.ensure();
        Backend {
            inner: Arc::new(Inner {
                client,
                documents: Mutex::new(HashMap::new()),
                indexes: Mutex::new(HashMap::new()),
                home,
                device: Mutex::new(None),
                generations: AtomicU64::new(0),
                _program: std::marker::PhantomData,
            }),
        }
    }

    /// Record `text` as the document's current text and analyze it after
    /// `delay`, publishing its diagnostics.
    ///
    /// The debounce is a **spawned** task, never a wait inside the handler:
    /// under `concurrency_level(1)` a wait here would serialize the queued
    /// edits and collapse nothing, while a spawned task lets the transport keep
    /// consuming messages (and shutdown keep working) during the wait.
    fn schedule_analysis(&self, uri: Url, text: String, delay: Duration) {
        let inner = Arc::clone(&self.inner);
        let generation = inner.set_text(uri.clone(), text);
        tokio::spawn(async move {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            // A superseded edit stops here, before the frontend runs at all.
            if !inner.is_current(&uri, generation) {
                return;
            }
            inner.analyze_and_publish(uri, generation).await;
        });
    }
}

/// The package store for one frontend run: the device registry this server
/// already has open (`None` on the first run, or when a concurrent run holds
/// it), else a fresh open.  An in-memory store is used only when it is
/// *intended* — the program's codec cannot persist (`NoPersist`), the same rule
/// [`Doc::new_with_cache`] applies.
fn open_store<P>(device: Option<DeviceRegistry>, cache_root: PathBuf) -> PackageStore<P>
where
    P: LangProgramShape,
    P::Value: ValueType + AsEnum<ComputeValue> + From<ComputeValue> + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + From<ComputeOperator> + 'static,
{
    if !P::Codec::PERSISTENT {
        return PackageStore::new();
    }
    match device {
        Some(device) => PackageStore::with_device(cache_root, device),
        None => PackageStore::with_cache_dir(cache_root),
    }
}

/// Whether an analysis may be cached.  A run that failed to resolve or read a
/// package is not cacheable: no set of *existing* files names the one whose
/// appearance or repair would change the answer, so a hit could keep reporting
/// a failure the editor has already fixed.
fn cacheable<P: LangProgramShape>(doc: &Doc<P>) -> bool {
    !doc.diagnostics
        .iter()
        .any(|d| matches!(d.stage, Stage::Preprocess | Stage::Io))
}

/// The imported package files a run loaded, with the sha256 of each file's
/// bytes — the dependency half of an analysis's cache key.  `None` when a
/// loaded package's bytes cannot be read back, which makes the analysis
/// uncacheable: the key would no longer cover that file.
///
/// The store loaded exactly the document's import closure (transitively), so
/// these are the files whose contents can change an answer for it.  A native
/// package (`compute.lichen`) is compiled from source embedded in this binary
/// and is never on disk, so it is deliberately absent: it cannot change under a
/// cache root.
fn imported_files<P>(store: &PackageStore<P>) -> Option<Vec<(PathBuf, Hash)>>
where
    P: LangProgramShape,
{
    let mut dependencies = Vec::with_capacity(store.packages.len());
    for path in store.packages.keys() {
        let bytes = std::fs::read(path).ok()?;
        dependencies.push((path.clone(), sha256(&bytes)));
    }
    dependencies.sort();
    Some(dependencies)
}

/// Whether every recorded dependency still has the bytes the analysis was built
/// from.  A missing or unreadable file is a miss like any other: it is a change
/// in what the frontend would read.
fn dependencies_unchanged(dependencies: &[(PathBuf, Hash)]) -> bool {
    dependencies
        .iter()
        .all(|(path, hash)| std::fs::read(path).is_ok_and(|bytes| sha256(&bytes) == *hash))
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
        // A fresh document: its previous text's diagnostics and analysis are
        // not this document's.  The spawn is immediate (no edit delay).
        self.inner.indexes.lock().unwrap().remove(&uri);
        self.schedule_analysis(uri, text, Duration::ZERO);
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        // Full document sync: the last content change replaces the whole text.
        let Some(last) = params.content_changes.into_iter().last() else {
            return;
        };
        self.schedule_analysis(uri, last.text, EDIT_DEBOUNCE);
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        // Dropping the document makes every in-flight analysis of it
        // superseded, so none of them publishes after the close.
        self.inner.documents.lock().unwrap().remove(&uri);
        self.inner.indexes.lock().unwrap().remove(&uri);
        // Clear the now-stale diagnostics for the closed document.
        self.inner
            .client
            .publish_diagnostics(uri, Vec::new(), None)
            .await;
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let Some((text, hash, _generation)) = self.inner.current(&uri) else {
            return Ok(None);
        };
        let index = self.inner.index_for(&uri, text, hash).await;
        let hover = index.hover_at(position).map(|(contents, range)| Hover {
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
        let Some((text, hash, _generation)) = self.inner.current(&uri) else {
            return Ok(None);
        };
        let index = self.inner.index_for(&uri, text, hash).await;
        let response = index.definition_at(position).map(|range| {
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
        let Some((text, hash, _generation)) = self.inner.current(&uri) else {
            return Ok(None);
        };
        let index = self.inner.index_for(&uri, text, hash).await;
        Ok(Some(index.completion_at(position).into()))
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let uri = params.text_document.uri;
        let Some((text, hash, _generation)) = self.inner.current(&uri) else {
            return Ok(None);
        };
        // The frontend classifies every token into a `SemanticTokens` payload
        // (delta-encoded, with the legend indices), computed from the cached
        // index — the frontend run and the `!Send` `Doc` are not repeated.
        let index = self.inner.index_for(&uri, text, hash).await;
        Ok(Some(SemanticTokensResult::Tokens(
            index.semantic_tokens_lsp(),
        )))
    }
}

/// Run the stdio LSP server for the composed program `P` (shipping or
/// plugin-built), caching the settled imported packages under `cache_root`
/// (the vocabulary's `compilers/<plugin-set-key>` slot).  This builds the
/// runtime itself so the generated crate's `main` needs no `#[tokio::main]`;
/// requests are serialized via `concurrency_level(1)`, and the analysis a
/// request needs is either cached or run once per document text (see the module
/// docs).
pub fn main<P>(cache_root: &Path)
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

        let cache_root = cache_root.to_path_buf();
        let (service, socket) =
            LspService::new(move |client| Backend::<P>::new(client, cache_root.clone()));
        Server::new(stdin, stdout, socket)
            .concurrency_level(1)
            .serve(service)
            .await;
    });
}
