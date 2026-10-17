//! Rendering: the output value and the diagnostics share one pretty printer.
//! See docs/notes/raw-rendering-mark.md.
use lichen_highlevel::diagnostic::{AssertSpelling, Diag as CheckerDiag, DiagKind};
use lichen_highlevel::program::{HighProgram, ValueType};
use lichen_lowlevel::{AnyNodeId, BudgetExhausted, LowValue, Module, NodeId, ValueExt};

use crate::LangProgramShape;
use crate::diag::Diag;
use crate::lang_attr_ext;
use lichen_utils::extend::AsEnum;

pub use lichen_render::{
    TypePrinter, ValuePrinter, print_type, print_value, render_attributes,
    render_struct_fields_named, struct_type_named_fields, value_label,
};

// --- the extension-vocabulary render hook -----------------------------------

/// The `&'static` hook injected into the shared printers for this vocabulary.
///
/// # Invariant
///
/// The hook is the program's own [`LangProgramShape::value_name`], so the
/// language never names a leaf it was not composed over and a host's spelling
/// reaches every printer without a call site naming it.
pub fn lang_render_ext<P>() -> &'static dyn Fn(&P::Value) -> Option<String>
where
    P: LangProgramShape,
{
    &P::value_name
}

/// [`print_type`] with the language's extension vocabulary.
pub fn print_type_lang<P>(module: &Module<P>, root: NodeId) -> String
where
    P: LangProgramShape,
    P::Value: ValueType,
    P::Operator: From<crate::program::GcdOp> + From<lichen_lowlevel::LowOperator>,
{
    let attrs = lang_attr_ext::<P>();
    let mut printer = TypePrinter::new_with_ext(module, Some(lang_render_ext::<P>()));
    printer.show_attributes(&*attrs);
    printer.node(root)
}

/// [`print_value`] with the language's extension vocabulary.
pub fn print_value_lang<P>(module: &Module<P>, value: P::Value, ty: NodeId) -> String
where
    P: LangProgramShape,
    P::Value: ValueType,
    P::Operator: From<crate::program::GcdOp> + From<lichen_lowlevel::LowOperator>,
{
    let attrs = lang_attr_ext::<P>();
    let mut printer = ValuePrinter::new_with_ext(module, Some(lang_render_ext::<P>()));
    printer.show_attributes(&*attrs);
    printer.print(value, ty)
}

// --- the caret shell ---------------------------------------------------------

/// Render a diagnostic with its source line and a caret.
///
/// # Invariant
///
/// A diagnostic with no span prints its message alone, with no position and no
/// caret.
pub fn render<P: lichen_lowlevel::Program>(source: &str, diag: &Diag<P>) -> String {
    let starts = crate::lex::line_starts(source);
    render_with_line_starts(source, &starts, diag)
}

/// [`render`] against an already-computed line model, so a whole report costs
/// one scan of the source.
fn render_with_line_starts<P: lichen_lowlevel::Program>(
    source: &str,
    starts: &[usize],
    diag: &Diag<P>,
) -> String {
    let document_starts = starts;
    let mut out = format!("error: {}\n", diag.message);
    let Some((line, col)) = diag.span else {
        return out;
    };
    // A foreign position carries that file's path and its own line model, so
    // text and `(line, col)` agree.
    let foreign_starts = diag
        .file
        .as_ref()
        .map(|package| crate::lex::line_starts(&package.code));
    let (text, starts, arrow) = match (diag.file.as_ref(), foreign_starts.as_deref()) {
        (Some(package), Some(starts)) => (
            package.code.as_ref(),
            starts,
            format!("{}:{line}:{col}", package.path.display()),
        ),
        _ => (source, starts, format!("{line}:{col}")),
    };
    out.push_str(&format!("  --> {arrow}\n"));
    out.push_str("   |\n");
    if let Some(text) = crate::lex::line_text(text, starts, line) {
        let caret = format!("{}^", " ".repeat((col as usize).saturating_sub(1)));
        out.push_str(&format!(" {line} | {text}\n"));
        out.push_str(&format!("   | {caret}\n"));
    }
    // The failing use is in this document even when the failure's line is elsewhere.
    if let Some((line, col)) = diag.related {
        out.push_str("note: the failing use is here\n");
        out.push_str(&format!("  --> {line}:{col}\n"));
        out.push_str("   |\n");
        if let Some(text) = crate::lex::line_text(source, document_starts, line) {
            let caret = format!("{}^", " ".repeat((col as usize).saturating_sub(1)));
            out.push_str(&format!(" {line} | {text}\n"));
            out.push_str(&format!("   | {caret}\n"));
        }
    }
    out
}

/// A **class domain** — a set's value, its member type values — spelled
/// `{Int, Float}`.
///
/// # Invariant
///
/// The domain is read as the member list it is; a node that is not a member
/// list falls back to the type printer.
fn class_domain<P>(printer: &mut TypePrinter<'_, P>, domain: NodeId) -> String
where
    P: HighProgram,
    P::Value: ValueType,
{
    let members = lichen_highlevel::set::members(
        printer.module(),
        lichen_lowlevel::AnyNodeId::Dynamic(domain),
    );
    match members {
        Some(members) => format!(
            "{{{}}}",
            members
                .into_iter()
                .map(|member| printer.any_node(member))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        None => printer.node(domain),
    }
}

/// The spelling a non-`Condition` assert failure reads, rendered through `printer`.
///
/// # Invariant
/// `None` only for [`AssertSpelling::Condition`], whose message needs the failure's
/// own value; every other spelling is a fact of the assertion and is the same on
/// every failure of that template (`docs/notes/core-prelude.md`).
pub fn assert_spelling_text<P>(
    printer: &mut TypePrinter<'_, P>,
    spelling: AssertSpelling,
) -> Option<String>
where
    P: HighProgram,
    P::Value: ValueType,
{
    match spelling {
        AssertSpelling::Condition => None,
        // A refinement: the value is outside the admitted class set.
        // See docs/notes/operator-polymorphism.md §8.5.
        AssertSpelling::Refinement { domain } => Some(format!(
            "does not satisfy {}",
            class_domain(printer, domain)
        )),
        // The deferred half of the container-kind requirement, in the read's own
        // vocabulary.
        AssertSpelling::StructKind { container } => Some(format!(
            "expected a struct type, found {}",
            printer.node(container)
        )),
        // The positional read's half of the same requirement.
        AssertSpelling::TupleKind { container } => Some(format!(
            "expected a tuple type, found {}",
            printer.node(container)
        )),
        // The index read's half.
        AssertSpelling::ArrayKind { container } => Some(format!(
            "expected an array type, found {}",
            printer.node(container)
        )),
    }
}

/// Render a whole diagnostic list back to back: one caret block each.
pub fn render_all<P: lichen_lowlevel::Program>(source: &str, diags: &[Diag<P>]) -> String {
    let starts = crate::lex::line_starts(source);
    diags
        .iter()
        .map(|d| render_with_line_starts(source, &starts, d))
        .collect()
}

// --- the pretty checker message ----------------------------------------------

// Re-render the highlevel's facts in the CLI's vocabulary, through one shared
// printer so a class keeps one `?a` name.

/// Re-render a checker diagnostic's message from the highlevel's facts.
///
/// # Invariant
///
/// `printer` is shared across a whole report, so a class keeps one `?a` name.
pub fn checker_message<P>(printer: &mut TypePrinter<'_, P>, d: &CheckerDiag<P>) -> String
where
    P: HighProgram + 'static,
    P::Value: ValueType,
{
    match d.kind {
        DiagKind::Annotation
        | DiagKind::Attribute
        | DiagKind::ArrayElement
        | DiagKind::TableKey
        | DiagKind::TableValue
        | DiagKind::Guard => {
            format!(
                "expected {}, found {}",
                printer.node(d.b),
                printer.node(d.a)
            )
        }

        DiagKind::RuntimeIndexTarget => {
            "this value is not a container — it has no element to read".to_string()
        }

        DiagKind::RuntimeTypeAccessElement => {
            "this type access found an element that is not a value/type pair — \
             `X{n}` and `X::a` read the element's own pair, so the container \
             holds pairs (a type value); a runtime array's element is read with \
             `e[i]`"
                .to_string()
        }
        DiagKind::RuntimeIndexSubscript => {
            "this value is not an index — an element can only be read by position".to_string()
        }
        DiagKind::RuntimeApplyTarget => {
            "this value is not a function — it cannot be applied".to_string()
        }
        DiagKind::ImportExport => {
            "this package's export is not a value — a package must end in a value".to_string()
        }
        DiagKind::NamedField => {
            let fields = struct_type_named_fields(printer.module(), d.a);
            let container = match fields {
                Some(_) => printer.node(d.a),
                None => container_spelling(printer, d),
            };
            let base = format!("no field with this name in the struct type {container}");
            // A value container carries no type term, so its table supplies the clause.
            let Some(fields) = fields else {
                let names = kind_name_table(printer.module(), d.b)
                    .and_then(|table| table_names(printer.module(), table));
                let clause = match (d.field.as_deref(), names) {
                    (Some(name), Some(names)) => crate::suggest::did_you_mean(name, names),
                    _ => None,
                };
                return match clause {
                    Some(clause) => format!("{base}{clause}"),
                    None => base,
                };
            };
            with_field_suggestion(d, base, fields)
        }
        DiagKind::StructUnknownField => {
            let base = match &d.field {
                Some(name) => format!(
                    "no field named {name} in the struct type {}",
                    printer.node(d.a)
                ),
                None => format!("no such field in the struct type {}", printer.node(d.a)),
            };
            let Some(fields) = struct_type_named_fields(printer.module(), d.a) else {
                return base;
            };
            with_field_suggestion(d, base, fields)
        }
        DiagKind::StructDuplicateField => match &d.field {
            Some(name) => format!(
                "duplicate field {name} in the struct type {}",
                printer.node(d.a)
            ),
            None => format!("duplicate field in the struct type {}", printer.node(d.a)),
        },
        DiagKind::StructMissingField => match &d.field {
            Some(name) => format!(
                "missing field {name} in the struct type {}",
                printer.node(d.a)
            ),
            None => format!("missing a field in the struct type {}", printer.node(d.a)),
        },
        DiagKind::StructExcessField => {
            format!("too many fields for the struct type {}", printer.node(d.a))
        }
        DiagKind::StructAnonymousField => {
            "cannot name a field — the struct has no named fields".to_string()
        }
        DiagKind::StructFieldName => {
            "a struct field must be named — write `struct{.name T}`".to_string()
        }
        DiagKind::InstantiateCallee => format!(
            "the callee of an instantiation must be a struct type, found {}",
            printer.node(d.a)
        ),
        // The expected side is the class the operation computes over, read from
        // the report rather than spelled here.
        DiagKind::BinOp | DiagKind::Conv => format!(
            "expected {}, found {}",
            printer.node(d.b),
            printer.node(d.a)
        ),
        // A runtime apply-time failure: the parameter is the expected side
        // (a), the argument the found side (b).
        DiagKind::Runtime => {
            let expected = printer.node(d.a);
            let found = printer.node(d.b);
            // Two open cells name neither side; the collision's two values do.
            if expected == "?" && found == "?" {
                let named = |value: &Option<P::Value>| match value {
                    Some(value) if !value.is_handle() => Some(format!("{value:?}")),
                    _ => None,
                };
                match (named(&d.value_a), named(&d.value_b)) {
                    (Some(expected), Some(found)) => format!(
                        "expected ?, found ? — two answers met on one node: {expected} against \
                         {found} (`docs/notes/answer-collision.md`)"
                    ),
                    _ => "expected ?, found ? — two answers met on one node \
                          (`docs/notes/answer-collision.md`)"
                        .to_string(),
                }
            } else {
                format!("expected {expected}, found {found}")
            }
        }
        DiagKind::IndexOutOfBounds => {
            let (Some(index), Some(length)) = (d.index, d.length) else {
                return "index out of bounds".to_string();
            };
            format!("index {index} out of bounds (array length {length})")
        }
        DiagKind::TableMiss => match table_key_text(d) {
            Some(key) => format!("table lookup missed — no entry for key {key}"),
            None => "table lookup missed — no entry for this key".to_string(),
        },
        // The report invariant's last resort: the build failed with nothing to
        // attribute it to.
        DiagKind::UnattributedFailure => {
            "the build failed, but the failing check could not be attributed to an expression in this source".to_string()
        }
        DiagKind::TableKeyUndecided => {
            "table key is not concrete (it is undecided or a failed read) — the entry is dropped"
                .to_string()
        }
        DiagKind::NativeOpUnresolved => match d.field.as_deref() {
            Some(name) => format!(
                "unresolved native operator '{name}' — this module composes no plugin registering it"
            ),
            None => "unresolved native operator — this module composes no plugin registering it"
                .to_string(),
        },
        DiagKind::NativeOpContract => match d.field.as_deref() {
            Some(name) => format!(
                "native operator '{name}' returned a malformed term — a native operator must return the [value, type] pair it built in the current block"
            ),
            None => "a native operator returned a malformed term — a native operator must return the [value, type] pair it built in the current block"
                .to_string(),
        },
        DiagKind::ExtensionRule => match d.field.as_deref() {
            Some(sentence) => sentence.to_string(),
            None => "a layer above the language refused this definition".to_string(),
        },
        DiagKind::NoAttributeExtension => {
            "this expression carries an attribute, but this build has no attribute extension to lower it"
                .to_string()
        }
        DiagKind::Assert => d
            .assert_spelling
            .and_then(|spelling| assert_spelling_text(printer, spelling))
            .unwrap_or_else(|| {
                // An explicit `@assert e`: the condition's own failed value.
                let value = match d.assert_value.as_ref().and_then(|v| v.as_enum()) {
                    Some(LowValue::USize(n)) => n.to_string(),
                    Some(LowValue::None | LowValue::Error) => "none".to_string(),
                    Some(other) => format!("{other:?}"),
                    None => "—".to_string(),
                };
                format!("assertion failed: expected 1, found {value}")
            }),
        DiagKind::NonTerminating => match d.budget {
            // The recorded guard and its limit name the runaway shape exactly.
            Some(BudgetExhausted::ApplyDepth { limit }) => format!(
                "the checker stopped on its apply-depth budget: nested applications exceeded {limit} levels. The program may still terminate when run."
            ),
            Some(BudgetExhausted::ApplyTotal { limit }) => format!(
                "the checker stopped on its work budget: it applied a function more than {limit} times. The program may still terminate when run."
            ),
            Some(BudgetExhausted::EvaluateDepth { limit }) => format!(
                "the checker stopped on its evaluate-depth budget: its value grows deeper than {limit} levels. The program may still terminate when run."
            ),
            None => "the checker stopped on a budget before it could decide this binding. The program may still terminate when run.".to_string(),
        },
        DiagKind::LoopNotRecorded => {
            // The wording names what the program said and what is missing, so a
            // refusal is not read as a miscompile.
            match &d.field {
                Some(rule) => format!(
                    "this call is a recursion of a `@loop`-marked binding whose trip \
                     count is not decided before the body is lowered, and whose shape \
                     does not convert to a loop — it is {rule} — so the call is refused \
                     rather than expanded"
                ),
                None => "this call is a recursion of a `@loop`-marked binding whose trip \
                         count is not decided before the body is lowered — no loop has \
                         been recorded for it, so the call is refused rather than expanded"
                    .to_string(),
            }
        }
        DiagKind::LoopNotEmitted => {
            // The shape is a loop but no backend consumes it yet: the compiler
            // cannot do this yet, not the program being wrong.
            "this call is a recursion of a `@loop`-marked binding whose trip count is \
             not decided before the body is lowered; its shape converts to a loop, but \
             no backend emits one yet, so the call is refused rather than expanded"
                .to_string()
        }
    }
}

/// `base` plus a did-you-mean clause over the struct type's own field names.
fn with_field_suggestion<P>(
    d: &CheckerDiag<P>,
    base: String,
    fields: Vec<Option<&'static str>>,
) -> String
where
    P: HighProgram,
{
    let Some(name) = d.field.as_deref() else {
        return base;
    };
    match crate::suggest::did_you_mean(name, fields.into_iter().flatten()) {
        Some(clause) => format!("{base}{clause}"),
        None => base,
    }
}

/// The struct type's name table, read from the kind path (`kind[0][0][1]`).
fn kind_name_table<P: HighProgram>(module: &Module<P>, kind: NodeId) -> Option<AnyNodeId>
where
    P::Value: ValueType,
{
    let at = |node: AnyNodeId, index: usize| -> Option<AnyNodeId> {
        // SAFETY: live module payloads, read only; no block is released here.
        unsafe { module.array_items_of(node) }
            .and_then(|items| items.get(index))
            .map(|item| item.node)
    };
    let marker = at(AnyNodeId::Dynamic(kind), 0)?;
    let payload = at(marker, 0)?;
    at(payload, 1)
}

/// The names in a struct type's name table, in the table's own order.
fn table_names<P: HighProgram>(module: &Module<P>, table: AnyNodeId) -> Option<Vec<&'static str>>
where
    P::Value: ValueType,
{
    let LowValue::Table(table) = module.node_value(table)?.as_enum()? else {
        return None;
    };
    // SAFETY: the table is a live module payload, read only; no block is released here.
    let items = unsafe { table.items() };
    Some(
        items
            .iter()
            .filter_map(|item| match module.node_value(item.key)?.as_enum()? {
                LowValue::Str(name) => Some(name),
                _ => None,
            })
            .collect(),
    )
}

/// A refused named read's container: the type printer, or a struct type value's value print.
fn container_spelling<P>(printer: &mut TypePrinter<'_, P>, d: &CheckerDiag<P>) -> String
where
    P: HighProgram + 'static,
    P::Value: ValueType,
{
    let module = printer.module();
    let spelled = module
        .node_value(AnyNodeId::Dynamic(d.a))
        .map(|value| (value, d.b))
        .map(|(value, ty)| print_value(module, value, ty));
    spelled.unwrap_or_else(|| printer.node(d.a))
}

/// The missed read's key in the language's spelling; `None` for a non-scalar key
/// or a key that held no value.
fn table_key_text<P>(d: &CheckerDiag<P>) -> Option<String>
where
    P: HighProgram,
    P::Value: ValueType,
{
    match d.table_key.as_ref().and_then(|key| key.as_enum())? {
        LowValue::USize(n) => Some(n.to_string()),
        LowValue::Str(s) => Some(format!("{s:?}")),
        LowValue::Float(f) => Some(f.to_string()),
        _ => None,
    }
}

#[cfg(test)]
#[path = "tests/render_tests.rs"]
mod tests;
