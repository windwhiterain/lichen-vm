use crate::module::{Module, OperationId, equation::Equation, value::Value};

pub struct Solver {}
impl Solver {
    pub fn solve_equations<'a>(equations: impl Iterator<Item = &'a Equation>) {
        for equation in equations {
            Self::solve_equation(&equation.properties);
        }
    }
    pub fn solve_equation(operations: &[OperationId]) {
        let (mut max_value, mut max_order, mut max_operation) =
            (&mut Value::UnSolved, (0, 0), *operations.first().unwrap());
        for operation in operations.iter().copied() {
            let value = Module::value_mut(operation).root_mut();
            let order = value.solve_order();
            if order > max_order {
                max_value = value;
                max_order = order;
                max_operation = operation;
            }
            if order.0 == 2 {
                break;
            }
        }
        if let Value::UnSolved = max_value{
            *max_value = Value::AUTO; 
        }
        for operation in operations.iter().copied() {
            if operation == max_operation {
                continue;
            }
            let value = Module::value_mut(operation).root_mut();
            match max_value {
                Value::Auto {
                    referrer_count: max_referrer_count,
                } => match *value {
                    Value::UnSolved => {
                        *max_referrer_count += 1;
                        *value = Value::Ref(max_operation);
                    }
                    Value::Auto { referrer_count } => {
                        *max_referrer_count += referrer_count;
                        *value = Value::Ref(max_operation);
                    }
                    _ => unreachable!(),
                },
                ref mut max_value => {
                    if value.solve_order().0 == 2 {
                        match (max_value, value) {
                            (Value::Array(max_array), Value::Array(array)) => {
                                let (max_array, array) = (max_array.as_ref(), array.as_ref());
                                assert!(max_array.len() == array.len());
                                for i in 0..max_array.len() {
                                    Self::solve_equation(&[
                                        *max_array.get(i),
                                        *array.get(i),
                                    ]);
                                }
                            }
                            _ => panic!(),
                        }
                    } else {
                        *value = **max_value
                    }
                }
            }
        }
    }
}

