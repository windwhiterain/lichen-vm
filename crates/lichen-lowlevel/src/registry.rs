//! The [`Registry`] inherent API: modules bound to it, and the resident
//! artifact map (freeze, insert, read, iterate).

use super::*;
impl<P: Program> Default for Registry<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: Program> Registry<P> {
    pub fn new() -> Self {
        Registry {
            entries: HashMap::new(),
        }
    }

    /// An executing module bound to this registry: every static ref it
    /// touches resolves through `self`.  Modules in one thread share the one
    /// registry `Arc` (see the `Registry` doc for why it cannot cross a
    /// thread) — a `Module` itself is never shared (`Arc<Module>` does not
    /// exist; it is a per-thread owned value).
    pub fn new_module(registry: &Arc<RwLock<Registry<P>>>) -> Module<P> {
        Module::with_registry(registry.clone())
    }

    /// Compile a dynamic module into a static artifact and file it under
    /// `key` — the device key allocated by the device registry (the caller
    /// provides it so the artifact's refs are baked with their final key;
    /// the key must not already be registered — same content must not be
    /// compiled twice).  A failed build leaves the registry untouched.
    pub fn freeze(&mut self, module: &Module<P>, key: ModuleKey, hash: [u8; 32]) -> ModuleKey {
        self.freeze_mapped(module, key, hash).key
    }

    /// Like [`Self::freeze`], but also returns the source→statics node map
    /// so a caller can construct exported [`StaticNodeId`]s for root nodes.
    ///
    /// The source may itself carry static refs — its frozen dependencies
    /// (a package importing packages).  They are absolute from birth, so the
    /// artifact keeps them verbatim; this method checks the soundness
    /// precondition the verbatim refs imply: every module key the source's
    /// values reference must already be registered *here*, so the frozen
    /// artifact resolves from any importer through this registry.  Freeze
    /// dependencies first.
    pub fn freeze_mapped(&mut self, module: &Module<P>, key: ModuleKey, hash: [u8; 32]) -> Freeze {
        for dep in crate::static_module::referenced_keys(module) {
            assert!(
                self.entries.contains_key(&dep),
                "freezing a module that references dependency key {dep:?}, which is not registered here — freeze dependencies first"
            );
        }
        assert!(
            !self.entries.contains_key(&key),
            "freezing a module under device key {key:?}, which is already registered — the same content must not be compiled twice"
        );
        let (static_module, node_map) = StaticModule::from_module_mapped(module, key);
        self.entries.insert(
            key,
            Package {
                module: Arc::new(static_module),
                meta: Default::default(),
                hash,
            },
        );
        Freeze { key, node_map }
    }

    /// File an already-built artifact (a module loaded from the device's
    /// persistent store) under its device `key` — the load-time mirror of
    /// [`Self::freeze_mapped`]: the artifact's refs are already baked with
    /// `key`, nothing is rebuilt or re-keyed.  The key must not already be
    /// registered — a registered key is a loaded module, and re-inserting
    /// it would shadow the resident one; the caller checks first
    /// ([`Self::get`], comparing [`Package::hash`] to recognize a key
    /// reallocated after reclamation).
    pub fn insert_module(&mut self, key: ModuleKey, hash: [u8; 32], module: StaticModule<P>) {
        assert!(
            !self.entries.contains_key(&key),
            "inserting a module under device key {key:?}, which is already registered — a loaded module is never shadowed"
        );
        self.entries.insert(
            key,
            Package {
                module: Arc::new(module),
                meta: Default::default(),
                hash,
            },
        );
    }

    /// Set the opaque per-package metadata for an existing registered
    /// package.  Higher layers use this to store export markers, source
    /// paths, or any future package-level state without the lowlevel
    /// knowing what that state means.
    pub fn set_package_meta(&mut self, key: ModuleKey, meta: P::PackageMeta) {
        self.entries
            .get_mut(&key)
            .expect("set_package_meta on an unregistered module key")
            .meta = meta;
    }

    /// The registered module behind a device key — the file-system `get`.
    /// `None` is the "no such key" answer; a static ref naming an
    /// unregistered key is a broken module graph and panics at its
    /// resolution site.
    pub fn get(&self, key: ModuleKey) -> Option<&Package<P>> {
        self.entries.get(&key)
    }

    /// Iterate the registered modules — the device's directory listing.
    /// (The persistent store uses it to collect the arenas a serialized
    /// artifact's payload refs point into.)
    pub fn iter(&self) -> impl Iterator<Item = (ModuleKey, &Package<P>)> {
        self.entries.iter().map(|(&key, package)| (key, package))
    }

    /// Whether the registry holds no registered modules.  (Sources with
    /// static refs — packages importing packages — may only be frozen into
    /// a registry that holds their dependencies; see
    /// [`Self::freeze_mapped`].)
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
