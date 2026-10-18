//! The compute sub-language: this plugin composed over the bare language.
//! See docs/notes/plugin-taxonomy.md.
//!
//! # Invariant
//!
//! This module is the only place the compute vocabulary exists as a whole. The
//! leaves, the operators and the embedded wrapper stay program-generic in
//! [`crate::compute`]; the bare language composes none of them, so a host either
//! composes what is composed here or runs without compute at all.

use std::path::{Path, PathBuf};

use lichen_highlevel::native::NativeOps;
use lichen_language::package::{PackageStore, Wiring};
use lichen_utils::extend::AsEnum;

use crate::compute::{ComputeOperator, ComputeValue, LANDING_DIAGNOSTIC, WRAPPER_SOURCE};
use crate::compute_native_ops;

/// The virtual path this sub-language's own package is imported at.
pub const COMPUTE_PATH: &str = "compute.lichen";

// The composed vocabulary: the bare language's leaves plus this plugin's.
lichen_language::lang_compose_vocabulary! {
    attrs = [
        lichen_perspective::Perspective as Perspective;
        lichen_doc::Doc as Doc;
        lichen_highlevel::refinement::Refinement as Refinement;
    ]
    // Each attribute's `AttrExt` lists the operator bounds it needs.
    [ P::Operator: From<lichen_perspective::GcdOp> + From<lichen_lowlevel::LowOperator> ];
    values = [
        lichen_lowlevel::LowValue as LowValue;
        lichen_highlevel::program::TypeValue as TypeValue;
        ComputeValue as ComputeValue;
    ];
    operators = [
        lichen_lowlevel::LowOperator as LowOperator;
        lichen_highlevel::program::TypeOperator as TypeOperator;
        lichen_perspective::GcdOp as GcdOp;
        ComputeOperator as ComputeOperator;
    ];
    render = [ value_name ; ];
}

/// This vocabulary's value spellings: the compute leaves, named by kind.
///
/// # Invariant
/// A value's name is its kind, not the backend it dispatches to, and a leaf this
/// composition does not carry is named by no one.
pub fn value_name(value: &LangValue) -> Option<String> {
    match AsEnum::<ComputeValue>::as_enum(value) {
        Some(ComputeValue::Kernel(_)) => Some("Kernel".to_string()),
        Some(ComputeValue::ParKernel(..)) => Some("ParKernel".to_string()),
        Some(ComputeValue::Buffer(..)) => Some("Buffer".to_string()),
        _ => None,
    }
}

/// This sub-language's own package, as a store registers it.
///
/// # Invariant
///
/// The wrapper is compiled against this plugin's private ops, so `$jit` and
/// `$launch` resolve for it alone and a second plugin registering its own never
/// collides. It is always fresh in memory: its `Kernel` values are runtime-only,
/// so the artifact format cannot serialize them.
pub fn native_packages() -> [(&'static str, &'static str, NativeOps<LangProgram>); 1] {
    [(
        COMPUTE_PATH,
        WRAPPER_SOURCE,
        compute_native_ops!(LangProgram),
    )]
}

/// What a `--pack` installs here: this sub-language's floor, and the
/// pass channel a run that installed one reports.
///
/// # Invariant
///
/// A pack is read before the run compiles anything, so the floor it installs is
/// the one every graph consults; a path that is not a pack stops the run before
/// anything is installed.
pub fn wiring() -> Wiring<'static> {
    fn install(paths: &[PathBuf]) -> Result<(), String> {
        let packs = crate::packs::load_packs(paths).map_err(|refusal| refusal.to_string())?;
        if !packs.is_empty() {
            crate::floor::install_floor_for_run(&packs);
        }
        Ok(())
    }
    Wiring {
        install: &install,
        pass_categories: &[LANDING_DIAGNOSTIC],
    }
}

/// Register this sub-language's own package on `store`, as the virtual
/// `compute.lichen` a program imports.
pub fn register_compute(store: &mut PackageStore<LangProgram>) -> Result<(), String> {
    store.register_native(
        COMPUTE_PATH,
        WRAPPER_SOURCE,
        compute_native_ops!(LangProgram),
    )?;
    Ok(())
}

/// The plugin list that names this sub-language, in the slot key's own spelling.
///
/// # Invariant
/// `compiler_slot_key` joins the parts it is handed and the package manager spells one
/// `<alias>@<version>`; the alias is the one [`register_compute`] registers under, so
/// this list names the vocabulary whose assembler wrote the cached modules and a
/// second plugin composed over the bare language lands in a slot of its own.
fn compute_plugins() -> Vec<String> {
    vec![format!("compute@{}", env!("CARGO_PKG_VERSION"))]
}

/// Point the assembled-kernel disk cache at `cache_root`'s compiler slot.
///
/// # Invariant
///
/// The kernels sit beside the device's own artifacts rather than among them, so one
/// cache cannot be mistaken for the other's contents, and the slot key is derived from
/// the plugin this module registers: the assembler and the plugin are one binary, and
/// a slot naming neither would serve its bytes to its replacement.
fn configure_kernel_disk_cache(cache_root: &Path) -> PathBuf {
    lichen_runtime::configure_kernel_disk_cache(lichen_runtime::kernel_disk_cache_dir(
        cache_root,
        lichen_utils::cache::DEFAULT_CORE_REPO,
        &compute_plugins(),
    ))
}

/// A store over this sub-language with its own package registered.
///
/// # Invariant
///
/// `cache_root` `None` keeps the store in memory, so nothing this sub-language
/// compiles is ever serialized; a rooted store writes under the same shipping
/// slot the compiler reads (`persist::shipping_cache_root`) and names the kernel
/// disk cache beside it. The first rooted store in a process wins that, so a host
/// holding two roots reads the one that took effect off
/// [`configure_kernel_disk_cache`] rather than assuming its own.
pub fn new_store(cache_root: Option<PathBuf>) -> Result<PackageStore<LangProgram>, String> {
    let mut store = match cache_root {
        Some(root) => {
            configure_kernel_disk_cache(&root);
            PackageStore::with_cache_dir(root)
        }
        None => PackageStore::new(),
    };
    register_compute(&mut store)?;
    Ok(store)
}

/// A store over this sub-language, over an already-open device handle.
///
/// # Invariant
///
/// The handle is the caller's, so a long-lived server reuses one lock across its
/// runs rather than reopening per analysis; the cache root names the kernel
/// disk cache on the same terms [`new_store`] does.
pub fn store_with_device(
    cache_root: PathBuf,
    device: lichen_language::persist::DeviceRegistry,
) -> Result<PackageStore<LangProgram>, String> {
    configure_kernel_disk_cache(&cache_root);
    let mut store = PackageStore::with_device(cache_root, device);
    register_compute(&mut store)?;
    Ok(store)
}
