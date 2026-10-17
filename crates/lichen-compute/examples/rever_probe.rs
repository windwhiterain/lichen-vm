//! Probe: the operator bodies, swept under the shipped emitter.
//!
//! One case per process: the backend slot `install_default` fills and the
//! kernel registry are both process-global, so each invocation names one case.
//! Every case runs its program twice, once on `cpu` and once on `gpu`, and scores
//! the two renderings of the same program against each other.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use lichen_tools::readme;

/// The eight mantissas by eight exponents every grid below is cut from.
fn grid(index: usize, shift: i32) -> f32 {
    (1.0 + (index % 8) as f32 / 8.0) * 2.0f32.powi((index / 8) as i32 + shift)
}

/// A value as the **printer** spells it, which is what the two columns are compared
/// through: Rust's shortest form, which round-trips an `f32` exactly.
fn printed(value: f32) -> String {
    format!("{value:?}")
}

/// A swept value as the **source** spells it: no exponent form, no infinity, no
/// `NaN`, and — because the language has no unary minus — a negative written as
/// the subtraction the examples themselves write it with.
fn literal(value: f32) -> String {
    let text = printed(value);
    assert!(
        value.is_finite() && !text.contains('e') && !text.contains('E'),
        "a swept value rendered as {text}, which the source cannot carry"
    );
    match text.strip_prefix('-') {
        None => text,
        Some(magnitude) => format!("(0.0 - {magnitude})"),
    }
}

/// The `array{Float, n}` literal the source reads.
fn array(values: &[f32]) -> String {
    let body: Vec<String> = values.iter().copied().map(literal).collect();
    format!("[{}] : array{{Float, {}}}", body.join(", "), values.len())
}

/// How many units in the last place two `f32`s sit apart.
fn ulps_apart(from: f32, to: f32) -> i64 {
    let key = |value: f32| {
        let bits = value.to_bits() as i32;
        i64::from(if bits < 0 {
            i32::MIN.wrapping_sub(bits)
        } else {
            bits
        })
    };
    (key(from) - key(to)).abs()
}

/// One element of a rendered `array{Float, n}`: the printer's own token, the
/// value parsed back from it, and whether that token round-trips exactly.
struct Element {
    token: String,
    value: f32,
    round_trips: bool,
}

/// The elements of a rendered `array{Float, n}`.
fn elements(rendered: &str) -> Vec<Element> {
    let open = rendered
        .find('[')
        .unwrap_or_else(|| panic!("the program printed no array: {rendered}"));
    let close = rendered[open..]
        .find(']')
        .unwrap_or_else(|| panic!("the printed array is unterminated: {rendered}"))
        + open;
    rendered[open + 1..close]
        .split(',')
        .map(|token| {
            let token = token.trim().to_string();
            let value = token.parse::<f32>().unwrap_or_else(|why| {
                panic!("{token:?} is not a number in {rendered}: {why}")
            });
            Element {
                round_trips: printed(value) == token,
                token,
                value,
            }
        })
        .collect()
}

/// One case's two columns, scored against each other.
#[derive(Default)]
struct Score {
    points: usize,
    exact: usize,
    moving: usize,
    worst_ulp: i64,
    worst_abs: f32,
    /// Of the elements that moved, how many both sides printed in a form that
    /// round-trips exactly — so the ulp figure is read off exact bits.
    approximate: usize,
}

impl Score {
    fn record(&mut self, cpu: &Element, device: &Element) {
        self.points += 1;
        // The printer is a function of the value, so two tokens that are equal
        // are one `f32` whatever the decimal cannot carry.
        if cpu.token == device.token {
            self.exact += 1;
            return;
        }
        self.worst_ulp = self.worst_ulp.max(ulps_apart(device.value, cpu.value));
        self.worst_abs = self.worst_abs.max((device.value - cpu.value).abs());
        self.moving += 1;
        if !(cpu.round_trips && device.round_trips) {
            self.approximate += 1;
        }
    }

    fn merge(&mut self, other: &Score) {
        self.points += other.points;
        self.exact += other.exact;
        self.moving += other.moving;
        self.worst_ulp = self.worst_ulp.max(other.worst_ulp);
        self.worst_abs = self.worst_abs.max(other.worst_abs);
        self.approximate += other.approximate;
    }

    fn report(&self, name: &str) {
        println!(
            "CASE {name} points={} exact={} differ={} worst_ulp={} worst_abs={:e} \
             inexact_printing={}",
            self.points,
            self.exact,
            self.moving,
            self.worst_ulp,
            self.worst_abs,
            self.approximate
        );
    }
}

/// Run `source` once per backend and score the two renderings.
fn measure(name: &str, file: &Path, source: &str) -> Score {
    let on_cpu = readme::program_output(file, source);
    let on_device = readme::program_output(file, &source.replace("\"cpu\"", "\"gpu\""));
    let cpu = elements(&on_cpu);
    let device = elements(&on_device);
    if std::env::var("REVER_PRINT").is_ok() {
        let head: Vec<&str> = cpu.iter().take(4).map(|e| e.token.as_str()).collect();
        let head_gpu: Vec<&str> = device.iter().take(4).map(|e| e.token.as_str()).collect();
        println!("PRINT {name} cpu={head:?} gpu={head_gpu:?}");
    }
    assert_eq!(
        cpu.len(),
        device.len(),
        "{name}: the two backends printed different lengths"
    );
    let mut score = Score::default();
    for (left, right) in cpu.iter().zip(device.iter()) {
        score.record(left, right);
    }
    score
}

/// One program's own case, run on both backends and scored.
fn run_case(name: &str, source: String, file: &Path) -> Score {
    let score = measure(name, file, &source);
    score.report(name);
    score
}

fn example(name: &str) -> PathBuf {
    readme::example_dir().join(name)
}

/// The `silu` gate body over the note's 256 gates, with one spelling per row.
fn silu(spelling: Option<&str>) -> Score {
    let file = example("silu.lichen");
    let source = readme::read_normalized(&file);
    let gates: Vec<f32> = (0..256).map(|m| -12.0 + m as f32 / 16.0).collect();
    let ones = vec![1.0f32; 256];
    let mut body = source
        .replace(
            "[0.0, 1.0, (0.0 - 1.0), 2.0] : array{Float, 4}",
            &array(&gates),
        )
        .replace(
            "[3.0, 1.0, 0.5, 4.0] : array{Float, 4}",
            &array(&ones),
        )
        .replace(".n 4", ".n 256")
        .replace("array{Float, 4}", "array{Float, 256}")
        .replace(
            "compute.collect out.y : array{Float, 256}",
            "compute.collect act.a : array{Float, 256}",
        );
    if let Some(replacement) = spelling {
        body = body.replace("g / (1.0 + exp (0.0 - g))", replacement);
    }
    run_case(&format!("silu:{spelling:?}"), body, &file)
}

/// `exp` alone, over one of the two grids the note swept.
fn exponentials(name: &str, base: f32, step: f32) -> Score {
    let file = readme::example_dir().join("exp-probe.lichen");
    let arguments: Vec<f32> = (0..256).map(|m| base + m as f32 * step).collect();
    let program = format!(
        "---\n  compute = import \"compute.lichen\"\n  output = \"probe\"\n---\n\
         One  = struct{{.x (compute.Buf Float)}}\n\
         OutE = struct{{.y (compute.Buf Float)}}\n\
         ParE = compute.P (compute.KT Float)(.I One, .O OutE)\n\
         exp_raw = (k : ParE) => {{\n  i = compute.range k.n\n  \
         compute.write ((compute.Write _)(.to k.out.y, .at i, .value \
         (exp (compute.read ((compute.Read _)(.from k.in.x, .at i))) ~ \"element\")))\n}}\n\
         expon = compute.parallel (exp_raw ~ \"claim:Elementwise(element)\") \"cpu\"\n\
         x = (compute.buffer ({}) : (compute.Buf Float))\n\
         out = (compute.plrun expon ((compute.A One)(.n 256, .I One(.x x))) : OutE)\n\
         compute.collect out.y : array{{Float, 256}}\n",
        array(&arguments)
    );
    run_case(name, program, &file)
}

/// `rmsnorm`'s `rescale_raw`, its divisor read per lane; three spellings.
fn rmsnorm(spelling: &str) -> Score {
    let file = example("rmsnorm.lichen");
    let source = readme::read_normalized(&file);
    let numerators: Vec<f32> = (0..256).map(|index| grid(index % 64, 0)).collect();
    let divisors: Vec<f32> = (0..256).map(|m| 3.0 + m as f32 / 16.0).collect();
    let ones = vec![1.0f32; 256];
    let mut body = source
        .replace("[7.0, 3.0, 1.0, 2.0] : array{Float, 4}", &array(&numerators))
        .replace("[2.0, 4.0, 8.0, 16.0] : array{Float, 4}", &array(&ones))
        .replace(
            "sums = (compute.plrun sumsq ((compute.A Row)(.n 4, .I Row(.x x))) : Sums)",
            &format!(
                "ssweep = (compute.buffer ({}) : (compute.Buf Float))\n\
                 sums = (compute.plrun sumsq ((compute.A Row)(.n 256, .I Row(.x x))) : Sums)",
                array(&divisors)
            ),
        )
        .replace(".s sums.s", ".s ssweep")
        .replace(".n 4", ".n 256")
        .replace(
            "d = sqrt (compute.read ((compute.Read _)(.from k.in.s, .at 0)) / int2float k.n + 0.25)",
            "d = sqrt (compute.read ((compute.Read _)(.from k.in.s, .at i)) / 4.0 + 0.25)",
        )
        .replace("array{Float, 4}", "array{Float, 256}");
    body = match spelling {
        "sqrt" => body.replace(
            ".value ((x / d) * w ~ \"element\")",
            ".value (d ~ \"element\")",
        ),
        "div" => body.replace(
            ".value ((x / d) * w ~ \"element\")",
            ".value ((x / d) ~ \"element\")",
        ),
        _ => body,
    };
    if std::env::var("REVER_DUMP").is_ok() {
        let tail: String = body.lines().skip(28).collect::<Vec<_>>().join("\n");
        println!("{tail}");
        return Score::default();
    }
    run_case(&format!("rmsnorm:{spelling}"), body, &file)
}

/// `softmax`'s last line over a gathered divisor rather than the fold.
fn softmax_gathered(base: f32) -> Score {
    let file = example("softmax.lichen");
    let source = readme::read_normalized(&file);
    let exponents: Vec<f32> = (0..256).map(|m| base + m as f32 / 16.0).collect();
    let divisors: Vec<f32> = (0..256).map(|index| grid(index % 64, -4)).collect();
    let body = source
        .replace("[1.0, 2.0, 3.0, 4.0] : array{Float, 4}", &array(&exponents))
        .replace(
            "mx = (compute.plrun rowmax ((compute.A Row)(.n 4, .I Row(.x x))) : Maxs)",
            &format!(
                "tot = (compute.buffer ({}) : (compute.Buf Float))\n\
                 mx = (compute.plrun rowmax ((compute.A Row)(.n 256, .I Row(.x x))) : Maxs)",
                array(&divisors)
            ),
        )
        .replace(
            "m = compute.read ((compute.Read _)(.from k.in.m, .at 0))",
            "m = 0.0",
        )
        .replace(
            ".value (here / total)",
            ".value (here / (compute.read ((compute.Read _)(.from k.in.m, .at i))))",
        )
        .replace("Soft(.x x, .m mx.m)", "Soft(.x x, .m tot)")
        .replace(".n 4", ".n 256")
        .replace("array{Float, 4}", "array{Float, 256}");
    run_case(&format!("softmax:gathered:base{base}"), body, &file)
}

/// `softmax`'s real two-kernel body over the twelve rows the note names.
fn softmax_rows() -> Score {
    let file = example("softmax.lichen");
    let source = readme::read_normalized(&file);
    let rows: [&[f32]; 12] = [
        &[1.0, 2.0, 3.0, 4.0],
        &[0.0, -1.0, -2.0, -3.0],
        &[0.0, -0.25, -0.5, -0.75],
        &[0.0, -4.0, -8.0, -12.0],
        &[0.0, -8.0, -16.0, -24.0],
        &[7.0463777, 0.73105794, -0.13447031, 0.0],
        &[0.0, -2.0, -4.0, -6.0],
        &[0.0, 1.0, 2.0, 4.0],
        &[-3.0, -2.0, 1.0, 6.0],
        &[0.0, -0.5, -1.5, -4.5],
        &[3.0, -1.0, -2.0, -7.0463777],
        &[0.0, -0.125, -0.25, -0.375],
    ];
    let mut total = Score::default();
    for (row, values) in rows.iter().enumerate() {
        let body = source.replace("[1.0, 2.0, 3.0, 4.0] : array{Float, 4}", &array(values));
        let one = measure(&format!("row{row}"), &file, &body);
        println!(
            "ROW {row} {:?} points={} exact={} worst_ulp={}",
            values,
            one.points,
            one.exact,
            one.worst_ulp
        );
        total.merge(&one);
    }
    total.report("softmax:rows");
    total
}

/// The `rope` example's own row, and the same three kernels over a swept angle.
fn rope(sweep: bool) -> Score {
    let file = example("rope.lichen");
    let source = readme::read_normalized(&file);
    if !sweep {
        return run_case("rope:example", source, &file);
    }
    let angles: Vec<f32> = (0..256).map(|m| -8.0 + m as f32 / 16.0).collect();
    let ones = vec![1.0f32; 512];
    let body = source
        .replace("[0.0, 1.0, 2.0, 3.0] : array{Float, 4}", &array(&angles))
        .replace("[1.0, 1.0, 1.0, 1.0] : array{Float, 4}", &array(&angles))
        .replace(
            "[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0] : array{Float, 8}",
            &array(&ones),
        )
        .replace(".n 4", ".n 256")
        .replace(".n 8", ".n 512")
        .replace("array{Float, 4}", "array{Float, 256}")
        .replace("array{Float, 8}", "array{Float, 512}");
    run_case("rope:sweep", body, &file)
}

/// One example's own row, untouched.
fn untouched(name: &str, file_name: &str) -> Score {
    let file = example(file_name);
    let source = readme::read_normalized(&file);
    run_case(name, source, &file)
}

/// `gemv` with its short vector padded, the one spelling the device accepts.
fn gemv() -> Score {
    let file = example("gemv.lichen");
    let source = readme::read_normalized(&file);
    let body = source.replace(
        "[1.0, 2.0, 3.0] : array{Float, 3}",
        "[1.0, 2.0, 3.0, 0.0] : array{Float, 4}",
    );
    run_case("gemv:padded", body, &file)
}

fn main() {
    let wanted: Vec<String> = std::env::args().skip(1).collect();
    let asks: Vec<String> = if wanted.is_empty() {
        [
            "silu",
            "exp",
            "rmsnorm",
            "softmax",
            "rope",
            "rest",
        ]
        .iter()
        .map(|name| (*name).to_string())
        .collect()
    } else {
        wanted
    };
    println!(
        "PROBE driver={}",
        std::env::var("VK_DRIVER_FILES").unwrap_or_else(|_| "(default loader)".into())
    );
    if let Err(why) = lichen_compute_gpu::install_default() {
        println!("PROBE: no device backend: {why}");
        return;
    }
    for ask in &asks {
        let outcome = catch_unwind(AssertUnwindSafe(|| match ask.as_str() {
            "silu" => {
                silu(None);
                silu(Some("g / (1.0 + g)"));
                silu(Some("g / 4.0"));
            }
            "exp" => {
                exponentials("exp:fine", -2.0, 1.0 / 64.0);
                exponentials("exp:wide", -16.0, 1.0 / 8.0);
            }
            "rmsnorm" => {
                rmsnorm("body");
                rmsnorm("sqrt");
                rmsnorm("div");
            }
            "softmax" => {
                softmax_gathered(-16.0);
                softmax_gathered(0.0);
                softmax_rows();
            }
            "rope" => {
                rope(false);
                rope(true);
            }
            "rest" => {
                untouched("layernorm:example", "layernorm.lichen");
                untouched("attention:example", "attention.lichen");
                untouched("matmul:example", "matmul.lichen");
                gemv();
            }
            other => println!("CASE {other}: no such case"),
        }));
        if outcome.is_err() {
            println!("CASE {ask}: PANICKED — see the diagnostics above");
        }
    }
}
