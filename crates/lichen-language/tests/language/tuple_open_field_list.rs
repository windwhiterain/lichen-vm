//! The tuple printer's undecided-field-list arm; see
//! docs/notes/index-read-unify.md §7.6.

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
            Some(LangValue::from(LowValue::Array(handle))),
        )
    }

    /// `None` leaves the shape slot value-less, which is what opens the list.
    fn tuple_term(&mut self, fields: Option<&[AnyNodeId]>) -> NodeId {
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
    let report = compile("f = x => x");
    assert!(report.ok(), "the seed checks: {:?}", report.diagnostics);
    let build = report.build.expect("a build");
    Seed {
        block: build.module.node_block(build.root_ty),
        universe: build.type_expr,
        module: build.module,
    }
}

/// No program in the language builds this: `f4c2b93a` removed the only
/// front-end site that constructed one.
#[test]
fn an_undecided_field_list_reads_as_a_cell_beside_an_ellipsis() {
    // The open arm's spelling, which `cyclic_type_cut.rs` also reaches by a cut.
    let mut seed = seed();
    let term = seed.tuple_term(None);
    assert_eq!(print_type(&seed.module, term), "tuple{?a, …}");
}

/// A decided field list prints no ellipsis, so the `…` names the arm itself.
#[test]
fn a_decided_field_list_reads_without_an_ellipsis() {
    let mut seed = seed();
    let cell = seed.module.add_node(seed.block, None, None);
    let term = seed.tuple_term(Some(&[AnyNodeId::Dynamic(cell); 2]));
    assert_eq!(print_type(&seed.module, term), "tuple{?a, ?a}");
}
