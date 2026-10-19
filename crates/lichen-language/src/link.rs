//! A `link`ed dynamic library: the manifest it exports, its entries, and the
//! lichen source it contributes.
//!
//! # Invariant
//! A library is loaded once and kept alive by its [`LinkedLibrary`]: the entries
//! installed from it ([`LinkedLibrary::install`]) are plain function pointers, so
//! dropping the value unloads code a running program may still call.
//! See docs/notes/native-link-library.md.

use std::path::Path;

use lichen_highlevel::ir::{ExprId, Loc};
use lichen_highlevel::native::{NativeApply, NativeArg, NativeOp};
use lichen_highlevel::program::{Ctx, HighProgram};
use lichen_lowlevel::{ExternalEntry, LowOperator};

/// The manifest version this loader reads.
const MANIFEST_VERSION: &str = "lichen-link 1";

/// The revision the compiler was built from, when the build stamped one.
const BUILD_REV: Option<&str> = option_env!("LICHEN_BUILD_REV");

/// The tag a generated library carries, and the tag this compiler requires.
///
/// # Invariant
/// A library's entries use this build's `Module` and value layouts, so a
/// library stamped with any other tag is refused at load
/// (docs/notes/native-link-library.md §4).
pub fn build_tag() -> String {
    match BUILD_REV {
        Some(rev) => format!("{}+{rev}", env!("CARGO_PKG_VERSION")),
        None => format!("{}+dev", env!("CARGO_PKG_VERSION")),
    }
}

/// One family entry a library serves: the lichen op name, the instance the
/// compiler records, and the exported symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Family {
    pub name: String,
    pub instance: u32,
    pub symbol: String,
}

/// The manifest a `link`ed library exports, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    build: String,
    families: Vec<Family>,
    source: String,
}

impl Manifest {
    /// Parse a manifest: a version line, header lines, a `source` line, and the
    /// lichen source after it.
    ///
    /// # Invariant
    /// The source is everything after the `source` line, verbatim — it is lichen
    /// source, not manifest data, and may contain any bytes the language allows.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut lines = text.split_inclusive('\n');
        let Some(version) = lines.next() else {
            return Err("an empty manifest".into());
        };
        if version.trim_end() != MANIFEST_VERSION {
            return Err(format!(
                "unsupported manifest version {:?}; expected {MANIFEST_VERSION:?}",
                version.trim_end()
            ));
        }
        let mut build = String::new();
        let mut families = Vec::new();
        let rest_start;
        loop {
            let Some(line) = lines.next() else {
                return Err("the manifest has no `source` line".into());
            };
            let trimmed = line.trim_end();
            if trimmed == "source" {
                rest_start = text.len() - lines.clone().map(str::len).sum::<usize>();
                break;
            }
            if let Some(value) = trimmed.strip_prefix("build ") {
                build = value.to_string();
            } else if let Some(value) = trimmed.strip_prefix("family ") {
                let mut parts = value.split(' ');
                let (Some(name), Some(instance), Some(symbol)) =
                    (parts.next(), parts.next(), parts.next())
                else {
                    return Err(format!("a malformed family line: {trimmed:?}"));
                };
                let instance = instance
                    .parse()
                    .map_err(|_| format!("a family's instance is not a number: {trimmed:?}"))?;
                families.push(Family {
                    name: name.to_string(),
                    instance,
                    symbol: symbol.to_string(),
                });
            } else if trimmed.is_empty() {
                // A blank header line is nothing.
            } else {
                return Err(format!("an unknown manifest line: {trimmed:?}"));
            }
        }
        Ok(Self {
            build,
            families,
            source: text[rest_start..].to_string(),
        })
    }

    /// The build tag the library was built with.
    pub fn build(&self) -> &str {
        &self.build
    }

    /// The family entries.
    pub fn families(&self) -> &[Family] {
        &self.families
    }

    /// The lichen source the library contributes.
    pub fn source(&self) -> &str {
        &self.source
    }
}

/// A loaded library: its manifest, and the entries it exports.
pub struct LinkedLibrary {
    manifest: Manifest,
    library: libloading::Library,
}

impl LinkedLibrary {
    /// Load `path` and read its manifest.
    ///
    /// # Safety
    /// The caller asserts the library was built by this lichen and this compiler:
    /// its entries take this build's `Module` and value layouts
    /// (docs/notes/native-link-library.md §4).
    pub unsafe fn load(path: &Path) -> Result<Self, String> {
        // SAFETY: the caller upholds the build-compatibility contract.
        let library = unsafe { libloading::Library::new(path) }
            .map_err(|error| format!("cannot load {}: {error}", path.display()))?;
        let text = {
            type ManifestFn = unsafe extern "C" fn(*mut *const u8, *mut usize);
            // SAFETY: the symbol's type is the contract every linked library exports.
            let manifest = unsafe { library.get::<ManifestFn>(b"lichen_link_manifest") }
                .map_err(|error| format!("{} exports no manifest: {error}", path.display()))?;
            let mut pointer: *const u8 = std::ptr::null();
            let mut len: usize = 0;
            // SAFETY: the entry writes the two words it is given.
            unsafe { manifest(&mut pointer, &mut len) };
            // SAFETY: the entry's contract is a live byte range of `len` bytes.
            unsafe { std::str::from_utf8(std::slice::from_raw_parts(pointer, len)) }
                .map_err(|_| "the manifest is not UTF-8".to_string())?
                .to_string()
        };
        let manifest = Manifest::parse(&text)?;
        let expected = build_tag();
        if manifest.build() != expected {
            return Err(format!(
                "the library was built for {:?}; this compiler is {expected:?}",
                manifest.build()
            ));
        }
        Ok(Self { manifest, library })
    }

    /// The lichen source the library contributes.
    pub fn source(&self) -> &str {
        self.manifest.source()
    }

    /// The parsed manifest.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Install every family entry under `library`, so an `External` operator can
    /// resolve it.
    pub fn install(&self, library: u32) -> Result<(), String> {
        for family in self.manifest.families() {
            // SAFETY: an entry is a plain `extern "C"` symbol, and
            // `LinkedLibrary::load` asserted build compatibility.
            let entry = unsafe {
                self.library
                    .get::<ExternalEntry>(family.symbol.as_bytes())
                    .map_err(|error| format!("cannot resolve {:?}: {error}", family.symbol))?
            };
            lichen_lowlevel::install_process_external(library, family.instance, *entry);
        }
        Ok(())
    }

    /// The library's native-op registry, for a compiler building a program.
    ///
    /// # Invariant
    /// One op per family, keyed by the manifest's name: the names and the op
    /// boxes are leaked, and the caller joins the families into a registry.
    pub fn ops<P>(&self, library: u32) -> Vec<(&'static str, &'static dyn NativeOp<P>)>
    where
        P: HighProgram,
        P::Value: lichen_highlevel::program::ValueType,
        P::Operator: From<LowOperator>,
    {
        self.manifest
            .families()
            .iter()
            .map(|family| {
                let name: &'static str = Box::leak(family.name.clone().into_boxed_str());
                let op: &'static dyn NativeOp<P> = Box::leak(Box::new(LinkOp {
                    library,
                    instance: family.instance,
                }));
                (name, op)
            })
            .collect()
    }
}

/// One linked family's native op: it emits the `External` operator the entry runs
/// under, over the call's single operand.
struct LinkOp {
    library: u32,
    instance: u32,
}

impl<P> NativeOp<P> for LinkOp
where
    P: HighProgram,
    P::Value: lichen_highlevel::program::ValueType,
    P::Operator: From<LowOperator>,
{
    /// # Invariant
    /// The operand is the wrapper's parameter cell — the one node the apply's
    /// clone re-points per call. An aggregate built by [`Ctx::array_node`] is a
    /// decided value, and its items are **not** re-pointed
    /// (docs/notes/native-link-library.md §11).
    fn build(
        &self,
        ctx: &mut dyn Ctx<P>,
        _e: ExprId,
        args: &[NativeArg],
        _loc: Loc,
    ) -> NativeApply {
        let Some(argument) = args.first() else {
            return NativeApply {
                value: ctx.op_node(
                    P::Operator::from(LowOperator::External {
                        library: self.library,
                        instance: self.instance,
                    }),
                    None,
                ),
                decided: false,
            };
        };
        NativeApply {
            value: ctx.op_node(
                P::Operator::from(LowOperator::External {
                    library: self.library,
                    instance: self.instance,
                }),
                Some(argument.value),
            ),
            decided: false,
        }
    }
}

/// Splice every linked library's source in front of `code`, so the wrappers
/// compile in the program's own module.
///
/// # Invariant
/// The splice is a **source** join, not a frozen package: a wrapper compiled
/// here instantiates per application like any program function. The caller owns
/// the positions the prefix shifts (a diagnostic inside the program names the
/// spliced offset).
pub fn splice(links: &[&LinkedLibrary], code: &str) -> String {
    let mut spliced = String::new();
    for link in links {
        spliced.push_str(link.source());
        if !link.source().ends_with('\n') {
            spliced.push('\n');
        }
    }
    spliced.push_str(code);
    spliced
}
