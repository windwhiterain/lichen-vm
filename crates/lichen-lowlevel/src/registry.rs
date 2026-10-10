//! The [`Registry`] inherent API: modules bound to it, and the resident
//! artifact map (freeze, insert, read, iterate).

use super::*;

/// The top bit of a key: set for a **retained cell**'s artifact, clear for a device's.
pub const CELL_KEY_BIT: u64 = 1 << 63;
impl<P: Program> Default for Registry<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: Program> Registry<P> {
    pub fn new() -> Self {
        Registry {
            entries: HashMap::new(),
            next_cell_key: 0,
        }
    }

    /// A fresh key for a **retained cell**'s artifact, unique within this registry.
    ///
    /// # Invariant
    ///
    /// Cell keys occupy their own space ([`CELL_KEY_BIT`]), disjoint from the device's
    /// dense counter and its reclaimed holes: a cell's frozen closure must resolve the
    /// packages it read through the registry it is filed in, so a cell and an import
    /// are filed side by side. A cell's identity is its occurrence path, never its
    /// content, so the counter only has to be distinct.
    pub fn allocate_cell_key(&mut self) -> ModuleKey {
        self.next_cell_key += 1;
        ModuleKey::from_raw(CELL_KEY_BIT | self.next_cell_key)
    }

    /// An executing module bound to this registry: every static ref it touches
    /// resolves through `self`.
    ///
    /// # Invariant
    ///
    /// Modules in one thread share the one registry `Arc`, never each other: a
    /// `Module` is per-thread owned (`Arc<Module>` does not exist).
    pub fn new_module(registry: &Arc<RwLock<Registry<P>>>) -> Module<P> {
        Module::with_registry(registry.clone())
    }

    /// Compile a dynamic module into a static artifact and file it under `key`.
    ///
    /// # Invariant
    ///
    /// `key` is the caller's already-allocated device key, unregistered here: refs are
    /// baked with their final key, and the same content must not be compiled twice. A
    /// failed build leaves the registry untouched.
    pub fn freeze(&mut self, module: &Module<P>, key: ModuleKey, hash: [u8; 32]) -> ModuleKey {
        self.freeze_mapped(module, key, hash).key
    }

    /// Like [`Self::freeze`], but also returns the source→statics node map.
    ///
    /// # Invariant
    ///
    /// A source carrying static refs keeps them verbatim — they are absolute from birth.
    /// Every module key its values reference must already be registered here, so the
    /// artifact resolves from any importer through this registry. Freeze dependencies
    /// first.
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
        // The artifact's own refs: the keys a live ref into the artifact could name.
        let refs = static_module.referenced_keys();
        self.entries.insert(
            key,
            Package {
                module: Arc::new(static_module),
                meta: Default::default(),
                hash,
                refs,
            },
        );
        Freeze { key, node_map }
    }

    /// [`Self::freeze_mapped`] for the **closure** of `roots`: only reachable nodes file.
    ///
    /// # Invariant
    ///
    /// The closure's ref set, not the whole module's, is what the artifact references,
    /// so that is the predicate checked: the module's set is a superset and costs a scan
    /// of every node on every freeze. The check runs before the freeze, so a refusal has
    /// transferred nothing.
    pub fn freeze_closure_mapped(
        &mut self,
        module: &Module<P>,
        key: ModuleKey,
        roots: &[NodeId],
        hash: [u8; 32],
    ) -> Freeze {
        assert!(
            !self.entries.contains_key(&key),
            "freezing a module under device key {key:?}, which is already registered — the same content must not be compiled twice"
        );
        let (static_module, node_map) = StaticModule::freeze_closure(
            module,
            key,
            roots,
            |dependencies| {
                for dependency in dependencies {
                    assert!(
                        self.entries.contains_key(&dependency),
                        "freezing a module that references dependency key {dependency:?}, which is not registered here — freeze dependencies first"
                    );
                }
            },
        );
        // The **closure's** refs: a superset would refuse to evict an unreferenced one.
        let refs = static_module.referenced_keys();
        self.entries.insert(
            key,
            Package {
                module: Arc::new(static_module),
                meta: Default::default(),
                hash,
                refs,
            },
        );
        Freeze { key, node_map }
    }

    /// [`Self::freeze_mapped`] for a **recompile**: the key is the file's own slot.
    ///
    /// # Invariant
    ///
    /// Every live artifact that referenced the replaced one is gone, or is replaced in this
    /// same operation: a recompile rebuilds every dependent whose identity moved, and a
    /// session that read the old artifact is dropped before its next compile
    /// (`docs/notes/incremental-update.md` §8), so a ref surviving into the replaced key
    /// names the *new* arena rather than crashing.
    pub fn freeze_mapped_replacing(
        &mut self,
        module: &Module<P>,
        key: ModuleKey,
        hash: [u8; 32],
    ) -> Freeze {
        self.entries.remove(&key);
        self.freeze_mapped(module, key, hash)
    }

    /// File an already-built artifact (one loaded from the device store) under its `key`.
    ///
    /// # Invariant
    ///
    /// `key` is unregistered: a registered key is a resident module, and re-inserting it
    /// would shadow the resident one. The caller checks first ([`Self::get`], comparing
    /// [`Package::hash`] to recognize a key reallocated after reclamation).
    pub fn insert_module(&mut self, key: ModuleKey, hash: [u8; 32], module: StaticModule<P>) {
        assert!(
            !self.entries.contains_key(&key),
            "inserting a module under device key {key:?}, which is already registered — a loaded module is never shadowed"
        );
        let refs = module.referenced_keys();
        self.entries.insert(
            key,
            Package {
                module: Arc::new(module),
                meta: Default::default(),
                hash,
                refs,
            },
        );
    }

    /// Evict a filed artifact: drop it, which frees its arena and runs the release
    /// obligations it owns.
    ///
    /// # Invariant
    ///
    /// No live registered artifact references the key (the `refs` recorded at freeze
    /// time); such an eviction is refused ([`Eviction::StillReferenced`]) and the caller
    /// may retry once the referencing artifact is gone too, so two artifacts referencing
    /// each other can never be evicted while both are filed.
    ///
    /// # Safety
    ///
    /// A static ref may also live outside the registry, in a [`Module`] the caller still
    /// holds — `Module::static_module` panics on an unregistered key. Eviction is the
    /// caller's decision, made when it knows every module that could still hold such a
    /// ref is gone.
    pub fn evict(&mut self, key: ModuleKey) -> Eviction {
        if !self.entries.contains_key(&key) {
            return Eviction::NotRegistered;
        }
        let referenced = self
            .entries
            .iter()
            .any(|(&other, package)| other != key && package.refs.contains(&key));
        if referenced {
            return Eviction::StillReferenced;
        }
        self.entries.remove(&key);
        Eviction::Freed
    }

    /// Set the opaque per-package metadata; higher layers store markers and paths.
    pub fn set_package_meta(&mut self, key: ModuleKey, meta: P::PackageMeta) {
        self.entries
            .get_mut(&key)
            .expect("set_package_meta on an unregistered module key")
            .meta = meta;
    }

    /// The registered module behind a device key; `None` means no such key.
    pub fn get(&self, key: ModuleKey) -> Option<&Package<P>> {
        self.entries.get(&key)
    }

    /// Iterate the registered modules — the device's directory listing.
    pub fn iter(&self) -> impl Iterator<Item = (ModuleKey, &Package<P>)> {
        self.entries.iter().map(|(&key, package)| (key, package))
    }

    /// Whether the registry holds no registered modules.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
