# LichenVM

Infrastructure for custom type checker.

## Highlevel

Built program from lowlevel from IR (intermediate representation), checking and runtime are unified.

## Features:

- Hindley-Milner System: with let-polymorphism.
- Dependent Type: via laziness.
- First Class Type: type instantiation via partial function application.

## Lowlevel

Interpreter that type and value are same thing.

### Features:

- Lazy Evaluation.
- Block Level Garbage Collection.
- Lmbda Calculas: first class function, higher order function, closure.
- Recursive Function Apply.
- Function Normalization.
- Unification.

### Philosophy:

- Minimal Memory Usage.
- Minimal Allocation.
- High Speed For Trivial Program (complex one will be JIT compiled in the future).

