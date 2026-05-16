use lichen_utils::arena::{array::ArenaArray, hashmap::ArenaHashMap};

use crate::{
    plugin::Project,
    runtime::{Module, OperationId, OperationIdRaw, Ptr, StringId, solve::Solver},
};

#[derive(Debug, Clone, Copy)]
pub enum Evaluation<P: Project> {
    Value(P::Value),
    Ref {
        id: OperationId<P>,
        brother: Option<OperationId<P>>,
    },
    Auto {
        referrer_count: usize,
        reference: Option<(OperationId<P>, OperationId<P>)>,
    },
}

pub type Int = i64;
pub type Array = ArenaArray<OperationIdRaw>;
pub type Table = ArenaHashMap<StringId, usize>;

pub fn new_array<P: Project>(
    module: &mut Module<P>,
    operations: impl Iterator<Item = OperationId<P>>,
) -> Array {
    Array::from_iter(&mut module.arena, operations.map(|x| x.raw()))
}
pub fn new_table<P: Project>(
    module: &mut Module<P>,
    entries: impl Iterator<Item = (StringId, usize)>,
) -> Table {
    Table::from_iter(&mut module.arena, entries)
}

impl<P: Project> Evaluation<P> {
    pub const AUTO: Self = Self::Auto {
        referrer_count: 1,
        reference: None,
    };
    pub fn evaluation_order(self) -> (usize, usize) {
        match self {
            Evaluation::Value(_) => (2, 0),
            Evaluation::Ref { id, .. } => id.evaluation().evaluation_order(),
            Evaluation::Auto { referrer_count, .. } => (1, referrer_count),
        }
    }
}

impl<P: Project> OperationId<P> {
    pub fn root(self) -> OperationId<P> {
        if let Evaluation::Ref { id, .. } = self.evaluation_mut() {
            let ret = id.root();
            *id = ret;
            ret
        } else {
            self
        }
    }
    pub fn set_value(self, value: P::Value) {
        let evaluation = self.evaluation_mut();
        let Evaluation::Auto { reference, .. } = *evaluation else {
            panic!()
        };
        let mut reference_iter = reference.map(|x| x.0);
        while let Some(reference) = reference_iter {
            let evaluation = reference.evaluation_mut();
            let Evaluation::Ref { brother, .. } = *evaluation else {
                unreachable!();
            };
            reference_iter = brother;
            Solver::set_operation_value(reference, value);
        }
        Solver::set_operation_value(self, value);
    }
    pub fn set_ref(self, id: OperationId<P>) {
        let evaluation = self.evaluation_mut();
        let Evaluation::Auto {
            referrer_count: self_referrer_count,
            reference: self_reference,
            ..
        } = *evaluation
        else {
            panic!()
        };
        let Evaluation::Auto {
            referrer_count,
            reference,
        } = id.evaluation_mut()
        else {
            panic!()
        };
        *referrer_count += self_referrer_count;
        if let Some(self_reference) = self_reference {
            if let Some(reference) = reference {
                let Evaluation::Ref { brother, .. } = self_reference.1.evaluation_mut() else {
                    unreachable!()
                };
                *brother = Some(reference.0);
                reference.0 = self;
            }
            *evaluation = Evaluation::Ref {
                id,
                brother: Some(self_reference.0),
            };
        } else {
            if let Some(reference) = reference {
                *evaluation = Evaluation::Ref {
                    id,
                    brother: Some(reference.0),
                };
                reference.0 = self;
            } else {
                *evaluation = Evaluation::Ref { id, brother: None };
            }
        }
    }
}
