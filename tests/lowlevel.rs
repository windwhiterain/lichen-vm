use bumpalo::Bump;
use lichen_vm::lowlevel::{Block, Operation, Operator, OperatorExt, Program, Value, ValueExt};

#[derive(Clone, Copy)]
struct TestProgram;

impl Program for TestProgram {
    type Value = TestValue;
    type Operator = TestOperator;
}

/// Extension values carrying typed pointers into the block's arena.  The
/// arena machinery only deals in byte views, so `ValueExt` converts
/// between the typed pointer and a byte slice on the fly.
#[derive(Clone, Copy)]
enum TestValue {
    /// A `u128` payload (16 bytes).
    U128(*const u128),
    /// A `char` payload, four bytes per char.
    String(*const [char]),
}
impl ValueExt for TestValue {
    fn is_ptr() -> bool {
        true
    }
    // `U128` requires 16-byte alignment; returning the strictest
    // alignment keeps the `String` copies over-aligned, which is safe.
    fn alignment() -> usize {
        16
    }
    fn ptr(&self) -> *const [u8] {
        match self {
            TestValue::U128(p) => std::ptr::slice_from_raw_parts(*p as *const u8, 16),
            TestValue::String(p) => {
                let chars = unsafe { &**p };
                std::ptr::slice_from_raw_parts(chars.as_ptr() as *const u8, chars.len() * 4)
            }
        }
    }
    fn set_ptr(&mut self, ptr: *const [u8]) {
        match self {
            TestValue::U128(p) => *p = ptr as *const u128,
            TestValue::String(p) => {
                *p = std::ptr::slice_from_raw_parts(ptr as *const char, ptr.len() / 4)
            }
        }
    }
}

#[derive(Clone, Copy)]
enum TestOperator {
    Add,
    Concat,
}

/// A block spec for tests.  Constant nodes (`U128`, `Str`, `Array`) are
/// pre-seeded values whose node has no operation; `Op` nodes carry an
/// operator and an optional operand node id.
#[derive(Clone, Copy)]
enum Node {
    Op(Operator<TestProgram>, Option<usize>),
    U128(u128),
    Str(&'static [char]),
    Array(&'static [usize]),
}

fn u128_of(value: Value<TestProgram>) -> u128 {
    let Value::Ext(TestValue::U128(ptr)) = value else {
        panic!("expected U128")
    };
    u128::from_ne_bytes(
        unsafe { std::slice::from_raw_parts(ptr as *const u8, 16) }
            .try_into()
            .unwrap(),
    )
}

fn string_of(value: Value<TestProgram>) -> Vec<char> {
    let Value::Ext(TestValue::String(ptr)) = value else {
        panic!("expected String")
    };
    unsafe { &*ptr }.to_vec()
}

impl OperatorExt<TestProgram> for TestOperator {
    fn run(
        &self,
        operand: Value<TestProgram>,
        block: &mut Block<TestProgram>,
    ) -> Value<TestProgram> {
        // Binary ops receive their operands as an array of two node ids;
        // the elements are already evaluated.
        let Value::Array(operands) = operand else {
            unreachable!("Add/Concat expect an array of two node ids")
        };
        let operands = unsafe { &*operands };
        let left = block.values[operands[0]].unwrap();
        let right = block.values[operands[1]].unwrap();
        match self {
            TestOperator::Add => {
                let sum = u128_of(left).wrapping_add(u128_of(right));
                let p = block.arena.alloc(sum);
                Value::Ext(TestValue::U128(p as *const u128))
            }
            TestOperator::Concat => {
                let mut result = string_of(left);
                result.extend(string_of(right));
                let slice = block.arena.alloc_slice_copy(&result);
                Value::Ext(TestValue::String(std::ptr::slice_from_raw_parts(
                    slice.as_ptr(),
                    slice.len(),
                )))
            }
        }
    }
}

fn block(nodes: &[Node]) -> Box<Block<TestProgram>> {
    let mut b = Box::new(Block {
        arena: Bump::new(),
        operations: Vec::new(),
        values: Vec::new(),
    });
    for node in nodes {
        match *node {
            Node::Op(operator, operand) => {
                b.operations.push(Some(Operation { operator, operand }));
                b.values.push(None);
            }
            // Constants are pre-seeded values with no operation, so
            // `run_node` returns them without evaluating anything.
            Node::U128(n) => {
                let p = b.arena.alloc(n);
                b.operations.push(None);
                b.values
                    .push(Some(Value::Ext(TestValue::U128(p as *const u128))));
            }
            Node::Str(chars) => {
                let slice = b.arena.alloc_slice_copy(chars);
                b.operations.push(None);
                b.values.push(Some(Value::Ext(TestValue::String(
                    std::ptr::slice_from_raw_parts(slice.as_ptr(), slice.len()),
                ))));
            }
            Node::Array(ids) => {
                let slice = b.arena.alloc_slice_copy(ids);
                b.operations.push(None);
                b.values
                    .push(Some(Value::Array(std::ptr::slice_from_raw_parts(
                        slice.as_ptr(),
                        slice.len(),
                    ))));
            }
        }
    }
    b
}

fn assert_array(value: Value<TestProgram>, expected: &[usize]) {
    let Value::Array(ptr) = value else {
        panic!("expected array")
    };
    assert_eq!(unsafe { &*ptr }, expected);
}

#[test]
fn redundant_nodes_are_not_compacted() {
    let mut parent = block(&[Node::U128(0)]);
    let mut child = block(&[Node::U128(5), Node::U128(7)]); // node 1: redundant

    let value = child.run(&mut parent);

    assert_eq!(u128_of(value), 5);
    assert_eq!(parent.values.len(), 1); // scalar return: nothing compacted into parent
}

#[test]
fn u128_payload_is_relocated_into_parent_and_block_releasable() {
    let mut parent = block(&[Node::U128(0)]);
    // Marker allocated in parent's arena before the child runs.  Bumpalo
    // allocates downward within a chunk, so later allocations sit at
    // lower addresses.
    let marker = parent.arena.alloc_slice_copy(b"marker");
    let marker_start = marker.as_ptr() as usize;

    let mut child = block(&[Node::U128(42)]);
    let value = child.run(&mut parent);
    let Value::Ext(TestValue::U128(ptr)) = value else {
        panic!("expected U128")
    };
    assert_eq!(u128_of(value), 42);
    // Relocated into parent's arena: the copy was made after the marker,
    // so it sits below it in the same chunk.
    assert!(ptr as *const u8 as usize + 16 <= marker_start);

    // The child block can be released; the value still points into
    // parent's arena.
    drop(child);
    assert_eq!(u128_of(value), 42);
}

#[test]
fn add_sums_u128_operands() {
    let mut parent = block(&[Node::U128(0)]);
    let mut child = block(&[
        Node::Op(Operator::Ext(TestOperator::Add), Some(3)), // node 0: 3 + 4
        Node::U128(3),                                       // node 1
        Node::U128(4),                                       // node 2
        Node::Array(&[1, 2]),                                // node 3: operands
    ]);

    let value = child.run(&mut parent);

    assert_eq!(u128_of(value), 7);
}

#[test]
fn concat_joins_string_operands() {
    let mut parent = block(&[Node::U128(0)]);
    let mut child = block(&[
        Node::Op(Operator::Ext(TestOperator::Concat), Some(3)), // node 0: "ab" + "cd"
        Node::Str(&['a', 'b']),                                 // node 1
        Node::Str(&['c', 'd']),                                 // node 2
        Node::Array(&[1, 2]),                                   // node 3: operands
    ]);

    let value = child.run(&mut parent);

    assert_eq!(string_of(value), vec!['a', 'b', 'c', 'd']);
}

#[test]
fn array_return_compacts_elements_into_parent() {
    let mut parent = block(&[Node::U128(0)]);
    let mut child = block(&[
        Node::Array(&[1, 2, 3]), // node 0: return [1, 2, 3]
        Node::U128(10),
        Node::U128(20),
        Node::U128(30),
    ]);

    let value = child.run(&mut parent);

    // ids remapped to slots appended after parent's own node 0
    assert_array(value, &[1, 2, 3]);
    assert_eq!(parent.values.len(), 4);
    assert_eq!(u128_of(parent.values[1].unwrap()), 10);
    assert_eq!(u128_of(parent.values[2].unwrap()), 20);
    assert_eq!(u128_of(parent.values[3].unwrap()), 30);

    // Elements were relocated along with their payloads, so they stay
    // readable after the child block is released.
    drop(child);
    assert_eq!(u128_of(parent.values[1].unwrap()), 10);
}

#[test]
fn nested_scalar_return_compacts_into_grandparent() {
    let mut grandparent = block(&[Node::U128(0)]);

    let mut inner = block(&[Node::U128(9)]);
    let inner_ptr: *mut Block<TestProgram> = &mut *inner;

    let mut outer = block(&[
        Node::Array(&[1]),                          // node 0: return [node 1]
        Node::Op(Operator::Block(inner_ptr), None), // node 1
    ]);

    let value = outer.run(&mut grandparent);

    assert_array(value, &[1]);
    assert_eq!(grandparent.values.len(), 2);
    assert_eq!(u128_of(grandparent.values[1].unwrap()), 9);
}

#[test]
fn nested_array_return_remaps_ids_twice() {
    let mut grandparent = block(&[Node::U128(0)]);

    let mut inner = block(&[
        Node::Array(&[1]), // node 0: return [node 1]
        Node::U128(7),     // node 1
    ]);
    let inner_ptr: *mut Block<TestProgram> = &mut *inner;

    let mut outer = block(&[
        Node::Array(&[1]),                          // node 0: return [node 1]
        Node::Op(Operator::Block(inner_ptr), None), // node 1
    ]);

    let value = outer.run(&mut grandparent);

    // inner's array was remapped to outer's ids first (2), then to
    // grandparent's (2); the element lands at grandparent id 1.
    assert_array(value, &[2]);
    assert_eq!(grandparent.values.len(), 3);
    assert_eq!(u128_of(grandparent.values[1].unwrap()), 7);
    let Value::Array(ptr) = grandparent.values[2].unwrap() else {
        unreachable!()
    };
    assert_eq!(unsafe { &*ptr }, &[1]);
}
