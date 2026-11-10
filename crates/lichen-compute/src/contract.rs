//! The contract library's host-side plan: a shape table in, the plan's integers out.

/// One contraction dim: each operand's stride along it, and the extent.
///
/// # Invariant
/// A stride is zero exactly when its operand does not carry the dim, and the plan keeps the
/// table's order: the entries are `a_stride`, `b_stride`, `extent` per dim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dim {
    pub a_stride: u32,
    pub b_stride: u32,
    pub extent: u32,
}

/// The plan's integers, three per dim, in the table's order.
///
/// # Invariant
/// The library's kernel reads a plan buffer — three entries per contraction dim — and decodes
/// an output lane's coordinates through them. The plan is host data: the language reads the
/// entries from the value it is handed and never recomputes a stride.
pub fn plan_entries(dims: &[Dim]) -> Vec<i64> {
    let mut entries = Vec::with_capacity(dims.len() * 3);
    for dim in dims {
        entries.push(i64::from(dim.a_stride));
        entries.push(i64::from(dim.b_stride));
        entries.push(i64::from(dim.extent));
    }
    entries
}
