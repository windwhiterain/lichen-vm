//! The fused multiply-add on a device: what the host and the device each answer.
//!
//! # Invariant
//! Every figure below is a comparison against the **host's own `f32::mul_add`** over the same
//! significand grid, and the grid is this file's, so both sides are read over the same points.
//! See `docs/notes/fused-multiply-add.md` §11.

use std::io::Write;
use std::process::Command;

use lichen_compute_gpu::spirv::{self, Binding};
use lichen_compute_gpu::{GpuContext, LOCAL_SIZE_X};
use lichen_kernel_ir::{
    BufferSlot, IntWidth, KernelBin, KernelBody, KernelFragment, KernelInstr, KernelRoles,
    KernelShape, LaneHeight, LaneWidth, LaunchSet, ScalarClass, ScalarData, TargetFacts,
    Terminator,
};

/// The lanes one grid holds: the numerators of the earlier slices' grids, `[1, 2)` at `2^-12`.
const LANES: usize = 4096;

/// The significand grid: `1 + i / 2^12`.
fn dense() -> Vec<f32> {
    (0..LANES)
        .map(|lane| 1.0 + f32::from(lane as u16) / 4096.0)
        .collect()
}

/// The `ln` band, `[1/sqrt(2), sqrt(2))`, mapped off the same grid.
fn band() -> Vec<f32> {
    let low = 1.0 / 2.0f32.sqrt();
    let span = 2.0f32.sqrt() - low;
    dense()
        .iter()
        .map(|numerator| low + span * (numerator - 1.0))
        .collect()
}

/// A constant's `f32` bits, as the IR's `Const` payload carries them.
fn bits(value: f32) -> i64 {
    i64::from(value.to_bits())
}

/// How many units in the last place two `f32`s sit apart.
fn ulps_apart(from: f32, to: f32) -> i64 {
    let key = |value: f32| {
        let raw = value.to_bits() as i32;
        i64::from(if raw < 0 {
            i32::MIN.wrapping_sub(raw)
        } else {
            raw
        })
    };
    (key(from) - key(to)).abs()
}

/// A fragment over `(config, index)` with `inputs` `Float` inputs and one `Float` output.
///
/// # Invariant
/// `exact` is the body's own `exact_float_arithmetic`, which is the decision the emitter turns into
/// a `NoContraction` per float result (`docs/notes/contraction-control.md` §9.1).
fn float_fragment(inputs: usize, mut body: KernelBody, exact: bool) -> KernelFragment {
    body.exact_float_arithmetic = exact;
    body.validate().expect("the probe's body is well formed");
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body,
        inputs,
        outputs: 1,
        input_classes: vec![ScalarClass::Float; inputs],
        output_classes: vec![ScalarClass::Float],
        result_classes: vec![ScalarClass::Float],
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(LOCAL_SIZE_X),
            lane_height: LaneHeight::new(1),
        },
        slices: Vec::new(),
        marks: Vec::new(),
    }
}

/// The named values one float probe's entry block computes, in the order it computes them.
struct Probe {
    body: KernelBody,
    entry: usize,
    index: lichen_kernel_ir::ValueId,
}

/// A fresh body over `(config, index)`.
fn start() -> Probe {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    body.add_param(entry); // the config leaf, fixed by the launch
    let index = body.add_param(entry);
    Probe { body, entry, index }
}

/// `out[i] = compute(its inputs)`, over `inputs` `Float` buffers read at the lane's index.
fn kernel(
    inputs: usize,
    exact: bool,
    compute: impl FnOnce(
        &mut KernelBody,
        usize,
        &[lichen_kernel_ir::ValueId],
        lichen_kernel_ir::ValueId,
    ),
) -> KernelFragment {
    let Probe {
        mut body,
        entry,
        index,
    } = start();
    let arguments: Vec<_> = (0..inputs)
        .map(|position| {
            let ordinal = body.add_const(entry, ScalarClass::Int, position as i64);
            body.add_op(
                entry,
                KernelInstr::BufferReadCall(ScalarClass::Float),
                vec![ordinal, index],
                vec![ScalarClass::Float],
            )
        })
        .collect();
    compute(&mut body, entry, &arguments, index);
    let result = body.add_const(entry, ScalarClass::Int, 0);
    body.set_terminator(
        entry,
        Terminator::Return {
            values: vec![result],
        },
    );
    float_fragment(inputs, body, exact)
}

/// `out[i] = a*b + c`, on the fused instruction and on a multiply with an add.
fn product(fused: bool, exact: bool) -> KernelFragment {
    kernel(3, exact, |body, entry, arguments, index| {
        let [a, b, c] = arguments else {
            unreachable!("a product reads three inputs")
        };
        let sum = if fused {
            body.add_op(
                entry,
                KernelInstr::Fma,
                vec![*a, *b, *c],
                vec![ScalarClass::Float],
            )
        } else {
            let product = body.add_op(
                entry,
                KernelInstr::Bin(ScalarClass::Float, KernelBin::Mul),
                vec![*a, *b],
                vec![ScalarClass::Float],
            );
            body.add_op(
                entry,
                KernelInstr::Bin(ScalarClass::Float, KernelBin::Add),
                vec![product, *c],
                vec![ScalarClass::Float],
            )
        };
        write(body, entry, index, sum);
    })
}

/// Markstein's compensated division, on the fused instruction: §2's `Markstein on mul_add`.
///
/// # Invariant
/// The residual is `a - b*q`, which is `Fma(-b, q, a)`, and **the IR has no negation**: `-b` is
/// written as `Bin(Float, Sub)` against `+0.0`, which is `-b` exactly for every divisor the
/// language admits, since a zero divisor is undefined.
fn compensated(exact: bool) -> KernelFragment {
    kernel(2, exact, |body, entry, arguments, index| {
        let [a, b] = arguments else {
            unreachable!("a division reads two inputs")
        };
        let one = body.add_const(entry, ScalarClass::Float, bits(1.0));
        let zero = body.add_const(entry, ScalarClass::Float, bits(0.0));
        let reciprocal = body.add_op(
            entry,
            KernelInstr::Bin(ScalarClass::Float, KernelBin::Div),
            vec![one, *b],
            vec![ScalarClass::Float],
        );
        let quotient = body.add_op(
            entry,
            KernelInstr::Fma,
            vec![*a, reciprocal, zero],
            vec![ScalarClass::Float],
        );
        let negated = body.add_op(
            entry,
            KernelInstr::Bin(ScalarClass::Float, KernelBin::Sub),
            vec![zero, *b],
            vec![ScalarClass::Float],
        );
        let residual = body.add_op(
            entry,
            KernelInstr::Fma,
            vec![negated, quotient, *a],
            vec![ScalarClass::Float],
        );
        let corrected = body.add_op(
            entry,
            KernelInstr::Fma,
            vec![residual, reciprocal, quotient],
            vec![ScalarClass::Float],
        );
        write(body, entry, index, corrected);
    })
}

/// The plain division over the same two inputs: the control the compensated body is read beside.
fn division(exact: bool) -> KernelFragment {
    kernel(2, exact, |body, entry, arguments, index| {
        let [a, b] = arguments else {
            unreachable!("a division reads two inputs")
        };
        let quotient = body.add_op(
            entry,
            KernelInstr::Bin(ScalarClass::Float, KernelBin::Div),
            vec![*a, *b],
            vec![ScalarClass::Float],
        );
        write(body, entry, index, quotient);
    })
}

/// Write `value` to output position `0` at `index`.
fn write(
    body: &mut KernelBody,
    entry: usize,
    index: lichen_kernel_ir::ValueId,
    value: lichen_kernel_ir::ValueId,
) {
    let position = body.add_const(entry, ScalarClass::Int, 0);
    body.add_op(
        entry,
        KernelInstr::BufferWriteCall(ScalarClass::Float),
        vec![position, index, value],
        Vec::new(),
    );
}

/// The packed bytes of `values`, one `f32` each.
fn pack(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// The `f32` elements of a fetched payload.
fn floats(data: ScalarData) -> Vec<f32> {
    match data {
        ScalarData::Float(elements) => elements,
        ScalarData::Int(elements) => {
            panic!(
                "a float run's result came back as {} integer element(s)",
                elements.len()
            )
        }
    }
}

/// Where a dumped module is written: inside this run's own target directory.
fn dump_path(name: &str) -> std::path::PathBuf {
    let root = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".into());
    std::path::Path::new(&root).join("fma-probe").join(name)
}

/// The validator, at the path it is installed on this machine.
const VALIDATOR: &str = r"C:\VulkanSDK\1.3.296.0\Bin\spirv-val.exe";
/// The disassembler, at the path it is installed on this machine.
const DISASSEMBLER: &str = r"C:\VulkanSDK\1.3.296.0\Bin\spirv-dis.exe";

/// Hand a module to `spirv-val` under `--target-env vulkan1.1`, and print its verdict.
///
/// # Invariant
/// The return says whether the module was actually checked, so a machine without the validator
/// cannot read as one whose modules validated.
fn validate(name: &str, words: &[u32]) -> bool {
    let path = dump_path(&format!("{name}.spv"));
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("create the dump directory");
    let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    let mut file = std::fs::File::create(&path).expect("write the module");
    file.write_all(&bytes).expect("write the module");
    drop(file);
    println!("  {name}: {} words", words.len());

    let Ok(output) = Command::new(VALIDATOR)
        .args(["--target-env", "vulkan1.1", path.to_str().expect("a path")])
        .output()
    else {
        println!("    NOT VALIDATED: `{VALIDATOR}` did not start");
        return false;
    };
    println!(
        "    spirv-val: {}",
        if output.status.success() {
            "ACCEPT"
        } else {
            "REJECT"
        }
    );
    if !output.status.success() {
        println!("    {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    output.status.success()
}

/// The `OpExtInst`, `OpExtInstImport` and `OpDecorate NoContraction` census of a module.
fn census(words: &[u32]) -> (usize, usize, usize) {
    let mut ext_inst = 0;
    let mut ext_import = 0;
    let mut decorations = 0;
    let mut at = 5;
    while at < words.len() {
        let word = words[at];
        let (count, opcode) = ((word >> 16) as u16, (word & 0xffff) as u16);
        match opcode {
            12 => ext_inst += 1,
            11 => ext_import += 1,
            71 => decorations += 1,
            _ => {}
        }
        at += count as usize;
    }
    (ext_inst, ext_import, decorations)
}

/// Print every `spirv-dis` line of a dumped module that names an extended instruction.
fn disassemble(name: &str) {
    let path = dump_path(&format!("{name}.spv"));
    let Ok(output) = Command::new(DISASSEMBLER)
        .arg(path.to_str().expect("a path"))
        .output()
    else {
        return;
    };
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if line.contains("Fma") || line.contains("ExtInstImport") {
            println!("    {}", line.trim());
        }
    }
}

/// One dispatch, and the `f32` elements it wrote.
fn dispatch(context: &GpuContext, fragment: &KernelFragment, inputs: &[&[f32]]) -> Vec<f32> {
    let packed: Vec<Vec<u8>> = inputs.iter().map(|values| pack(values)).collect();
    let slots: Vec<BufferSlot<'_>> = packed.iter().map(|bytes| BufferSlot::Host(bytes)).collect();
    let count = inputs[0].len();
    let resident = context
        .run(&LaunchSet::single(fragment), &slots, count)
        .unwrap_or_else(|reason| panic!("the dispatch completes: {reason}"));
    let answer = floats(
        context
            .fetch(resident[0], count)
            .expect("the result comes back off the device"),
    );
    context.release(resident[0]);
    answer
}

/// The three grids the product body runs over, each `(a, b, c)` per lane.
fn product_grids() -> Vec<(&'static str, Vec<f32>, Vec<f32>, Vec<f32>)> {
    let dense = dense();
    let band = band();
    let mixed = (0..LANES)
        .map(|lane| dense[(7 * lane + 1) % LANES])
        .collect::<Vec<f32>>();
    vec![
        (
            "product",
            dense.clone(),
            mixed.clone(),
            (0..LANES)
                .map(|lane| dense[(13 * lane + 11) % LANES])
                .collect(),
        ),
        (
            "cancel",
            dense.clone(),
            mixed.clone(),
            (0..LANES)
                .map(|lane| -(dense[lane] * mixed[lane]))
                .collect(),
        ),
        (
            "band",
            band.clone(),
            vec![3.0f32; LANES],
            (0..LANES)
                .map(|lane| band[(11 * lane + 5) % LANES])
                .collect(),
        ),
    ]
}

/// The four division grids: `(a, b)` per lane, the set the earlier slices measured.
fn division_grids() -> Vec<(&'static str, Vec<f32>, Vec<f32>)> {
    let numerators = dense();
    let band = band();
    vec![
        ("dense / 3.0", numerators.clone(), vec![3.0; LANES]),
        ("dense / 7.0", numerators.clone(), vec![7.0; LANES]),
        ("dense / 2.5", numerators, vec![2.5; LANES]),
        (
            "ln band",
            dense(),
            (0..LANES)
                .map(|lane| band[(3usize * lane + 1) % LANES])
                .collect(),
        ),
    ]
}

/// The compensated division, on the host, in the order the body computes it.
fn host_markstein(a: f32, b: f32) -> f32 {
    let reciprocal = 1.0f32 / b;
    let quotient = a.mul_add(reciprocal, 0.0);
    let residual = (0.0f32 - b).mul_add(quotient, a);
    residual.mul_add(reciprocal, quotient)
}

/// How many lanes a device's answers are bit for bit, and the worst ulp distance seen.
fn agree(answers: &[f32], expected: &[f32]) -> (usize, i64) {
    answers
        .iter()
        .zip(expected)
        .map(|(answer, want)| ulps_apart(*answer, *want))
        .fold((0usize, 0i64), |(exact, worst), ulps| {
            (exact + usize::from(ulps == 0), worst.max(ulps))
        })
}

fn main() {
    let context = GpuContext::new().unwrap_or_else(|reason| {
        eprintln!("no device to run on: {reason}");
        std::process::exit(2);
    });
    println!("device: {}", context.device_name());
    println!("lanes:  {LANES}\n");

    println!("== the emitted modules ==");
    let three = Binding {
        inputs: 3,
        outputs: 1,
    };
    let two = Binding {
        inputs: 2,
        outputs: 1,
    };
    let mut validated = 0;
    let mut offered = 0;
    for (name, fragment, binding) in [
        ("fused_exact", product(true, true), three),
        ("fused_plain", product(true, false), three),
        ("unfused_exact", product(false, true), three),
        ("compensated_exact", compensated(true), two),
        ("compensated_plain", compensated(false), two),
        ("plain_division", division(true), two),
    ] {
        offered += 1;
        let words = spirv::compile(&LaunchSet::single(&fragment), binding)
            .unwrap_or_else(|reason| panic!("{name} is emitted: {reason}"));
        let (ext_inst, ext_import, decorations) = census(&words);
        println!(
            "  OpExtInst {ext_inst}, OpExtInstImport {ext_import}, OpDecorate {decorations}, \
             id bound {}",
            words[3]
        );
        if validate(name, &words) {
            validated += 1;
        }
        if name == "fused_exact" || name == "compensated_exact" {
            disassemble(name);
        }
    }
    println!("  {validated} of {offered} emitted module(s) `spirv-val` ACCEPT\n");

    println!("== a*b + c, against the host's `f32::mul_add` ==");
    for (name, a, b, c) in product_grids() {
        let differing = (0..LANES)
            .filter(|&lane| {
                a[lane].mul_add(b[lane], c[lane]).to_bits()
                    != (a[lane] * b[lane] + c[lane]).to_bits()
            })
            .count();
        let wanted: Vec<f32> = (0..LANES)
            .map(|lane| a[lane].mul_add(b[lane], c[lane]))
            .collect();
        for exact in [true, false] {
            let (bit_for_bit, worst) = agree(
                &dispatch(&context, &product(true, exact), &[&a, &b, &c]),
                &wanted,
            );
            println!(
                "  {name}, fused, NoContraction {exact}: {bit_for_bit} of {LANES} bit for bit, \
                 worst {worst} ulp ({differing} lane(s) where the two readings differ)",
            );
        }
        let (bit_for_bit, worst) = agree(
            &dispatch(&context, &product(false, true), &[&a, &b, &c]),
            &wanted,
        );
        println!(
            "  {name}, a multiply and an add, NoContraction true: {bit_for_bit} of {LANES} bit \
             for bit, worst {worst} ulp",
        );
        // **The `cancel` grid's own reading**: its ulp distance is unreadable, so it
        // is read by the relative gap.
        let gap = |answers: &[f32]| -> f64 {
            answers
                .iter()
                .zip(&wanted)
                .map(|(got, want)| {
                    let want = *want as f64;
                    let gap = (*got as f64 - want).abs();
                    if want == 0.0 {
                        // A relative gap against zero is only defined as "was it also zero".
                        if gap == 0.0 { 0.0 } else { f64::INFINITY }
                    } else {
                        gap / want.abs()
                    }
                })
                .fold(0.0f64, f64::max)
        };
        let fused = dispatch(&context, &product(true, true), &[&a, &b, &c]);
        let unfused = dispatch(&context, &product(false, true), &[&a, &b, &c]);
        println!(
            "  {name}: the fused body's worst relative gap from the host is {:e}, the \
             multiply-and-add body's is {:e}",
            gap(&fused),
            gap(&unfused)
        );
    }

    println!("\n== the compensated division, against the correctly rounded quotient ==");
    for (name, a, b) in division_grids() {
        let wanted: Vec<f32> = (0..LANES).map(|lane| a[lane] / b[lane]).collect();
        for exact in [true, false] {
            let (bit_for_bit, worst) =
                agree(&dispatch(&context, &compensated(exact), &[&a, &b]), &wanted);
            println!(
                "  {name}, compensated, NoContraction {exact}: {bit_for_bit} of {LANES} bit for \
                 bit, worst {worst} ulp",
            );
        }
        let (bit_for_bit, worst) = agree(&dispatch(&context, &division(true), &[&a, &b]), &wanted);
        let (host_exact, host_worst) = agree(
            &(0..LANES)
                .map(|lane| host_markstein(a[lane], b[lane]))
                .collect::<Vec<f32>>(),
            &wanted,
        );
        println!(
            "  {name}, a plain division: {bit_for_bit} of {LANES} bit for bit, worst {worst} ulp",
        );
        println!(
            "  {name}, the same sequence on the host: {host_exact} of {LANES} bit for bit, worst \
             {host_worst} ulp",
        );
    }
}
