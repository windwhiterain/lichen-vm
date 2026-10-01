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
//! # The compile worker
//!
//! The analysis is *incremental*: a document's compile is driven by a
//! [`BufferSession`](lichen_language::session::BufferSession), which retains the
//! `cache`d bindings' frozen artifacts across edits and re-lexes and re-parses
//! only what an edit touched (`docs/notes/incremental-update.md`).  A session
//! holds the checker's [`Build`](lichen_highlevel::checker::Build), so it is
//! `!Send` for the same reason `Doc` is — and unlike `Doc`, it must **outlive**
//! the request that used it.  It therefore cannot live in this struct, and it
//! cannot live in a `spawn_blocking` closure either: the pool may run that on any
//! thread.  So the `!Send` half of the server lives on one dedicated thread
//! ([`Worker`]): the package store, one shared registry, and one session per open
//! document.  Requests hand it a text and await a `Send` index back.
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
//! Neither can abort a compile that has already started: `tower-lsp` answers
//! `$/cancelRequest` by dropping the request future, so the worker finishes the
//! job and its answer is dropped.

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
    /// The compile worker — the one thread that owns the `!Send` half of the
    /// server: the package store, the shared registry, and one
    /// [`BufferSession`](lichen_language::session::BufferSession) per open
    /// document.  See the module docs.
    worker: Worker,
    /// The only way a generation is minted: strictly increasing, never reused,
    /// so an analysis of a closed-then-reopened document can never match the
    /// generation of an older analysis of the same URI.
    generations: AtomicU64,
    // `fn() -> P` keeps the type parameter without requiring `P: Send + Sync`
    // (the composed value/operator leaves carry raw arena pointers, so `P`
    // itself is not `Send`); a function-pointer phantom is always `Send + Sync`.
    _program: std::marker::PhantomData<fn() -> P>,
}

/// The compile worker: a dedicated thread owning every `!Send` compile artifact.
///
/// The sessions have to outlive the request that built them (that is what makes
/// a compile incremental) and they are `!Send` (they hold the checker's
/// [`Build`](lichen_highlevel::checker::Build)), so neither this struct nor a
/// `spawn_blocking` closure can hold them.  The thread is the boundary: a job
/// goes in, a `Send` [`Analysis`] comes back.
///
/// One thread costs nothing here: the transport serializes requests
/// (`concurrency_level(1)`), and a session is a single-threaded object by
/// construction — its value is the state the last compile left behind.
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
    /// What the compile did to the document's retained cells, and whether it
    /// reused the established build instead of re-lowering and re-checking it.
    /// Pushed to the client as [`AnalysisStats`].
    cells: CellEvents,
    reused_build: bool,
}

/// The analysis telemetry the server pushes to the client: what the last compile
/// did to the document's retained cells (`docs/notes/incremental-update.md`).
///
/// A custom notification, because the session's own event surface exists exactly
/// because a caller cannot otherwise tell a rebuild that reused nine cells from
/// one that reused none — and this is that caller.  An editor that does not know
/// the method ignores it (LSP has no registration step for notifications), and
/// the integration test is what reads it.
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
    /// Spawn the worker thread.  Everything it needs is built *on* it, so the
    /// closure captures only `Send` values (a path and the receiver).
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
    /// The device handle, taken for one run and handed back.  A `PackageStore` is
    /// built per run rather than kept, so its `packages` map is exactly *that*
    /// document's import closure — which is what an analysis records as its
    /// dependencies (a store kept across runs would accumulate every document's
    /// imports and invalidate every analysis when any of them moved).
    device: Option<DeviceRegistry>,
    /// The registry every session's cells and every imported package are filed
    /// in.  One registry, because a cell's frozen closure names the imports its
    /// value read and must resolve them where it is filed
    /// ([`BufferSession::with_registry`]), and because all the open documents
    /// share it.
    registry: Arc<RwLock<Registry<P>>>,
    /// One session per open document, kept across requests: this is what makes
    /// an edit incremental rather than a fresh compile.
    sessions: HashMap<Url, BufferSession<P>>,
    /// Per document: the imported files its session's retained cells were
    /// computed from, as of its last analysis (`None` when a loaded file could
    /// not be read back, so the record proves nothing).
    ///
    /// A cell is a *value*, not a program: it was computed from the bytes those
    /// files held then, and nothing in the document's own text records that — its
    /// content key is over its resolved structure.  So the worker keeps the
    /// record, and drops the session when it cannot prove the files are
    /// unchanged, which re-derives everything from the new bytes.  This is the
    /// coarse cut: the fine one names the cells that read the changed file
    /// (`docs/notes/incremental-update.md` §12.3).
    dependencies: HashMap<Url, Option<Vec<(PathBuf, Hash)>>>,
}

impl<P> WorkerState<P>
where
    P: LangProgramShape,
    P::Value: ValueType + AsEnum<ComputeValue> + From<ComputeValue> + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + From<ComputeOperator> + 'static,
{
    fn new(cache_root: PathBuf) -> Self {
        // A codec that cannot persist has no device at all; an in-memory store is
        // the intended mode then, exactly as `open_store` decides it.
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
                // A compile that panics must not take the worker — and with it
                // every later request — down.  The session that did it is dropped
                // (its state is not trusted again) and the request is answered by
                // the one-shot frontend instead: the degradation this mechanism
                // exists to avoid, not a failure.
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
    /// The session is the only holder of its reports — [`Self::analyze`] answers
    /// with the `Send` index alone, which holds no static ref — so once it is
    /// gone nothing outside the registry can still point into those artifacts,
    /// which is `evict`'s caller-side precondition.  What [`evict_unreachable`]
    /// still refuses (a live artifact in the registry that references the key,
    /// including another document's cell) is dropped with the session: the debt
    /// is the session's, and a forgotten session has none to pay.
    ///
    /// [`evict_unreachable`]: BufferSession::evict_unreachable
    fn forget(&mut self, uri: &Url) {
        self.dependencies.remove(uri);
        if let Some(mut session) = self.sessions.remove(uri) {
            session.evict_unreachable();
        }
    }

    /// The incremental analysis: preprocess (the caller's stage — it owns the
    /// store and the block's directive spans), hand the session the resulting
    /// view, and build the editor index from what its compile produced.
    fn analyze(&mut self, uri: Url, text: &str) -> Analysis {
        // A session whose imported files moved is dropped first: its cells hold
        // the old bytes' values.  An unprovable record (`None`) is dropped too —
        // the honest answer when the files cannot be read back is to re-derive.
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
        let (index, diagnostics) = index::<P>(text, line_starts, &pre, artifacts);
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

    /// The one-shot analysis — the whole frontend and a check, with no session
    /// and nothing retained.  This is what the server did before the sessions
    /// existed, kept as the answer to a compile the session could not do.
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
            // The one-shot path retains nothing and reuses nothing: there is no
            // session for it to have read a cell from.
            cells: CellEvents::default(),
            reused_build: false,
        }
    }

    /// A store for one run, over the worker's device handle and its registry.
    ///
    /// The registry is the shared one, not the store's own: the imports the store
    /// resolves must be registered where the sessions' cells are filed, and where
    /// every other open document's imports already are.
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
    /// this text with every dependency unchanged, else one compile on the worker.
    ///
    /// The second half is what that compile did, and it is `None` for a cache
    /// hit — nothing was compiled, so there is nothing to report.
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

    /// The cached index for `uri` when it holds `hash` and every dependency it
    /// was built from still has the bytes it was built from.
    ///
    /// A miss is a miss whether the text moved or a dependency did — the compile
    /// worker drops the *session* in the second case, which is state this cache
    /// knows nothing about (see `WorkerState::dependencies`).
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

    /// Analyze the document's current text and publish its diagnostics, unless
    /// a newer edit superseded this analysis while the frontend ran.
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
        // The compile's own event surface, pushed alongside the diagnostics: a
        // caller that cannot see it cannot tell an incremental analysis from a
        // full one, and this is the caller (`AnalysisStats`).
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
        // `cache_root` is this vocabulary's `compilers/<plugin-set-key>` slot
        // (the shipping server uses the empty plugin set's slot).  It is the
        // root the device registry is opened at — once, on the compile worker —
        // so the settled imported packages are cached on disk and shared
        // cross-process with the `lichen` compiler.  See
        // `docs/notes/liche-lsp-home.md`.
        let home = LichenHome::at(cache_root);
        home.ensure();
        Backend {
            inner: Arc::new(Inner {
                client,
                documents: Mutex::new(HashMap::new()),
                indexes: Mutex::new(HashMap::new()),
                worker: Worker::spawn::<P>(home.cache_root().to_path_buf()),
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

/// The package store for one run on the worker: the device handle the worker
/// keeps open, else a fresh open.  An in-memory store is used only when it is
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
fn cacheable<P: LangProgramShape>(diagnostics: &[Diag<P>]) -> bool {
    !diagnostics
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
        // The worker forgets the document too: its session — and the artifacts
        // the retained cells hold — is the one piece of the server that outlives
        // a request, and a closed document must not keep it.  This is also when
        // those artifacts become evictable (see `WorkerState::forget`).
        self.inner.worker.close(uri.clone());
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
        // The frontend classifies every token into a `SemanticTokens` payload
        // (delta-encoded, with the legend indices), computed from the cached
        // index — the frontend run and the `!Send` `Doc` are not repeated.
        let (index, _) = self.inner.index_for(&uri, text, hash).await;
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
