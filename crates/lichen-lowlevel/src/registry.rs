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
        // The artifact's own refs, read off the frozen values: exactly the keys a
        // live ref into it could name (see `StaticModule::referenced_keys`), which
        // is what `evict` checks.
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

    /// [`Self::freeze_mapped`] for the **closure** of `roots` — the per-cell
    /// freeze: only the nodes those roots can reach are filed, so an artifact is
    /// as small as the value it keeps (see [`StaticModule::freeze_closure`]).
    ///
    /// The preconditions are `freeze_mapped`'s, and are checked the same way.  The
    /// dependency check is over the **whole module** rather than the closure,
    /// which is stricter than the closure needs and free: a live module's
    /// referenced keys are registered, or a read of one would already panic.
    pub fn freeze_closure_mapped(
        &mut self,
        module: &Module<P>,
        key: ModuleKey,
        roots: &[NodeId],
        hash: [u8; 32],
    ) -> Freeze {
        let module_refs = crate::static_module::referenced_keys(module);
        for dep in &module_refs {
            assert!(
                self.entries.contains_key(dep),
                "freezing a module that references dependency key {dep:?}, which is not registered here — freeze dependencies first"
            );
        }
        assert!(
            !self.entries.contains_key(&key),
            "freezing a module under device key {key:?}, which is already registered — the same content must not be compiled twice"
        );
        let (static_module, node_map) = StaticModule::freeze_closure(module, key, roots);
        // The **closure's** refs, not the whole module's: the module-level set is a
        // superset, and a superset would refuse to evict an artifact nothing
        // actually references — a leak in the name of safety.
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
    /// obligations it owns ([`Release`]).
    ///
    /// **Two preconditions, and this call can only check one of them.**
    ///
    /// It checks that no **live registered artifact** references the key (the
    /// `refs` recorded at freeze time): a static ref is a raw handle into the
    /// artifact's arena, and a surviving artifact's value may hold one — a
    /// retained cell whose value read another retained cell, for instance.  Such
    /// an eviction is refused ([`Eviction::StillReferenced`]) and the caller may
    /// retry once the referencing artifact is gone too.  Two artifacts that
    /// reference each other can therefore never be evicted while both are filed;
    /// that is the honest answer for a cycle.
    ///
    /// It cannot check the other one: a static ref may also live outside the
    /// registry, in a [`Module`] the caller still holds (a `StaticNodeId` names a
    /// key, and [`Module::static_module`] panics on an unregistered one).  So
    /// eviction is the caller's decision, made when the caller knows every module
    /// that could still hold such a ref is gone.
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
