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
use lichen_highlevel::program::{Ctx, HighProgram, TypeValue, ValueType};
use lichen_lowlevel::{AnyNodeId, ExternalEntry, LowOperator};
use lichen_utils::extend::AsEnum;

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
    program: String,
    build: String,
    families: Vec<Family>,
    /// The names the wrapper binds at top level, in source order.
    bindings: Vec<String>,
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
        let mut program = String::new();
        let mut build = String::new();
        let mut families = Vec::new();
        let mut bindings = Vec::new();
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
            if let Some(value) = trimmed.strip_prefix("program ") {
                program = value.to_string();
            } else if let Some(value) = trimmed.strip_prefix("build ") {
                build = value.to_string();
            } else if let Some(value) = trimmed.strip_prefix("bind ") {
                bindings.push(value.to_string());
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
        if program.is_empty() {
            return Err("the manifest names no program".into());
        }
        Ok(Self {
            bindings,
            program,
            build,
            families,
            source: text[rest_start..].to_string(),
        })
    }

    /// A manifest from its parts: what a generator writes and [`Self::parse`]
    /// reads back.
    pub fn new(
        bindings: Vec<String>,
        program: impl Into<String>,
        build: impl Into<String>,
        families: Vec<Family>,
        source: impl Into<String>,
    ) -> Self {
        Self {
            program: program.into(),
            build: build.into(),
            families: families.clone(),
            bindings,
            source: source.into(),
        }
    }

    /// The manifest's text.
    ///
    /// # Invariant
    /// `render` followed by `parse` is the identity: the loader has one writer
    /// and one reader, so a generated library and its loader cannot disagree.
    pub fn render(&self) -> String {
        let mut text = String::with_capacity(self.source.len() + 64);
        text.push_str(MANIFEST_VERSION);
        text.push('\n');
        text.push_str("program ");
        text.push_str(&self.program);
        text.push('\n');
        text.push_str("build ");
        text.push_str(&self.build);
        text.push('\n');
        for family in &self.families {
            text.push_str("family ");
            text.push_str(&family.name);
            text.push(' ');
            text.push_str(&family.instance.to_string());
            text.push(' ');
            text.push_str(&family.symbol);
            text.push('\n');
        }
        for binding in &self.bindings {
            text.push_str("bind ");
            text.push_str(binding);
            text.push('\n');
        }
        text.push_str("source\n");
        text.push_str(&self.source);
        text
    }

    /// The program vocabulary the library was generated for.
    pub fn program(&self) -> &str {
        &self.program
    }

    /// The build tag the library was built with.
    pub fn build(&self) -> &str {
        &self.build
    }

    /// The names the wrapper binds at top level, in source order.
    pub fn bindings(&self) -> &[String] {
        &self.bindings
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

    /// The names the wrapper binds at top level.
    pub fn bindings(&self) -> &[String] {
        self.manifest.bindings()
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
/// under, over the call's arguments.
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
    /// The operand is the call's **argument array**, one item per argument in
    /// source order; the entry reads the items it needs. The apply re-points an
    /// open aggregate's items per call (docs/notes/native-link-library.md §17).
    fn build(
        &self,
        ctx: &mut dyn Ctx<P>,
        _e: ExprId,
        args: &[NativeArg],
        _loc: Loc,
    ) -> NativeApply {
        let values: Vec<lichen_lowlevel::NodeId> = args.iter().map(|arg| arg.value).collect();
        let operand = (!values.is_empty()).then(|| ctx.array_node(&values));
        NativeApply {
            value: ctx.op_node(
                P::Operator::from(LowOperator::External {
                    library: self.library,
                    instance: self.instance,
                }),
                operand,
            ),
            decided: false,
        }
    }
}
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

/// One instantiation a built program used: a linked family at one kind.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Used {
    /// The library the family belongs to.
    pub library: u32,
    /// The family's instance.
    pub instance: u32,
    /// The kind's spelling ([`TypeValue::name`]).
    pub kind: String,
}

/// The instantiations a built program used, deduplicated and sorted.
///
/// # Invariant
/// The kind is read off the applied clone's parameter: the wrapper's
/// `(t ! …) => …` leaves the clone's type slot holding the applied type
/// expression, whose first item is the kind marker. A clone whose parameter was
/// never applied states no kind and is skipped; the set is the union over the
/// program's applications, so it is what a generator must instantiate.
pub fn record_uses<P>(module: &mut lichen_lowlevel::Module<P>) -> Vec<Used>
where
    P: lichen_lowlevel::Program,
    P::Value: ValueType + AsEnum<TypeValue>,
{
    let mut probes = Vec::new();
    for (node, _) in &module.nodes {
        let Some(operation) = module.node_operation(node) else {
            continue;
        };
        let Some(LowOperator::External { library, instance }) =
            AsEnum::<LowOperator>::as_enum(&operation.operator)
        else {
            continue;
        };
        let parameter = module
            .node_function(node)
            .and_then(|function| module.functions.get(function))
            .map(|function| function.parameter);
        probes.push((library, instance, parameter));
    }
    let mut used = Vec::new();
    for (library, instance, parameter) in probes {
        let mut cursor = parameter
            .and_then(|parameter| module.expr_type_item(parameter))
            .and_then(dynamic_node);
        for _ in 0..2 {
            let Some(node) = cursor else {
                break;
            };
            let value = module.evaluate_node(AnyNodeId::Dynamic(node), None);
            if let Some(name) = value
                .as_ref()
                .and_then(AsEnum::<TypeValue>::as_enum)
                .map(|value| value.name())
            {
                used.push(Used {
                    library,
                    instance,
                    kind: name.to_string(),
                });
                break;
            }
            // SAFETY: `node` is a live node of `module`, read and not released.
            cursor = unsafe { module.array_items(node) }
                .and_then(|items| items.first())
                .and_then(|item| dynamic_node(item.node));
        }
    }
    used.sort();
    used.dedup();
    used
}

/// The dynamic node a reference names; a frozen ref names nothing this module
/// evaluates.
fn dynamic_node(node: AnyNodeId) -> Option<lichen_lowlevel::NodeId> {
    match node {
        AnyNodeId::Dynamic(node) => Some(node),
        AnyNodeId::Static(_) => None,
    }
}

/// One family entry: the lichen op name, the exported symbol, and the Rust body
/// that answers a call.
pub struct Entry {
    pub name: String,
    pub symbol: String,
    /// An expression over `module`, `operand` and the generated `item` helper,
    /// inserted verbatim, answering one value.
    pub body: String,
}

/// The `lib.rs` a generator writes for a library.
///
/// # Invariant
/// The generator owns the ABI (the `External` signature, the `item` helper, the
/// manifest) and the library author owns the body: every `Entry::body` is
/// inserted verbatim. The operand is the call's argument array
/// ([`LinkOp`]), so a body reads the arguments it needs and no argument shape is
/// privileged.
pub struct LibrarySource {
    /// The author's Rust, inserted verbatim: traits, impls, the generic code.
    pub items: String,
    /// The lichen wrapper the manifest carries, spliced by the loader.
    pub wrapper: String,
    pub entries: Vec<Entry>,
}

impl LibrarySource {
    /// The crate source, generated for the program vocabulary `P`.
    ///
    /// # Invariant
    /// One program, one spelling: `std::any::type_name::<P>()` is written into the
    /// manifest **and** into the generated `type Value`, so a library cannot be
    /// generated for one vocabulary and loaded by another.
    ///
    /// Every entry is `#[rustfmt::skip]`: the body is the author's verbatim text,
    /// so reformatting it would make a later render differ and `cargo fmt --check`
    /// would fail.
    pub fn render<P: lichen_lowlevel::Program>(&self, build: &str) -> String {
        let program = std::any::type_name::<P>();
        let families: Vec<Family> = self
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| Family {
                name: entry.name.clone(),
                instance: index as u32,
                symbol: entry.symbol.clone(),
            })
            .collect();
        let manifest = Manifest::new(
            wrapper_bindings(&self.wrapper),
            program,
            build,
            families,
            &self.wrapper,
        )
        .render();
        let mut text = String::new();
        text.push_str("//! Generated by lichen: one entry per family.\n\n");
        text.push_str("use lichen_lowlevel::{ArrayItem, LowValue, Module, Program};\n");
        text.push_str("use lichen_utils::extend::AsEnum;\n");
        text.push_str("use std::ffi::c_void;\n\n");
        text.push_str(&format!("type Value = <{program} as Program>::Value;\n\n"));
        text.push_str(&self.items);
        text.push_str("\n/// The call's argument array.\n");
        text.push_str("pub fn arguments(operand: &Value) -> Vec<ArrayItem> {\n");
        text.push_str("    match AsEnum::<LowValue>::as_enum(operand) {\n");
        text.push_str("        // SAFETY: `array` is a live operand payload.\n");
        text.push_str(
            "        Some(LowValue::Array(array)) => unsafe { array.items() }.to_vec(),\n",
        );
        text.push_str("        _ => Vec::new(),\n");
        text.push_str("    }\n");
        text.push_str("}\n\n");
        text.push_str(&format!(
            "/// Argument `index`, evaluated.\npub fn item(\n\
             \x20   module: &mut Module<{program}>,\n\
             \x20   operand: &Value,\n\
             \x20   index: usize,\n\
             ) -> Option<Value> {{\n\
             \x20   let items = arguments(operand);\n\
             \x20   let item = items.get(index)?;\n\
             \x20   module.evaluate_node(item.node, None)\n\
             }}\n\n"
        ));
        text.push_str("const MANIFEST: &str = ");
        text.push_str(&format!("{manifest:?}"));
        text.push_str(";\n\n");
        text.push_str("#[unsafe(no_mangle)]\n");
        text.push_str("pub unsafe extern \"C\" fn lichen_link_manifest(out: *mut *const u8, len: *mut usize) {\n");
        text.push_str("    unsafe {\n");
        text.push_str("        *out = MANIFEST.as_ptr();\n");
        text.push_str("        *len = MANIFEST.len()\n");
        text.push_str("    };\n");
        text.push_str("}\n");
        for entry in &self.entries {
            text.push_str(&format!(
                "\n#[rustfmt::skip]\n\
                 #[unsafe(no_mangle)]\n\
                 pub unsafe extern \"C\" fn {symbol}(\n\
                 \x20   module: *mut c_void,\n\
                 \x20   operand: *const c_void,\n\
                 \x20   out: *mut c_void,\n\
                 ) {{\n\
                 \x20   let module = unsafe {{ &mut *(module as *mut Module<{program}>) }};\n\
                 \x20   let operand = unsafe {{ &*(operand as *const Value) }};\n\
                 \x20   let result: Value = {body};\n\
                 \x20   unsafe {{ (out as *mut Value).write(result) }};\n\
                 }}\n",
                symbol = entry.symbol,
                body = entry.body
            ));
        }
        text
    }
}
/// The names `source` binds at the top level, in source order.
///
/// # Invariant
/// A wrapper binds **one name per line at column zero**; an indented `=` is a
/// body statement, not a binding of the library's export.
fn wrapper_bindings(source: &str) -> Vec<String> {
    let mut bindings = Vec::new();
    for line in source.lines() {
        if line.starts_with(char::is_whitespace) {
            continue;
        }
        let Some((name, _)) = line.split_once(" = ") else {
            continue;
        };
        let valid = !name.is_empty()
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if valid {
            bindings.push(name.to_string());
        }
    }
    bindings
}

/// Resolve a `link "path"` against the source that names it, as an import path
/// resolves.
///
/// # Invariant
/// A relative path is relative to the source's directory, and a directory base
/// is its own base — the same rule [`crate::preprocess`] applies to imports.
pub fn resolve_path(base: Option<&std::path::Path>, path: &str) -> std::path::PathBuf {
    let path = std::path::Path::new(path);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    let base_dir = base
        .map(|base| {
            if base.is_dir() {
                base.to_path_buf()
            } else {
                base.parent()
                    .map(|parent| parent.to_path_buf())
                    .unwrap_or_else(|| std::path::PathBuf::from("."))
            }
        })
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    base_dir.join(path)
}
