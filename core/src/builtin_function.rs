use crate::module::{Module, OperationId, value::Value};

pub enum BuiltinFunction {
    Index,
    Find,
}

macro_rules! count {
    () => (0);
    (,$variant0: path $(, $variant1: path)*) => (1 + count!($(, $variant1)*));
}

macro_rules! params {
    ($params:ident $(,$variant: path)*) => {{
        let crate::module::value::Value::Array(params) = $params else {
            return None;
        };
        let params = params.as_ref();
        if params.len() != count!($(,$variant)*) {
            return None;
        }
        let mut params = params.iter();
        ($({
            let param = *params.next().unwrap();
            let param = Module::value(param);
            let $variant(param) = param else {
                return None;
            };
            param
        },)*)
    }};

}

impl BuiltinFunction {
    pub fn run(self, expr_id: OperationId) -> Option<Value> {
        let params = Module::value(expr_id);
        match self {
            BuiltinFunction::Index => {
                let (array, index) = params!(params, Value::Array, Value::Int);
                let array = array.as_ref();
                if index >= array.len() as i64 || index < 0 {
                    return None;
                }
                Some(Value::Ref(*array.get(index as usize)))
            }
            BuiltinFunction::Find => {
                let (table, name) = params!(params, Value::Table, Value::String);
                let table = table.as_ref();
                let index = *table.get(name)?;
                Some(Value::Int(index as i64))
            }
        }
    }
}
