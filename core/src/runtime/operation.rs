use crate::{
    plugin::Project,
    plugin_define::Value,
    runtime::{OperationId, solve::Solver},
};

use std::fmt::Debug;

#[derive(Debug, Clone, Copy)]
pub struct Operation<P: Project> {
    pub param: OperationId<P>,
    pub operator: P::Operator,
}

pub trait Operator<P: Project>: Copy + Debug {
    fn run(self, param: P::Value, operation_id: OperationId<P>) -> Option<P::Value>;
}

macro_rules! params {
    ($params:ident, $operation_id:ident, $($variant: path,)*) => {{
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
            let param = Solver::solve_operation(param.project(),Some($operation_id))?;
            let Some(param) = $variant(param) else {
                return None;
            };
            param
        },)*)
    }};
    (@count) => (0);
    (@count, $variant0: path $(, $variant1: path)*) => (1 + params!(@count $(, $variant1)*));
}

pub fn sum<P: Project<Value: Value>>(
    param: P::Value,
    operation_id: OperationId<P>,
) -> Option<P::Value> {
    let Some(params) = param.array() else {
        return None;
    };
    let mut ret = 0;
    for param in params.as_ref().iter().copied() {
        let Some(int) = Solver::solve_operation(param.project(), Some(operation_id))?.int() else {
            return None;
        };
        ret += int;
    }
    Some(P::Value::from_int(ret))
}

pub fn index<P: Project<Value: Value>>(
    param: P::Value,
    operation_id: OperationId<P>,
) -> Option<P::Value> {
    let (array, index) = params!(param, operation_id, P::Value::array, P::Value::int,);
    let array = array.as_ref();
    if index >= array.len() as i64 || index < 0 {
        return None;
    }
    let reference_operation_id = array.get(index as usize).project();
    Solver::solve_equation(&[operation_id, reference_operation_id]);
    Solver::solve_operation(reference_operation_id, Some(operation_id))
}

pub fn find<P: Project<Value: Value>>(
    param: P::Value,
    operation_id: OperationId<P>,
) -> Option<P::Value> {
    let (table, name) = params!(param, operation_id, P::Value::table, P::Value::string,);
    let table = table.as_ref();
    let index = *table.get(name)?;
    Some(P::Value::from_int(index as i64))
}
