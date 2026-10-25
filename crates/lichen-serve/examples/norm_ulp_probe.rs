//! The pre-attention norm chain: the wide chain, the per-head chain, and the merged node.
//!
//! # Invariant
//! **The three runs differ in one thing each**: the wide chain and the per-head chain differ
//! in their lane map, and the merged run differs in being one node — so a difference the probe
//! prints belongs to the change the run's own name states, and to nothing else.

use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_compute::fusion_file::{Budget, FusionPlan, Strategy};
use lichen_compute_gpu::spirv::{self, Binding};
use lichen_graph_ir::Node;
use lichen_kernel_ir::{KernelFragment, LaunchSet, ScalarClass, ScalarData, sqrt_value};
use lichen_serve::Session;

/// The heads the chain has.
const HEADS: usize = 8;

/// The elements in one head's row.
const DIM: usize = 128;

/// The elements the chain's rows hold.
const ROWS: usize = HEADS * DIM;

/// The scale kernel of the composite's **old** lane map: one lane per element.
const WIDE_SCALE: &str = r#"
scale_raw = (k : ScalePar) => {
  i = compute.range k.n
  head = i / k.dim
  j = i - head * k.dim
  total = (compute.read ((compute.Read _)(.from k.in.s, .at head)) : Float)
  mean = total / (int2float k.dim)
  divisor = sqrt (mean + 0.000001)
  value = (compute.read ((compute.Read _)(.from k.in.x, .at i)) : Float)
  weight = (compute.read ((compute.Read _)(.from k.in.w, .at j)) : Float)
  b_y = compute.buf ((compute.BufParam _)(.type Float, .length __ROWS__))
  b_d = compute.buf ((compute.BufParam _)(.type Float, .length __ROWS__))
  ((Normed)(.y (compute.write ((compute.Write _)(.to b_y, .at i, .value (((value * weight) / divisor) ~ "element")))), .d (compute.write ((compute.Write _)(.to b_d, .at i, .value divisor)))))
}"#;

/// The scale kernel of the composite's **per-head** lane map: one lane per head.
const PER_HEAD_SCALE: &str = r#"
scale_raw = (k : ScalePar) => {
  head = compute.range k.n
  total = (compute.read ((compute.Read _)(.from k.in.s, .at head)) : Float)
  mean = total / (int2float k.dim)
  divisor = sqrt (mean + 0.000001)
  b_y = compute.buf ((compute.BufParam _)(.type Float, .length __ROWS__))
  b_d = compute.buf ((compute.BufParam _)(.type Float, .length __LANES__))
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (compute.write ((compute.Write _)(.to s(1), .at (head * k.dim + (s(0) - 1)), .value ((((compute.read ((compute.Read _)(.from k.in.x, .at (head * k.dim + (s(0) - 1))))) : Float) * (compute.read ((compute.Read _)(.from k.in.w, .at (s(0) - 1))) : Float)) / divisor) ~ "element"))))
  ((Normed)(.y (fold (k.dim, b_y)), .d (compute.write ((compute.Write _)(.to b_d, .at head, .value divisor)))))
}"#;

/// The chain's program: the squares kernel and the scale kernel, over `__LANES__` lanes.
///
/// # Invariant
/// **The two kernels are the composite's own bodies**: the squares kernel folds one head's row
/// at its own lane, and the scale kernel reads that sum at the same lane. The wide lane map
/// dispatches the squares over the elements and the scale over the elements too, and the
/// per-head lane map dispatches both over the heads — so the two programs differ in the lane
/// map alone.
const TEMPLATE: &str = r#"
--- compute = import "compute.lichen" ---
SquareIn = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length _)))}
Squared  = struct{.s (compute.Buf ((compute.BufParam _)(.type Float, .length _)))}
ScaleIn  = struct{.s (compute.Buf ((compute.BufParam _)(.type Float, .length _))), .x (compute.Buf ((compute.BufParam _)(.type Float, .length _))), .w (compute.Buf ((compute.BufParam _)(.type Float, .length _)))}
Normed   = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length _))), .d (compute.Buf ((compute.BufParam _)(.type Float, .length _)))}
Out      = struct{.s (compute.Buf ((compute.BufParam _)(.type Float, .length _))), .y (compute.Buf ((compute.BufParam _)(.type Float, .length _))), .d (compute.Buf ((compute.BufParam _)(.type Float, .length _)))}
SquarePar = struct{.n Int, .dim Int, .in SquareIn}
SquareArg = struct{.n Int, .dim Int, .I SquareIn}
squares_raw = (k : SquarePar) => {
  lane = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.x, .at ((lane * k.dim + (s(0) - 1))))) : Float) * (compute.read ((compute.Read _)(.from k.in.x, .at ((lane * k.dim + (s(0) - 1))))) : Float))) ~ "combine" ~ "element"))
  b_s = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((Squared)(.s (compute.write ((compute.Write _)(.to b_s, .at lane, .value (fold (k.dim, 0.0 ~ "identity")))))))
}
squares = compute.parallel (squares_raw ~ "claim:Reduction(combine,identity,element)") "gpu" Squared
ScalePar = struct{.n Int, .dim Int, .in ScaleIn}
ScaleArg = struct{.n Int, .dim Int, .I ScaleIn}
__SCALE__
scale = compute.parallel (scale_raw ~ "claim:Elementwise(element)") "gpu" Normed
Rows     = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length _))), .w (compute.Buf ((compute.BufParam _)(.type Float, .length _)))}
GArg = struct{.in Rows}
step = (s : GArg) => {
  sq = (compute.plrun squares ((SquareArg)(.n __LANES__, .dim __DIM__, .I SquareIn(.x s.in.x))) : Squared)
  nm = (compute.plrun scale ((ScaleArg)(.n __LANES__, .dim __DIM__, .I ScaleIn(.s sq.s, .x s.in.x, .w s.in.w))) : Normed)
  ((Out)(.s sq.s, .y nm.y, .d nm.d))
}
built = compute.graph step
built
"#;

/// The chain's program in the lane map `wide` names.
///
/// # Invariant
/// The wide map dispatches the squares over the **elements** (which is what the composite did
/// before the rewrite, and what read past its own buffer), so the wide run's input buffer is
/// padded to the length those reads reach — the padding holds zeroes and the meaningful rows
/// are the first `__ROWS__` of it.
fn program(wide: bool) -> String {
    let lanes = if wide { ROWS } else { HEADS };
    let scale = if wide { WIDE_SCALE } else { PER_HEAD_SCALE };
    TEMPLATE
        .replace("__SCALE__", scale)
        .replace("__LANES__", &lanes.to_string())
        .replace("__DIM__", &DIM.to_string())
        .replace("__ROWS__", &ROWS.to_string())
}

/// The plan that merges the chain: the sibling strategy, with nothing bounded.
fn plan() -> FusionPlan {
    FusionPlan {
        strategy: Strategy::Sibling,
        budget: Budget::unlimited(),
        overrides: Vec::new(),
        barriers: Vec::new(),
    }
}

/// The host's rows, and the gains that weight them.
///
/// # Invariant
/// **The same numbers for every run**: the pattern is named, so a run that differs from another
/// differs in the program and not in the data.
fn rows(pattern: usize) -> Vec<f32> {
    (0..ROWS)
        .map(|at| match pattern {
            0 => (at % 11) as f32 * 0.001,
            1 => ((at * 37 + 11) % 101) as f32 * 0.0173 - 0.7,
            2 => (at as f32 * 0.0031).sin() * 0.9 + (at % 5) as f32 * 0.01,
            // **The squares are inexact**: the plain sum and the fused sum differ.
            _ => 0.1 + (at % 13) as f32 * 0.0073,
        })
        .collect()
}

/// The host's gains, one per element of a head.
fn gains(pattern: usize) -> Vec<f32> {
    (0..DIM)
        .map(|at| match pattern {
            0 => (at % 7) as f32 * 0.001,
            1 => ((at * 13 + 3) % 29) as f32 * 0.021 - 0.2,
            2 => (at as f32 * 0.011).cos() * 0.8,
            _ => 0.1 + (at % 5) as f32 * 0.031,
        })
        .collect()
}

/// A buffer of full-precision elements.
fn floats(values: &[f32]) -> RunArgument {
    RunArgument::Buffer {
        class: ScalarClass::Float,
        data: values
            .iter()
            .flat_map(|value| value.to_bits().to_le_bytes())
            .collect(),
    }
}

/// The float's bits, in the order the buffer holds them.
fn bits(values: &[f32]) -> Vec<u32> {
    values.iter().map(|value| value.to_bits()).collect()
}

/// The distance between two floats' bits, in units in the last place.
///
/// # Invariant
/// The magnitude of the difference, counted in representable steps: `±0` are one step apart
/// because they are one step apart in the order, and every other pair is its own count.
fn distance(left: u32, right: u32) -> i64 {
    let magnitude = |bits: u32| (bits & 0x7fff_ffff) as i64;
    let signed = |bits: u32| {
        if bits & 0x8000_0000 != 0 {
            -magnitude(bits)
        } else {
            magnitude(bits)
        }
    };
    (signed(left) - signed(right)).abs()
}

/// The head's sum, in the two ways a device may compute it.
///
/// # Invariant
/// **The fold's own order**: the counter runs from `DIM` down to one, so the row is folded from
/// its last element to its first. The plain sum rounds the product and the sum separately; the
/// fused sum rounds once, which is what a driver contracts a multiply-add into.
fn sums(row: &[f32], head: usize) -> (f32, f32) {
    let mut plain = 0.0f32;
    let mut fused = 0.0f32;
    for offset in (0..DIM).rev() {
        let value = row[head * DIM + offset];
        plain += value * value;
        fused = value.mul_add(value, fused);
    }
    (plain, fused)
}

/// The divisor the chain's own arithmetic gives a sum.
///
/// # Invariant
/// **The IR's definition, not the platform's square root**: the kernel's `sqrt` is inlined by
/// [`sqrt_value`], so the host's answer is the one the device's definition computes — before
/// any driver is free to contract it.
fn divisor(sum: f32) -> f32 {
    let mean = sum / (DIM as f32);
    sqrt_value(mean + 0.000001)
}

/// One run's answers: the sums, the scaled row and the divisors.
struct Answers {
    /// The producer's sums, one per head.
    sums: Vec<u32>,
    /// The scaled elements, head by head.
    scaled: Vec<u32>,
    /// The divisor each head's lane computed.
    divisors: Vec<u32>,
    /// **The step between one head's divisor and the next**: the wide chain writes one per
    /// element and the per-head chain one per head, so the divisor of head `h` is at
    /// `h * stride`.
    divisor_stride: usize,
}

/// The run's answers, as the device wrote them.
fn answers(session: &Session, stepped: &lichen_serve::Stepped, divisor_stride: usize) -> Answers {
    let mut fetched = Vec::new();
    for result in &stepped.results {
        let RunResult::Resident(resident) = result else {
            panic!("no resident");
        };
        let answer = session.fetch(*resident).expect("the answer reads");
        session.release(*resident);
        let ScalarData::Float(values) = answer else {
            panic!("unexpected answer");
        };
        fetched.push(bits(&values));
    }
    Answers {
        sums: fetched[0].clone(),
        scaled: fetched[1].clone(),
        divisors: fetched[2].clone(),
        divisor_stride,
    }
}

/// The heads whose elements differ, and how many of each head's elements differ.
fn heads_that_differ(left: &[u32], right: &[u32]) -> Vec<(usize, usize)> {
    let mut counts = vec![0usize; HEADS];
    for (index, (one, other)) in left.iter().zip(right.iter()).enumerate() {
        if one != other {
            counts[index / DIM] += 1;
        }
    }
    counts
        .iter()
        .enumerate()
        .filter(|(_, count)| **count > 0)
        .map(|(head, count)| (head, *count))
        .collect()
}

/// The comparison of two runs' answers, element by element.
fn compare(label: &str, left: &Answers, right: &Answers) {
    println!("=== {label}");
    let differing = heads_that_differ(&left.scaled, &right.scaled);
    if differing.is_empty() {
        println!("  the scaled elements agree on every bit");
    } else {
        println!(
            "  the scaled elements differ on {} head(s): {:?}",
            differing.len(),
            differing
        );
        for (head, count) in &differing {
            let first = left.scaled[head * DIM];
            let second = right.scaled[head * DIM];
            println!(
                "    head {head}: {count} of {DIM} element(s) differ, first element {first:#010x} \
                 against {second:#010x} ({} ulp apart)",
                distance(first, second)
            );
        }
        // The elements of the first head that differ, in full.
        let head = differing[0].0;
        let mut printed = 0;
        for offset in 0..DIM {
            let index = head * DIM + offset;
            if left.scaled[index] != right.scaled[index] {
                if printed < 8 {
                    println!(
                        "      element {offset}: {:#010x} against {:#010x} ({} ulp apart)",
                        left.scaled[index],
                        right.scaled[index],
                        distance(left.scaled[index], right.scaled[index])
                    );
                }
                printed += 1;
            }
        }
        println!("      ... {printed} element(s) in total");
    }
    let differing: Vec<(usize, u32, u32)> = (0..HEADS)
        .map(|head| (head, left.sums[head], right.sums[head]))
        .filter(|(_, one, other)| one != other)
        .collect();
    if differing.is_empty() {
        println!("  the sums agree on every bit");
    } else {
        println!("  the sums differ on {} head(s)", differing.len());
        for (head, one, other) in &differing {
            println!(
                "    head {head}: {one:#010x} against {other:#010x} ({} ulp apart)",
                distance(*one, *other)
            );
        }
    }
    let differing: Vec<(usize, u32, u32)> = (0..HEADS)
        .map(|head| {
            (
                head,
                left.divisors[head * left.divisor_stride],
                right.divisors[head * right.divisor_stride],
            )
        })
        .filter(|(_, one, other)| one != other)
        .collect();
    if differing.is_empty() {
        println!("  the divisors agree on every bit");
    } else {
        println!("  the divisors differ on {} head(s)", differing.len());
        for (head, one, other) in &differing {
            println!(
                "    head {head}: {one:#010x} against {other:#010x} ({} ulp apart)",
                distance(*one, *other)
            );
        }
    }
}

/// The bodies' own exactness decision, and the nodes the prepared graph holds.
fn report_nodes(label: &str, prepared: &lichen_serve::Prepared<'_>) {
    println!("=== {label}: {} node(s)", prepared.nodes());
    for (position, node) in prepared.graph().nodes().iter().enumerate() {
        let Node::Kernel(kernel) = node;
        println!(
            "  node {position}: exact_float_arithmetic: {}, inputs: {}, outputs: {}, extent: {:?}, \
             NoContraction decoration(s): {}",
            kernel.fragment.body.exact_float_arithmetic,
            kernel.fragment.inputs,
            kernel.fragment.outputs,
            kernel.count,
            no_contraction(&kernel.fragment)
        );
    }
}

/// The `NoContraction` decorations the emitter writes for one fragment's module.
///
/// # Invariant
/// **The module's own words, counted**: the decoration is `OpDecorate <result> NoContraction`
/// and the decision that writes it is the body's own `exact_float_arithmetic`
/// (`docs/notes/contraction-control.md` §9.1), so the count is what the driver is forbidden
/// to contract.
fn no_contraction(fragment: &KernelFragment) -> usize {
    let binding = Binding {
        inputs: fragment.inputs,
        outputs: fragment.outputs,
    };
    let Ok(words) = spirv::compile(&LaunchSet::single(fragment), binding) else {
        return 0;
    };
    let mut count = 0;
    let mut at = 5;
    while at < words.len() {
        let word = words[at];
        let (length, opcode) = ((word >> 16) as usize, (word & 0xffff) as u16);
        // `OpDecorate` is 71, and its decoration operand is the second word of the instruction.
        if opcode == 71 && words.get(at + 2) == Some(&42) {
            count += 1;
        }
        at += length.max(1);
    }
    count
}

/// The probe's own answer, and the three runs it makes.
///
/// # Invariant
/// **Every answer is compared as the `u32` a float's bits are**: the question is a one-unit
/// difference, and a decimal printing cannot see one.
fn main() {
    let session = Session::open().expect("a device");
    let wide_program = program(true);
    let per_head_program = program(false);

    for pattern in 0..3 {
        println!("\n############ pattern {pattern}");
        let row = rows(pattern);
        let gain = gains(pattern);
        // The wide squares kernel reads past its own rows, so its buffer is padded.
        let mut padded = row.clone();
        padded.resize(ROWS * ROWS, 0.0);

        // The wide chain, unmerged: the composite's old lane map.
        lichen_compute::compute::clear_graph_pass();
        let wide = session
            .prepare(&wide_program)
            .expect("the wide chain prepares");
        report_nodes("the wide chain, unmerged", &wide);
        for line in wide.report() {
            println!("  {line}");
        }
        let wide_answers = answers(
            &session,
            &wide
                .step(vec![floats(&padded), floats(&gain)])
                .expect("the wide chain runs"),
            DIM,
        );

        // The per-head chain, unmerged: the composite's new lane map.
        lichen_compute::compute::clear_graph_pass();
        let per_head = session
            .prepare(&per_head_program)
            .expect("the per-head chain prepares");
        report_nodes("the per-head chain, unmerged", &per_head);
        for line in per_head.report() {
            println!("  {line}");
        }
        let per_head_answers = answers(
            &session,
            &per_head
                .step(vec![floats(&row), floats(&gain)])
                .expect("the per-head chain runs"),
            1,
        );

        // The per-head chain, merged: the same program, composed into one node.
        lichen_compute::compute::clear_graph_pass();
        let merged = session
            .prepare_with_plan(&per_head_program, &[], &plan())
            .expect("the per-head chain merges");
        report_nodes("the per-head chain, merged", &merged);
        for line in merged.report() {
            println!("  {line}");
        }
        let merged_answers = answers(
            &session,
            &merged
                .step(vec![floats(&row), floats(&gain)])
                .expect("the merged chain runs"),
            1,
        );

        compare(
            "the wide chain against the per-head chain",
            &wide_answers,
            &per_head_answers,
        );
        compare(
            "the per-head chain against the merged node",
            &per_head_answers,
            &merged_answers,
        );

        // The host's own arithmetic: the sum in the two ways a device may compute it, and the
        // divisor each gives.
        println!("=== the host's own arithmetic");
        for head in 0..HEADS {
            let (plain, fused) = sums(&row, head);
            let plain_bits = divisor(plain).to_bits();
            let fused_bits = divisor(fused).to_bits();
            println!(
                "  head {head}: the plain sum {:#010x} gives the divisor {plain_bits:#010x}, the \
                 fused sum {:#010x} gives {fused_bits:#010x}, the device says {:#010x}, the merged \
                 node says {:#010x}",
                plain.to_bits(),
                fused.to_bits(),
                per_head_answers.sums[head],
                merged_answers.sums[head]
            );
        }
    }
}
