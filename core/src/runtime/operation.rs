use crate::{
    plugin_define::Value,
    runtime::{Module, OperationId},
};

#[derive(Debug, Clone, Copy)]
pub enum Operation {
    None,
    Sum,
    Index,
    Find,
}

macro_rules! params {
    ($params:ident $(,$variant: path)*) => {{
        let Some(params) = $params.array() else {
            return None;
        };
        let params = params.as_ref();
        if params.len() != params!(@count $(,$variant)*) {
            return None;
        }
        let mut params = params.iter();
        ($({
            let param = *params.next().unwrap();
            let param = crate::runtime::Module::value(param);
            let Some(param) = $variant(param) else {
                return None;
            };
            param
        },)*)
    }};
    (@count) => (0);
    (@count, $variant0: path $(, $variant1: path)*) => (1 + params!(@count $(, $variant1)*));
}

impl Operation {
    pub fn run<V: Value>(self, expr_id: OperationId) -> Option<V> {
        let params = Module::<V>::value(expr_id);
        match self {
            Operation::Sum => {
                let Some(params) = params.array() else {
                    return None;
                };
                let mut ret = 0;
                for param in params.as_ref().iter().copied() {
                    let Some(int) = Module::<V>::value(param).int() else {
                        return None;
                    };
                    ret += int;
                }
                Some(V::from_int(ret))
            }
            Operation::Index => {
                let (array, index) = params!(params, V::array, V::int);
                let array = array.as_ref();
                if index >= array.len() as i64 || index < 0 {
                    return None;
                }
                Some(V::from_reference(*array.get(index as usize)))
            }
            Operation::Find => {
                let (table, name) = params!(params, V::table, V::string);
                let table = table.as_ref();
                let index = *table.get(name)?;
                Some(V::from_int(index as i64))
            }
            _ => None,
        }
    }
}
