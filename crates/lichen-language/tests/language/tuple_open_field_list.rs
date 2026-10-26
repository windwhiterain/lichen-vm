//! The tuple type is the shared layout template; the field list lives in the
//! value. See docs/notes/value-templates.md.

use lichen_highlevel::program::ValueType;
use lichen_language::compile;
use lichen_language::program::{LangProgram as P, LangValue};
use lichen_lowlevel::{AnyNodeId, ArrayItem, BlockId, LowValue, Module, NodeId};
use lichen_render::print_type;

/// A compiled module to add nodes to, and the block and universe to add them in.
struct Seed {
    module: Module<P>,
    block: BlockId,
    universe: NodeId,
}

impl Seed {
    /// A node holding the array `items`.
    fn array(&mut self, items: &[AnyNodeId]) -> NodeId {
        let handle = self.module.alloc_array(
            &items
                .iter()
                .copied()
                .map(ArrayItem::new)
                .collect::<Vec<_>>(),
            self.block,
        );
        self.module.add_node(
            self.block,
            None,
            Some(LangValue::from(LowValue::Array(handle.into()))),
        )
    }

    /// A retired `[shape, kind]` tuple term; `None` leaves its field list open.
    fn retired_tuple_term(&mut self, fields: Option<&[AnyNodeId]>) -> NodeId {
        let marker = self
            .module
            .add_node(self.block, None, Some(LangValue::tuple_type_marker()));
        let kind = self.array(&[
            AnyNodeId::Dynamic(marker),
            AnyNodeId::Dynamic(self.universe),
        ]);
        let shape = match fields {
            Some(items) => self.array(items),
            None => self.module.add_node(self.block, None, None),
        };
        self.array(&[AnyNodeId::Dynamic(shape), AnyNodeId::Dynamic(kind)])
    }
}

fn seed() -> Seed {
    let report = compile("f := x => x");
    assert!(report.ok(), "the seed checks: {:?}", report.diagnostics);
    let build = report.build.expect("a build");
    Seed {
        block: build.module.node_block(build.root_ty),
        universe: build.type_expr,
        module: build.module,
    }
}

/// A retired open field list prints as raw layout, not `tuple{…}`.
#[test]
fn a_retired_field_list_reads_as_the_raw_layout() {
    let mut seed = seed();
    let term = seed.retired_tuple_term(None);
    assert_eq!(print_type(&seed.module, term), "raw[?a, Tuple]");
}

/// The same retired term with a decided field list: the list is a raw array of
/// cells, and no ellipsis stands beside it.
#[test]
fn a_retired_field_list_with_fields_reads_as_the_raw_layout() {
    let mut seed = seed();
    let cell = seed.module.add_node(seed.block, None, None);
    let term = seed.retired_tuple_term(Some(&[AnyNodeId::Dynamic(cell); 2]));
    assert_eq!(print_type(&seed.module, term), "raw[raw[?a, ?a], Tuple]");
}
