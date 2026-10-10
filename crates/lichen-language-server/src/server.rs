//! The stdio LSP server, generic over the compiled program `P`.
//!
//! # Invariant
//! `Doc` is `!Send` — it owns pointers into the frontend arena — so a document is
//! held as its `Send` [`DocIndex`](crate::analysis::DocIndex), and the `!Send`
//! session must outlive its request, which puts it on a dedicated compile
//! worker thread (`docs/notes/incremental-update.md` §6.4,
//! `docs/notes/liche-lsp-home.md` §3).

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use lichen_compute::{ComputeOperator, ComputeValue};
use lichen_highlevel::program::{TypeOperator, ValueType};
use lichen_language::LangProgramShape;
use lichen_language::diag::{Diag, Stage};
use lichen_language::lex;
use lichen_language::package::PackageStore;
use lichen_language::persist::{ArtifactCodec, DeviceRegistry, Hash, sha256};
use lichen_language::preprocess;
use lichen_language::program::GcdOp;
use lichen_language::session::{BufferSession, CellEvents};
use lichen_lowlevel::Registry;
use lichen_utils::extend::AsEnum;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use crate::analysis::{Artifacts, DocIndex, index};
use crate::home::LichenHome;
use crate::lsp::semantic_token_legend;

/// How long an edit waits for the next one, so a burst collapses to one run.
const EDIT_DEBOUNCE: Duration = Duration::from_millis(150);

/// The server state: one open document per URI, and the index of the text that
/// document currently holds.
///
/// # Invariant
/// [`Doc`] is `!Send`, so it is built and dropped inside the analysis; only the
/// `Send` index it produced is kept.
pub struct Backend<P: LangProgramShape> {
    inner: Arc<Inner<P>>,
}

struct Inner<P: LangProgramShape> {
    client: Client,
    /// The open documents: their text, that text's hash (a cache key) and
    /// its generation (which edits supersede).
    documents: Mutex<HashMap<Url, Document>>,
    /// The last analysis per open document: the cache is bounded by them.
    /// A keystroke replaces an entry, never adds one.
    indexes: Mutex<HashMap<Url, CachedIndex>>,
    /// The **other files** each document's last analysis published for
    /// (`docs/notes/core-prelude.md` §4).
    ///
    /// # Invariant
    /// A client keeps a file's diagnostics until an empty list replaces them, so
    /// a file no longer reported must be published empty, or a fixed error stays
    /// on the built-in's line for the rest of the session.
    published_files: Mutex<HashMap<Url, Vec<Url>>>,
    /// The compile worker: the one thread owning the `!Send` half of the server.
    worker: Worker,
    /// Generations are minted strictly increasing, never reused, so a
    /// reopened document cannot match an older analysis.
    generations: AtomicU64,
    // A function-pointer phantom is `Send + Sync`, so `P` itself need not be:
    // the composed leaves carry raw arena pointers.
    _program: std::marker::PhantomData<fn() -> P>,
}

/// The compile worker: a dedicated thread owning every `!Send` compile artifact.
///
/// # Invariant
/// A session must outlive the request that built it and is `!Send` (it holds the
/// checker's `Build`), so neither this struct nor a `spawn_blocking` closure can
/// hold it — the thread is the boundary. A job goes in, a `Send` [`Analysis`]
/// comes back (`docs/notes/incremental-update.md` §6.4).
struct Worker {
    jobs: tokio::sync::mpsc::UnboundedSender<Job>,
}

/// One unit of work for the compile worker.
enum Job {
    /// Analyze `text` as `uri`'s current content and answer with the index.
    Analyze {
        uri: Url,
        text: String,
        reply: tokio::sync::oneshot::Sender<Analysis>,
    },
    /// Forget `uri`: its session, and the artifacts it retained, are dropped.
    Close { uri: Url },
}

/// What one analysis produced: the `Send` index, and the imported files it read.
struct Analysis {
    index: Arc<DocIndex>,
    dependencies: Option<Vec<(PathBuf, Hash)>>,
    /// What the compile did to the retained cells, and whether it reused the
    /// established build. Pushed as [`AnalysisStats`].
    cells: CellEvents,
    reused_build: bool,
}

/// The `lichen/analysis` telemetry the server pushes to the client.
///
/// # Invariant
/// A caller cannot otherwise tell a rebuild that reused nine cells from one
/// that reused none, and this is that caller; an editor that does not know the
/// method ignores it.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct AnalysisStats {
    uri: Url,
    /// Marked bindings read back from a frozen artifact instead of compiled.
    reused_cells: usize,
    /// Cells this compile froze.
    frozen_cells: usize,
    /// Cells it dropped before compiling: the edit reached them, or the program
    /// no longer marks their position.
    dropped_cells: usize,
    /// Whether the resolved content was unchanged, so the established build was
    /// reused rather than re-lowered and re-checked.
    reused_build: bool,
}

/// The `lichen/analysis` notification method.
enum AnalysisNotification {}

impl tower_lsp::lsp_types::notification::Notification for AnalysisNotification {
    type Params = AnalysisStats;
    const METHOD: &'static str = "lichen/analysis";
}

impl Worker {
    /// Spawn the worker thread; everything it needs is built *on* it, so the
    /// closure captures only `Send` values.
    fn spawn<P>(cache_root: PathBuf) -> Worker
    where
        P: LangProgramShape,
        P::Value: ValueType + AsEnum<ComputeValue> + From<ComputeValue> + 'static,
        P::Operator: From<GcdOp> + From<TypeOperator> + From<ComputeOperator> + 'static,
    {
        let (jobs, mut queued) = tokio::sync::mpsc::unbounded_channel();
        std::thread::Builder::new()
            .name("lichen-lsp-compile".to_string())
            .spawn(move || {
                let mut state = WorkerState::<P>::new(cache_root);
                while let Some(job) = queued.blocking_recv() {
                    state.run(job);
                }
            })
            .expect("spawn the compile worker thread");
        Worker { jobs }
    }

    /// Analyze `text` as `uri`'s content, awaiting the worker's answer.
    async fn analyze(&self, uri: Url, text: String) -> Analysis {
        let (reply, answer) = tokio::sync::oneshot::channel();
        self.jobs
            .send(Job::Analyze { uri, text, reply })
            .expect("the compile worker outlives every request");
        answer.await.expect("the compile worker answers every job")
    }

    /// Forget `uri`'s document — what a close means to the worker.
    fn close(&self, uri: Url) {
        let _ = self.jobs.send(Job::Close { uri });
    }
}

/// The worker thread's state: the device handle, the registry, and one session
/// per open document.
struct WorkerState<P: LangProgramShape>
where
    P::Value: ValueType,
{
    cache_root: PathBuf,
    /// The device handle, taken for one run and handed back.
    ///
    /// # Invariant
    /// A store kept across runs would accumulate every document's imports, so
    /// its `packages` map must stay exactly one document's import closure —
    /// that is what an analysis records as its dependencies.
    device: Option<DeviceRegistry>,
    /// The registry every session's cells and imports are filed in.
    ///
    /// # Invariant
    /// A cell's frozen closure names the imports its value read, so they must be
    /// resolved where the cell is filed ([`BufferSession::with_registry`]).
    registry: Arc<RwLock<Registry<P>>>,
    /// One session per open document, kept across requests: this is what makes
    /// an edit incremental.
    sessions: HashMap<Url, BufferSession<P>>,
    /// Per document: the imported files its retained cells read from
    /// (`None` when a loaded file could not be read back).
    ///
    /// # Invariant
    /// A cell is a *value* computed from those bytes and nothing in the
    /// document's text records them, so a session whose files cannot be proven
    /// unchanged must be dropped. This is the coarse cut; the fine one would
    /// name the cells that read the changed file.
    dependencies: HashMap<Url, Option<Vec<(PathBuf, Hash)>>>,
}

impl<P> WorkerState<P>
where
    P: LangProgramShape,
    P::Value: ValueType + AsEnum<ComputeValue> + From<ComputeValue> + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + From<ComputeOperator> + 'static,
{
    fn new(cache_root: PathBuf) -> Self {
        // A codec that cannot persist has no device; in-memory is the intended mode.
        let device = P::Codec::PERSISTENT.then(|| DeviceRegistry::open(cache_root.clone()));
        WorkerState {
            cache_root,
            device,
            registry: Arc::new(RwLock::new(Registry::new())),
            sessions: HashMap::new(),
            dependencies: HashMap::new(),
        }
    }

    fn run(&mut self, job: Job) {
        match job {
            Job::Analyze { uri, text, reply } => {
                // A panicking compile must not take the worker down: the session is
                // dropped, and the request answered one-shot.
                let answer =
                    std::panic::catch_unwind(AssertUnwindSafe(|| self.analyze(uri.clone(), &text)))
                        .unwrap_or_else(|_| {
                            self.forget(&uri);
                            self.one_shot(uri.clone(), &text)
                        });
                let _ = reply.send(answer);
            }
            Job::Close { uri } => self.forget(&uri),
        }
    }

    /// Drop `uri`'s session, releasing the artifacts it retained.
    ///
    /// # Invariant
    /// The session is the only holder of its reports, so dropping it is
    /// `evict_unreachable`'s caller-side precondition; what eviction still
    /// refuses is the session's debt, and a forgotten session has none to pay.
    ///
    /// [`evict_unreachable`]: BufferSession::evict_unreachable
    fn forget(&mut self, uri: &Url) {
        self.dependencies.remove(uri);
        if let Some(mut session) = self.sessions.remove(uri) {
            session.evict_unreachable();
        }
    }

    /// The incremental analysis: preprocess, hand the session the view,
    /// build the index from what its compile produced.
    fn analyze(&mut self, uri: Url, text: &str) -> Analysis {
        // A session whose imports moved is dropped first: its cells hold stale values.

        // A record that cannot be proven is dropped too: re-derive beats guessing.
        let stale = self.dependencies.get(&uri).is_some_and(|recorded| {
            recorded
                .as_ref()
                .is_none_or(|dependencies| !dependencies_unchanged(dependencies))
        });
        if stale {
            self.forget(&uri);
        }

        let mut store = self.store();
        let base = uri.to_file_path().ok();
        let mut diagnostics = preprocess::stage_depends::<P>(&mut store, text);
        let (pre, preprocess_diagnostics) =
            preprocess::preprocess(text, base.as_deref(), &mut store);
        diagnostics.extend(preprocess_diagnostics);
        let line_starts = lex::line_starts(text);

        let registry = Arc::clone(&self.registry);
        let session = self
            .sessions
            .entry(uri.clone())
            .or_insert_with(|| BufferSession::with_registry("", uri.as_str(), registry));
        session.set_view(pre.code, pre.code_base, &line_starts, &pre.imports);
        let report = session.compile();
        diagnostics.extend(report.diagnostics);

        let artifacts = Artifacts {
            tokens: Arc::clone(&report.tokens),
            program: Arc::clone(&report.program),
            span_index: report.span_index.clone(),
            build: report.build.clone(),
            diagnostics,
        };
        let (index, diagnostics) = index::<P>(text, line_starts, &pre, artifacts, &store);
        let dependencies = imported_files(&store);
        let cacheable = cacheable(&diagnostics);
        let (cells, reused_build) = (report.cells, report.reused);
        self.device = store.into_device();
        self.dependencies.insert(uri, dependencies.clone());
        Analysis {
            index: Arc::new(index),
            dependencies: cacheable.then_some(dependencies).flatten(),
            cells,
            reused_build,
        }
    }

    /// The one-shot analysis: the whole frontend and a check, nothing retained.
    fn one_shot(&mut self, uri: Url, text: &str) -> Analysis {
        let mut store = self.store();
        let base = uri.to_file_path().ok();
        let (index, diagnostics) = crate::analysis::analyze::<P>(text, base.as_deref(), &mut store);
        let dependencies = imported_files(&store);
        let cacheable = cacheable(&diagnostics);
        self.device = store.into_device();
        self.dependencies.insert(uri, dependencies.clone());
        Analysis {
            index: Arc::new(index),
            dependencies: cacheable.then_some(dependencies).flatten(),
            // The one-shot path retains nothing and reuses nothing: there is
            // no session for it to have read a cell from.
            cells: CellEvents::default(),
            reused_build: false,
        }
    }

    /// A store for one run, over the worker's device handle and its registry.
    ///
    /// # Invariant
    /// The registry is the shared one: the imports a store resolves must be
    /// registered where the sessions' cells are filed.
    fn store(&mut self) -> PackageStore<P> {
        let mut store = open_store::<P>(self.device.take(), self.cache_root.clone());
        store.registry = Arc::clone(&self.registry);
        store
    }
}

/// One open document: its current text plus the identity and generation of that
/// text.
struct Document {
    text: String,
    /// The sha256 of `text`: the cache key, since a client version is
    /// only as injective as the client that sends it.
    hash: Hash,
    generation: u64,
}

/// The analysis of one document's current text.
struct CachedIndex {
    /// The hash of the text this index was built from.
    text: Hash,
    /// Imported files with each one's sha256: the dependency half of
    /// the cache key (`docs/notes/artifact-cache.md`).
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

    /// The index for `text`: a hit when the text and every dependency
    /// are unchanged, else one compile on the worker.
    ///
    /// # Invariant
    /// A cache hit compiles nothing, so the returned analysis is `None`.
    async fn index_for(
        &self,
        uri: &Url,
        text: String,
        hash: Hash,
    ) -> (Arc<DocIndex>, Option<Analysis>) {
        if let Some(index) = self.cached_index(uri, &hash) {
            return (index, None);
        }
        let analysis = self.worker.analyze(uri.clone(), text).await;
        if let Some(dependencies) = &analysis.dependencies {
            // Only the document's current text is kept: a superseded run
            // would overwrite a newer entry.
            let documents = self.documents.lock().unwrap();
            let current = documents.get(uri);
            let mut indexes = self.indexes.lock().unwrap();
            if current.is_some_and(|document| document.hash == hash) {
                indexes.insert(
                    uri.clone(),
                    CachedIndex {
                        text: hash,
                        dependencies: dependencies.clone(),
                        index: Arc::clone(&analysis.index),
                    },
                );
            }
        }
        // Not cacheable (see `imported_files` / `cacheable`): the next request
        // for this text compiles again.
        let index = Arc::clone(&analysis.index);
        (index, Some(analysis))
    }

    /// The cached index for `uri` when it holds `hash` and every dependency
    /// still has the bytes it was built from.
    ///
    /// # Invariant
    /// A miss is a miss whether the text moved or a dependency did — the worker
    /// drops the *session* in the second case, which this cache never sees.
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

    /// Analyze the document's current text and publish its diagnostics,
    /// unless a newer edit superseded this analysis.
    ///
    /// # Invariant
    /// An analysis publishes only while the text it was launched for is still
    /// current, so a superseded one stops instead of resurrecting a stale error.
    async fn analyze_and_publish(self: &Arc<Self>, uri: Url, generation: u64) {
        let Some((text, hash, current)) = self.current(&uri) else {
            return;
        };
        if current != generation {
            return;
        }
        let (index, analysis) = self.index_for(&uri, text, hash).await;
        if !self.is_current(&uri, generation) {
            return;
        }
        self.client
            .publish_diagnostics(uri.clone(), index.lsp_diagnostics(), None)
            .await;
        // Published against the built-in's own file, not the document:
        // the store's file (`docs/notes/core-prelude.md` §4).
        let mut files: Vec<Url> = Vec::new();
        for file in index.file_diagnostics() {
            let Ok(file_uri) = Url::from_file_path(&file.path) else {
                // A path the URL grammar cannot name (a virtual package with no
                // materialized file): nothing to publish it against.
                continue;
            };
            self.client
                .publish_diagnostics(file_uri.clone(), file.diagnostics.clone(), None)
                .await;
            files.push(file_uri);
        }
        // Clear the files no longer reported: a client keeps a file's
        // diagnostics until an empty list replaces them.
        let stale = self
            .published_files
            .lock()
            .unwrap()
            .insert(uri.clone(), files.clone())
            .unwrap_or_default();
        for file_uri in stale {
            if !files.contains(&file_uri) {
                self.client
                    .publish_diagnostics(file_uri, Vec::new(), None)
                    .await;
            }
        }
        // The compile's own event surface: a caller that cannot see it
        // cannot tell an incremental analysis from a full one.
        if let Some(analysis) = analysis {
            let _ = self
                .client
                .send_notification::<AnalysisNotification>(AnalysisStats {
                    uri,
                    reused_cells: analysis.cells.reused,
                    frozen_cells: analysis.cells.frozen,
                    dropped_cells: analysis.cells.dropped,
                    reused_build: analysis.reused_build,
                })
                .await;
        }
    }
}

impl<P> Backend<P>
where
    P: LangProgramShape,
    P::Value: ValueType + AsEnum<ComputeValue> + From<ComputeValue> + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + From<ComputeOperator> + 'static,
{
    fn new(client: Client, cache_root: PathBuf) -> Self {
        // This vocabulary's `compilers/<plugin-set-key>` slot, opened on
        // the compile worker: `docs/notes/liche-lsp-home.md` §3.
        let home = LichenHome::at(cache_root);
        home.ensure();
        Backend {
            inner: Arc::new(Inner {
                client,
                documents: Mutex::new(HashMap::new()),
                indexes: Mutex::new(HashMap::new()),
                published_files: Mutex::new(HashMap::new()),
                worker: Worker::spawn::<P>(home.cache_root().to_path_buf()),
                generations: AtomicU64::new(0),
                _program: std::marker::PhantomData,
            }),
        }
    }

    /// Record `text` as the document's current text and analyze it after
    /// `delay`, publishing its diagnostics.
    ///
    /// # Invariant
    /// The debounce is a **spawned** task, never a wait inside the handler: under
    /// `concurrency_level(1)` a wait would serialize the queued edits and
    /// collapse nothing.
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

/// The package store for one run: the device handle the worker keeps open,
/// else a fresh open.
///
/// # Invariant
/// In-memory only when intended — the codec cannot persist — the same rule
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

/// Whether an analysis may be cached.
///
/// # Invariant
/// A run that failed to resolve or read a package is not cacheable: no set of
/// *existing* files names the one whose appearance or repair would change the
/// answer, so a hit could keep reporting a failure the editor already fixed.
fn cacheable<P: LangProgramShape>(diagnostics: &[Diag<P>]) -> bool {
    !diagnostics
        .iter()
        .any(|d| matches!(d.stage, Stage::Preprocess | Stage::Io))
}

/// The imported package files a run loaded, with each file's sha256 — the
/// dependency half of an analysis's cache key.
///
/// # Invariant
/// `None` when a loaded package's bytes cannot be read back, which makes the
/// analysis uncacheable: the key would no longer cover that file. A native
/// package (`compute.lichen`) is embedded in this binary, never on disk, so it
/// is deliberately absent — it cannot change under a cache root.
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

/// Whether every dependency still has the bytes it was built from.
/// A missing or unreadable file is a miss.
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
                // Complete the names in scope at the cursor, the same scope
                // set an unresolved name's "did you mean" candidates use.
                completion_provider: Some(CompletionOptions::default()),
                // The grammar-optional highlight path: semantic tokens carry
                // the color when the tree-sitter grammar is absent.
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
        // A fresh document: its previous text's analysis is not this
        // document's. The spawn is immediate (no edit delay).
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
        // A closed document must not keep its session — the one piece that
        // outlives a request, and whose artifacts then free.
        self.inner.worker.close(uri.clone());
        // Clear the closed document's stale diagnostics, and its last analysis's.
        let files = self
            .inner
            .published_files
            .lock()
            .unwrap()
            .remove(&uri)
            .unwrap_or_default();
        for file_uri in files {
            self.inner
                .client
                .publish_diagnostics(file_uri, Vec::new(), None)
                .await;
        }
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
        let (index, _) = self.inner.index_for(&uri, text, hash).await;
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
        let (index, _) = self.inner.index_for(&uri, text, hash).await;
        // The document, unless the name is a built-in's file the store
        // materialized (`docs/notes/core-prelude.md` §4).
        let response = index.definition_at(position).map(|def| {
            let target = def
                .file
                .as_ref()
                .and_then(|file| Url::from_file_path(&file.path).ok())
                .unwrap_or_else(|| uri.clone());
            GotoDefinitionResponse::Scalar(Location {
                uri: target,
                range: index.definition_range(&def),
            })
        });
        Ok(response)
    }

    /// The names in scope at the cursor: the scope an unresolved name's
    /// "did you mean" candidates are drawn from.
    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let Some((text, hash, _generation)) = self.inner.current(&uri) else {
            return Ok(None);
        };
        let (index, _) = self.inner.index_for(&uri, text, hash).await;
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
        // Delta-encoded from the cached index: the frontend run and the
        // `!Send` `Doc` are not repeated.
        let (index, _) = self.inner.index_for(&uri, text, hash).await;
        Ok(Some(SemanticTokensResult::Tokens(
            index.semantic_tokens_lsp(),
        )))
    }
}

/// Run the stdio LSP server for the composed program `P`, caching the settled
/// imported packages under `cache_root`.
///
/// # Invariant
/// This builds the runtime itself so the generated crate's `main` needs no
/// `#[tokio::main]`; requests are serialized via `concurrency_level(1)`, so an
/// analysis is either cached or run once per document text.
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
