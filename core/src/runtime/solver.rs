use std::marker::PhantomData;

use crate::{
    plugin_define::Value,
    runtime::{
        Module, OperationId,
        equation::Equation,
        value::{Auto, root_mut, solve_order},
    },
};

pub struct Solver<V: Value> {
    _p: PhantomData<V>,
}
impl<V: Value> Solver<V> {
    pub fn solve_equations<'a>(equations: impl Iterator<Item = &'a Equation>) {
        for equation in equations {
            Self::solve_equation(&equation.properties);
        }
    }
    pub fn solve_equation(operations: &[OperationId]) {
        let (mut max_value, mut max_order, mut max_operation) =
            (&mut V::from_none(), (0, 0), *operations.first().unwrap());
        for operation in operations.iter().copied() {
            let value = root_mut(Module::<V>::value_mut(operation));
            let order = solve_order(*value);
            if order > max_order {
                max_value = value;
                max_order = order;
                max_operation = operation;
            }
            if order.0 == 2 {
                break;
            }
        }
        if max_value.none() {
            *max_value = V::from_auto(Auto::new());
        }
        for operation in operations.iter().copied() {
            if operation == max_operation {
                continue;
            }
            let value = root_mut(Module::<V>::value_mut(operation));
            if let Some(max_auto) = max_value.auto() {
                if value.none() {
                    *max_value = V::from_auto(Auto {
                        referrer_count: max_auto.referrer_count + 1,
                    })
                } else if let Some(auto) = value.auto() {
                    *max_value = V::from_auto(Auto {
                        referrer_count: max_auto.referrer_count + auto.referrer_count,
                    });
                    *value = V::from_reference(max_operation);
                } else {
                    unreachable!()
                }
            } else {
                if solve_order(*value).0 == 2 {
                    if let Some(max_array) = max_value.array()
                        && let Some(array) = value.array()
                    {
                        let (max_array, array) = (max_array.as_ref(), array.as_ref());
                        assert!(max_array.len() == array.len());
                        for i in 0..max_array.len() {
                            Self::solve_equation(&[*max_array.get(i), *array.get(i)]);
                        }
                    } else {
                        panic!()
                    }
                } else {
                    *value = *max_value
                }
            }
        }
    }
}
