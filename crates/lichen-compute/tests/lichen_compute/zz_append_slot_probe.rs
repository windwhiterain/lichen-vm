//! Probe: where the device loses a cache slot the host keeps, printing and asserting nothing.

use super::common;

use lichen_compute::language::new_store;
use lichen_language::run::evaluate_raw;

/// The elements of a `Float` literal list, as lichen spells them.
fn floats(values: &[f32]) -> String {
    values
        .iter()
        .map(|value| format!("{value}.0"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The committed append kernel over `capacity` positions of `dim` elements.
///
/// # Invariant
/// The cache and the value rows are distinct and no element equals its own slot, so a write that
/// lands in the wrong slot is visible in the answer rather than coinciding with the right one.
fn append_program(
    capacity: usize,
    dim: usize,
    keys: &[f32],
    position: usize,
    backend: &str,
) -> String {
    let count = capacity * dim;
    let cache = floats(
        &(1..=count)
            .map(|index| (10 * index) as f32)
            .collect::<Vec<f32>>(),
    );
    let values = floats(
        &(1..=count)
            .map(|index| (1000 + 10 * index) as f32)
            .collect::<Vec<f32>>(),
    );
    let key = floats(keys);
    let value = floats(&keys.iter().map(|key| key + 1000.0).collect::<Vec<f32>>());
    let keys_len = keys.len();
    format!(
        r#"
--- compute = import "compute.lichen" ---
In  = struct{{.k (compute.Buf Float), .v (compute.Buf Float), .nk (compute.Buf Float), .nv (compute.Buf Float)}}
Out = struct{{.k (compute.Buf Float), .v (compute.Buf Float)}}
Par = struct{{.n Int, .cap Int, .dim Int, .t Int, .in In, .out Out}}
Arg = struct{{.n Int, .cap Int, .dim Int, .t Int, .I In}}
append_raw = (k : Par) => {{
  column = compute.row k
  position = compute.col k
  slot = position * k.dim + column
  old_k = (compute.read ((compute.Read _)(.from k.in.k, .at slot)) : Float)
  old_v = (compute.read ((compute.Read _)(.from k.in.v, .at slot)) : Float)
  fresh = position == k.t
  new_k = if fresh then ((compute.read ((compute.Read _)(.from k.in.nk, .at column)) : Float)) else old_k
  new_v = if fresh then ((compute.read ((compute.Read _)(.from k.in.nv, .at column)) : Float)) else old_v
  (compute.write ((compute.Write _)(.to k.out.k, .at slot, .value (new_k ~ "element"))), compute.write ((compute.Write _)(.to k.out.v, .at slot, .value (new_v ~ "element"))))
}}
append = compute.parallel (append_raw ~ "claim:Reduction(element)") "{backend}" Out
k0 = (compute.buffer ([{cache}] : array{{Float, {count}}}) : (compute.Buf Float))
v0 = (compute.buffer ([{values}] : array{{Float, {count}}}) : (compute.Buf Float))
nk = (compute.buffer ([{key}] : array{{Float, {keys_len}}}) : (compute.Buf Float))
nv = (compute.buffer ([{value}] : array{{Float, {keys_len}}}) : (compute.Buf Float))
out = (compute.plrun append ((Arg)(.n {count}, .cap {capacity}, .dim {dim}, .t {position}, .I In(.k k0, .v v0, .nk nk, .nv nv))) : Out)
(compute.collect out.k, compute.collect out.v)
"#,
        count = count,
        capacity = capacity,
        dim = dim,
        position = position,
        backend = backend,
        cache = cache,
        values = values,
        key = key,
        value = value,
        keys_len = keys_len,
    )
}

/// One run, rendered, or its refusals.
fn run(source: &str) -> String {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match evaluate_raw::<lichen_compute::language::LangProgram>(source, None, &mut store) {
        Ok(rendered) => rendered,
        Err(diagnostics) => format!("refused: {diagnostics:?}"),
    }
}

#[test]
fn probe_the_lost_cache_slot() {
    let Some((_context, _guard)) = common::device("zz_append_slot_probe") else {
        return;
    };
    // The same run three times: a slot whose loss moves between them is a race, not an address.
    for attempt in 1..=3 {
        println!(
            "[eight_one/repeat{attempt}] {}",
            run(&append_program(8, 1, &[100.0], 1, "gpu"))
        );
    }
    // A `nk` long enough that a surplus lane's fresh write carries a value of its own.
    let keys: Vec<f32> = (0..8).map(|index| (7 + 10 * index) as f32).collect();
    for backend in ["gpu", "cpu"] {
        println!(
            "[long_keys/{backend}] {}",
            run(&append_program(8, 1, &keys, 1, backend))
        );
    }
    for (name, capacity, dim, position) in [
        ("four_one_t1", 4, 1, 1),
        ("sixteen_one_t1", 16, 1, 1),
        ("eight_one_t3", 8, 1, 3),
        ("four_two_t1", 4, 2, 1),
    ] {
        for backend in ["gpu", "cpu"] {
            println!(
                "[{name}/{backend}] {}",
                run(&append_program(capacity, dim, &[100.0], position, backend))
            );
        }
    }
}
