use lichen_core::{plugin::principal_traits::Value, runtime::NodeId, value::Array};

#[derive(Debug, Clone, Copy)]
pub struct Type {
    pub id: NodeId,
    pub params: Array,
    pub components: Array,
}

impl PartialEq for Type {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.params == other.params
    }
}

impl Eq for Type {}

impl Value for Type {}
