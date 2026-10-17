//! Probe: the float quotient `a / b`, isolated on both backends.
//!
//! One case per process: `install_default` fills a process-global backend slot
//! and the kernel registry is process-global too, so each invocation names one
//! case and `VK_DRIVER_FILES` names the device.

use std::panic::{AssertUnwindSafe, catch_unwind};

use lichen_tools::readme;

// ---------------------------------------------------------------- exact f32

/// `(significand, exponent)` of a normal `f32`, so `value == significand * 2^exponent`.
fn decompose(value: f32) -> (u32, i32) {
    let bits = value.to_bits();
    let exponent = ((bits >> 23) & 0xff) as i32;
    let mantissa = bits & 0x007f_ffff;
    assert!(
        exponent != 0 && exponent != 0xff,
        "the probe's grids stay inside the normal, finite range"
    );
    (mantissa | 0x0080_0000, exponent - 127 - 23)
}

/// Whether a lane's operands and quotient are all normal and finite, which is the
/// only range the integer reference and the residual are stated over.
fn scored(a: f32, b: f32) -> bool {
    let normal = |value: f32| {
        let field = (value.to_bits() >> 23) & 0xff;
        field != 0 && field != 0xff
    };
    normal(a) && normal(b) && normal(correctly_rounded(a, b))
}

/// `a / b` as the integer quotient `n1 / d1` and the power of two `2^shift`, with
/// `n1 / d1` in `[1, 2)`.
fn as_ratio(a: f32, b: f32) -> (u32, u32, i32) {
    let (ma, ea) = decompose(a);
    let (mb, eb) = decompose(b);
    let shift = ea - eb;
    if ma >= mb {
        (ma, mb, shift)
    } else {
        (2 * ma, mb, shift - 1)
    }
}

/// The 24-bit significand of the correctly rounded `a / b`, by integer division
/// with a round-half-to-even step. Returns `(significand, shift)`; the answer is
/// `significand * 2^(shift - 23)`.
fn rounded_significand(a: f32, b: f32) -> (u32, i32) {
    let (n1, d1, shift) = as_ratio(a, b);
    let scaled = (n1 as u128) << 23;
    let divisor = d1 as u128;
    let mut significand = scaled / divisor;
    let rest = scaled % divisor;
    let twice = 2 * rest;
    if twice > divisor || (twice == divisor && significand % 2 == 1) {
        significand += 1;
    }
    let mut significand = significand as u32;
    let mut shift = shift;
    if significand == 0x0100_0000 {
        significand >>= 1;
        shift += 1;
    }
    (significand, shift)
}

/// The correctly rounded `f32` of `a / b`, computed without the host's divide.
fn correctly_rounded(a: f32, b: f32) -> f32 {
    let (significand, shift) = rounded_significand(a, b);
    assemble(a.to_bits() & 0x8000_0000 != 0, significand, shift)
}

fn assemble(negative: bool, significand: u32, shift: i32) -> f32 {
    let exponent = shift + 127;
    assert!(
        (1..=254).contains(&exponent),
        "the probe's grids keep the quotient normal"
    );
    let bits = ((exponent as u32) << 23) | (significand & 0x007f_ffff);
    f32::from_bits(if negative { bits | 0x8000_0000 } else { bits })
}

/// Where the exact quotient sits inside its rounding interval, in units of one
/// ulp of the correctly rounded answer: `0` is the answer itself, `±1/2` is a
/// midpoint between it and its neighbour.
fn residual(a: f32, b: f32) -> f64 {
    let (n1, d1, _) = as_ratio(a, b);
    let scaled = (n1 as u128) << 23;
    let divisor = d1 as u128;
    let (significand, _) = rounded_significand(a, b);
    let exact = scaled as f64 / divisor as f64;
    exact - significand as f64
}

/// A value rounded to `24 - dropped` significand bits, keeping the rest.
fn drop_bits(value: f32, dropped: u32) -> f32 {
    assert!(value > 0.0 && value.is_finite());
    if dropped == 0 {
        return value;
    }
    let step = 1u32 << dropped;
    let bits = value.to_bits();
    let base = bits & !(step - 1);
    let half = bits & step;
    let round_up = half != 0 && (half != (step >> 1) || (base >> dropped) & 1 == 1);
    f32::from_bits(base + if round_up { step } else { 0 })
}

/// The reciprocal models this probe tests the device's answer against.
const RECIP_DROPS: [u32; 7] = [0, 1, 2, 3, 4, 5, 6];

fn model_exact(a: f32, b: f32) -> f32 {
    a / b
}

fn model_reciprocal(a: f32, b: f32) -> f32 {
    a * (1.0 / b)
}

fn model_reciprocal_dropped(dropped: u32) -> impl Fn(f32, f32) -> f32 {
    move |a, b| a * drop_bits(1.0 / b, dropped)
}

// ---------------------------------------------------------------- rendering

fn literal(value: f32) -> String {
    let text = format!("{value:?}");
    assert!(
        value.is_finite() && !text.contains('e') && !text.contains('E'),
        "a swept value rendered as {text}, which the source cannot carry"
    );
    match text.strip_prefix('-') {
        None => text,
        Some(magnitude) => format!("(0.0 - {magnitude})"),
    }
}

fn array(values: &[f32]) -> String {
    let body: Vec<String> = values.iter().copied().map(literal).collect();
    format!("[{}] : array{{Float, {}}}", body.join(", "), values.len())
}

fn ulps_apart(from: f32, to: f32) -> i64 {
    let key = |value: f32| {
        let bits = value.to_bits() as i32;
        i64::from(if bits < 0 { i32::MIN.wrapping_sub(bits) } else { bits })
    };
    (key(from) - key(to)).abs()
}

struct Element {
    token: String,
    value: f32,
    round_trips: bool,
}

fn elements(rendered: &str) -> Vec<Element> {
    let open = rendered
        .find('[')
        .unwrap_or_else(|| panic!("the program printed no array: {rendered}"));
    let close = rendered[open..].find(']').unwrap() + open;
    rendered[open + 1..close]
        .split(',')
        .map(|token| {
            let token = token.trim().to_string();
            let value = token.parse::<f32>().unwrap();
            Element {
                round_trips: format!("{value:?}") == token,
                token,
                value,
            }
        })
        .collect()
}

// ------------------------------------------------------------------ program

/// One `z[i] = <body>` kernel over two swept float buffers.
fn quotient_program(n: usize, a: &[f32], b: &[f32], body: &str) -> String {
    format!(
        "---\n  compute = import \"compute.lichen\"\n  output = \"probe\"\n---\n\
         Pair  = struct{{.a (compute.Buf Float), .b (compute.Buf Float)}}\n\
         OutZ  = struct{{.y (compute.Buf Float)}}\n\
         ParZ  = compute.P (compute.KT Float)(.I Pair, .O OutZ)\n\
         div_raw = (k : ParZ) => {{\n\
         \x20 i = compute.range k.n\n\
         \x20 qa = compute.read ((compute.Read _)(.from k.in.a, .at i))\n\
         \x20 qb = compute.read ((compute.Read _)(.from k.in.b, .at i))\n\
         \x20 qs = compute.read ((compute.Read _)(.from k.in.b, .at 0))\n\
         \x20 qn = int2float k.n\n{body}\n\
         \x20 compute.write ((compute.Write _)(.to k.out.y, .at i, .value (result ~ \"element\")))\n\
         }}\n\
         divs = compute.parallel (div_raw ~ \"claim:Elementwise(element)\") \"cpu\"\n\
         aa = (compute.buffer ({}) : (compute.Buf Float))\n\
         bb = (compute.buffer ({}) : (compute.Buf Float))\n\
         out = (compute.plrun divs ((compute.A Pair)(.n {n}, .I Pair(.a aa, .b bb))) : OutZ)\n\
         compute.collect out.y : array{{Float, {n}}}\n",
        array(a),
        array(b),
    )
}

/// Run `source` once per backend and return the two renderings element by element.
fn measure(source: &str) -> (Vec<Element>, Vec<Element>) {
    let file = readme::example_dir().join("div-probe.lichen");
    let on_cpu = readme::program_output(&file, source);
    let on_device = readme::program_output(&file, &source.replace("\"cpu\"", "\"gpu\""));
    let cpu = elements(&on_cpu);
    let device = elements(&on_device);
    assert_eq!(
        cpu.len(),
        device.len(),
        "the two backends printed different lengths"
    );
    (cpu, device)
}

// ------------------------------------------------------------------- report

/// One isolated-quotient case, scored against both references and against a set
/// of models for what the device might be computing.
struct Report {
    name: String,
    a: Vec<f32>,
    b: Vec<f32>,
    cpu: Vec<Element>,
    device: Vec<Element>,
}

impl Report {
    fn run(name: &str, source: &str, a: Vec<f32>, b: Vec<f32>) -> Report {
        let (cpu, device) = measure(source);
        Report {
            name: name.to_string(),
            a,
            b,
            cpu,
            device,
        }
    }

    fn report(&self) {
        let lanes: Vec<usize> = (0..self.a.len()).filter(|&i| scored(self.a[i], self.b[i])).collect();
        let n = lanes.len();
        // The reference question first: is the host's own divide correctly rounded?
        let host_matches = lanes
            .iter()
            .filter(|&&i| self.a[i] / self.b[i] == correctly_rounded(self.a[i], self.b[i]))
            .count();
        let cpu_matches = lanes
            .iter()
            .filter(|&&i| {
                self.cpu[i].value == correctly_rounded(self.a[i], self.b[i])
                    && self.cpu[i].round_trips
            })
            .count();
        let cpu_round_trips = lanes.iter().filter(|&&i| self.cpu[i].round_trips).count();
        let device_round_trips = lanes.iter().filter(|&&i| self.device[i].round_trips).count();
        let agree = lanes
            .iter()
            .filter(|&&i| self.cpu[i].token == self.device[i].token)
            .count();
        let agree_bits = lanes
            .iter()
            .filter(|&&i| self.cpu[i].value == self.device[i].value)
            .count();
        let worst = lanes
            .iter()
            .map(|&i| ulps_apart(self.device[i].value, self.cpu[i].value))
            .max()
            .unwrap_or(0);
        println!(
            "CASE {} scored={n}/{} host_=/_exact={host_matches} cpu_=_exact={cpu_matches} \
             cpu_round_trips={cpu_round_trips} device_round_trips={device_round_trips} \
             same_token={agree} same_bits={agree_bits} worst_ulp_vs_cpu={worst}",
            self.name,
            self.a.len()
        );

        // Models for the device's answer.
        type Predict = Box<dyn Fn(f32, f32) -> f32>;
        let models: Vec<(String, Predict)> = std::iter::once((
            "exact".to_string(),
            Box::new(model_exact) as Predict,
        ))
        .chain(std::iter::once((
            "recip".to_string(),
            Box::new(model_reciprocal) as Predict,
        )))
        .chain(RECIP_DROPS.into_iter().map(|drop| {
            (
                format!("recip_drop{drop}"),
                Box::new(model_reciprocal_dropped(drop)) as Predict,
            )
        }))
        .collect();
        for (name, model) in models {
            let hits = lanes
                .iter()
                .filter(|&&i| model(self.a[i], self.b[i]) == self.device[i].value)
                .count();
            if hits * 4 >= n * 3 {
                println!("  MODEL {name} explains {hits}/{n}");
            }
        }

        // The mover rule, from the exact residual.
        let mut worst_mover = 0.0f64;
        let mut best_quiet = 0.5f64;
        for &i in &lanes {
            let t = residual(self.a[i], self.b[i]).abs();
            if self.device[i].value == self.cpu[i].value {
                best_quiet = best_quiet.min(t);
            } else {
                worst_mover = worst_mover.max(t);
            }
        }
        println!("  MOVER residual: max over movers={worst_mover:.6} min over quiet={best_quiet:.6}");
    }
}

// -------------------------------------------------------------------- cases

/// `device-body-sweep.md` §2.1's numerators against one fixed divisor.
fn fixed_divisor(divisor: f32) -> Report {
    let a: Vec<f32> = (0..64).map(|m| grid(m, 0)).collect();
    let b = vec![divisor; 64];
    let body = "  result = qa / qb";
    Report::run(
        &format!("fixed:{divisor}"),
        &quotient_program(64, &a, &b, body),
        a,
        b,
    )
}

/// The full 64-by-64 product of §2.1's two grids.
fn grid_product() -> Report {
    let n = 4096;
    let a: Vec<f32> = (0..n).map(|m| grid(m % 64, 0)).collect();
    let b: Vec<f32> = (0..n).map(|m| grid((m / 64) % 64, -4)).collect();
    let body = "  result = qa / qb";
    Report::run(
        "grid:64x64",
        &quotient_program(n, &a, &b, body),
        a,
        b,
    )
}

/// One fixed divisor against every consecutive dyadic numerator in `[1, 2)`.
fn dense(divisor: f32) -> Report {
    let n = 4096;
    let a: Vec<f32> = (0..n).map(|m| 1.0 + m as f32 / 4096.0).collect();
    let b = vec![divisor; n];
    let body = "  result = qa / qb";
    Report::run(
        &format!("dense:{divisor}"),
        &quotient_program(n, &a, &b, body),
        a,
        b,
    )
}

/// `ln`'s own division, `(v - 1) / (v + 1)`, over the band its reduction lands in.
fn ln_band() {
    let n = 4096;
    let low = 0.707_106_78f32;
    let high = 1.414_213_5f32;
    let mut v: Vec<f32> = (0..n).map(|m| low + (high - low) * m as f32 / n as f32).collect();
    v[0] = 0.75;
    v[1] = 1.25;
    v[2] = 0.976_562_5;
    let body = "  result = (qa - 1.0) / (qa + 1.0)";
    let source = quotient_program(n, &v, &vec![1.0f32; n], body);
    let (cpu, device) = measure(&source);
    let same = (0..n)
        .filter(|&i| cpu[i].value == device[i].value)
        .count();
    let worst = (0..n)
        .map(|i| ulps_apart(device[i].value, cpu[i].value))
        .max()
        .unwrap_or(0);
    let by_recip = (0..n)
        .filter(|&i| {
            let numerator = v[i] - 1.0;
            let denominator = v[i] + 1.0;
            numerator * (1.0 / denominator) == device[i].value
        })
        .count();
    let cpu_round_trips = (0..n).filter(|&i| cpu[i].round_trips).count();
    let device_round_trips = (0..n).filter(|&i| device[i].round_trips).count();
    println!(
        "LNBAND n={n} same={same} worst_ulp={worst} recip_model={by_recip} \
         cpu_round_trips={cpu_round_trips} device_round_trips={device_round_trips}"
    );
    for lane in 0..3 {
        let numerator = v[lane] - 1.0;
        let denominator = v[lane] + 1.0;
        let reciprocal = numerator * (1.0 / denominator);
        println!(
            "  LN v={} denominator={} cpu={} cpu_bits=0x{:08x} device={} device_bits=0x{:08x} \
             ulp={} recip_model_bits=0x{:08x}",
            v[lane],
            denominator,
            cpu[lane].token,
            cpu[lane].value.to_bits(),
            device[lane].token,
            device[lane].value.to_bits(),
            ulps_apart(device[lane].value, cpu[lane].value),
            reciprocal.to_bits(),
        );
    }
}

/// `rmsnorm`'s own divisor: the Newton `sqrt` computed on the device, then the
/// isolated `x / d` beside it.
fn rmsnorm(spelling: &str) -> Report {
    let file = readme::example_dir().join("rmsnorm.lichen");
    let source = readme::read_normalized(&file);
    let n = 256;
    let a: Vec<f32> = (0..n).map(|m| grid(m % 64, 0)).collect();
    let divisors: Vec<f32> = (0..n).map(|m| 3.0 + m as f32 / 16.0).collect();
    let ones = vec![1.0f32; n];
    let mut body = source
        .replace("[7.0, 3.0, 1.0, 2.0] : array{Float, 4}", &array(&a))
        .replace("[2.0, 4.0, 8.0, 16.0] : array{Float, 4}", &array(&ones))
        .replace(
            "sums = (compute.plrun sumsq ((compute.A Row)(.n 4, .I Row(.x x))) : Sums)",
            &format!(
                "ssweep = (compute.buffer ({}) : (compute.Buf Float))\n\
                 sums = (compute.plrun sumsq ((compute.A Row)(.n {n}, .I Row(.x x))) : Sums)",
                array(&divisors)
            ),
        )
        .replace(".s sums.s", ".s ssweep")
        .replace(".n 4", &format!(".n {n}"))
        .replace(
            "d = sqrt (compute.read ((compute.Read _)(.from k.in.s, .at 0)) / int2float k.n + 0.25)",
            "d = sqrt (compute.read ((compute.Read _)(.from k.in.s, .at i)) / 4.0 + 0.25)",
        )
        .replace("array{Float, 4}", &format!("array{{Float, {n}}}"));
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
    // `d` is computed on the device, so the host's divisor is the definition's.
    let device_divisors: Vec<f32> = divisors
        .iter()
        .map(|s| (s / 4.0 + 0.25).sqrt())
        .collect();
    let (cpu, device) = measure(&body);
    let report = Report {
        name: format!("rmsnorm:{spelling}"),
        a,
        b: device_divisors,
        cpu,
        device,
    };
    report.report();
    report
}

/// `softmax`'s gathered divisor, `exp (x[i]) / total[i]`.
fn softmax_gathered() {
    let file = readme::example_dir().join("softmax.lichen");
    let source = readme::read_normalized(&file);
    let n = 256;
    let a: Vec<f32> = (0..n).map(|m| 0.0 + m as f32 / 16.0).collect();
    let totals: Vec<f32> = (0..n).map(|m| grid(m % 64, -4)).collect();
    let body = source
        .replace("[1.0, 2.0, 3.0, 4.0] : array{Float, 4}", &array(&a))
        .replace(
            "mx = (compute.plrun rowmax ((compute.A Row)(.n 4, .I Row(.x x))) : Maxs)",
            &format!(
                "tot = (compute.buffer ({}) : (compute.Buf Float))\n\
                 mx = (compute.plrun rowmax ((compute.A Row)(.n {n}, .I Row(.x x))) : Maxs)",
                array(&totals)
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
        .replace(".n 4", &format!(".n {n}"))
        .replace("array{Float, 4}", &format!("array{{Float, {n}}}"));
    let (cpu, device) = measure(&body);
    let all = (0..n).filter(|&i| cpu[i].value == device[i].value).count();
    let worst = (0..n)
        .map(|i| ulps_apart(device[i].value, cpu[i].value))
        .max()
        .unwrap_or(0);
    println!(
        "SOFTMAX_GATHERED n={n} same={all} worst_ulp={worst} \
         cpu_round_trips={} device_round_trips={}",
        (0..n).filter(|&i| cpu[i].round_trips).count(),
        (0..n).filter(|&i| device[i].round_trips).count(),
    );
}

/// Compensated spellings of `qa / qb`, over `rmsnorm`'s own divisor grid.
fn compensated(spelling: &str) {
    let n = 256;
    let a: Vec<f32> = (0..n).map(|m| grid(m % 64, 0)).collect();
    let b: Vec<f32> = (0..n).map(|m| (3.0 + m as f32 / 16.0 / 4.0 + 0.25).sqrt()).collect();
    let body = match spelling {
        "plain" => "  result = qa / qb".to_string(),
        "recip" => "  result = qa * (1.0 / qb)".to_string(),
        "nr1" => {
            "  rc0 = 1.0 / qb\n  rc = rc0 * (2.0 - qb * rc0)\n  result = qa * rc".to_string()
        }
        "rebind" => {
            "  rc = 1.0 / qb\n  rc = rc * (2.0 - qb * rc)\n  result = qa * rc".to_string()
        }
        "nr2" => {
            "  rc0 = 1.0 / qb\n  rc1 = rc0 * (2.0 - qb * rc0)\n  rc = rc1 * (2.0 - qb * rc1)\n  \
             result = qa * rc"
                .to_string()
        }
        "markstein" => {
            "  q0 = qa / qb\n  \
             sc = 4097.0\n  \
             bh = (qb * sc) - ((qb * sc) - qb)\n  \
             bl = qb - bh\n  \
             pr = qb * q0\n  \
             ph = (pr * sc) - ((pr * sc) - pr)\n  \
             pl = pr - ph\n  \
             qh = (q0 * sc) - ((q0 * sc) - q0)\n  \
             ql = q0 - qh\n  \
             ee = ((bh * qh - pr) + bh * ql + bl * qh) + bl * ql\n  \
             rr = (qa - pr) - ee\n  \
             result = q0 + rr * (1.0 / qb)"
                .to_string()
        }
        other => panic!("no such spelling: {other}"),
    };
    let source = quotient_program(n, &a, &b, &body);
    let (cpu, device) = measure(&source);
    let exact = (0..n)
        .filter(|&i| correctly_rounded(a[i], b[i]) == cpu[i].value)
        .count();
    let same = (0..n)
        .filter(|&i| cpu[i].value == device[i].value)
        .count();
    let worst = (0..n)
        .map(|i| ulps_apart(device[i].value, cpu[i].value))
        .max()
        .unwrap_or(0);
    let worse = (0..n)
        .filter(|&i| {
            ulps_apart(device[i].value, correctly_rounded(a[i], b[i]))
                > ulps_apart(cpu[i].value, correctly_rounded(a[i], b[i]))
        })
        .count();
    println!(
        "COMP spelling={spelling} n={n} cpu_is_exact={exact} device_=_cpu={same} \
         worst_ulp_vs_cpu={worst} lanes_worse_than_cpu={worse}"
    );
    if spelling == "markstein" {
        // The same sequence on the host, over the same grids the device ran, so a
        // shortfall is the device's and not the spelling's.
        let mut grids: Vec<(Vec<f32>, Vec<f32>, bool)> = Vec::new();
        grids.push((
            (0..4096).map(|m| grid(m % 64, 0)).collect(),
            (0..4096).map(|m| grid((m / 64) % 64, -4)).collect(),
            false,
        ));
        for divisor in [3.0f32, 7.0, 0.1, 2.5] {
            grids.push((
                (0..4096).map(|m| 1.0 + m as f32 / 4096.0).collect(),
                vec![divisor; 4096],
                false,
            ));
        }
        grids.push((
            (0..4096)
                .map(|m| 0.707_106_78 + (1.414_213_5 - 0.707_106_78) * m as f32 / 4096.0)
                .collect(),
            vec![1.0f32; 4096],
            true,
        ));
        let mut host_exact = 0usize;
        let mut host_total = 0usize;
        let mut host_worst = 0i64;
        for (numerators, divisors, atanh) in &grids {
            for i in 0..numerators.len() {
                let (numerator, denominator) = if *atanh {
                    (numerators[i] - 1.0, numerators[i] + 1.0)
                } else {
                    (numerators[i], divisors[i])
                };
                let right = correctly_rounded(numerator, denominator);
                let sc = 4097.0f32;
                let bh = (denominator * sc) - ((denominator * sc) - denominator);
                let bl = denominator - bh;
                let q0 = numerator / denominator;
                let pr = denominator * q0;
                let qh = (q0 * sc) - ((q0 * sc) - q0);
                let ql = q0 - qh;
                let ee = ((bh * qh - pr) + bh * ql + bl * qh) + bl * ql;
                let rr = (numerator - pr) - ee;
                let out = q0 + rr * (1.0 / denominator);
                host_total += 1;
                if out.to_bits() == right.to_bits() {
                    host_exact += 1;
                } else {
                    host_worst = host_worst.max(ulps_apart(out, right));
                }
            }
        }
        println!(
            "COMP host_markstein_exact={host_exact}/{host_total} host_worst_ulp={host_worst}"
        );
    }
}

/// The `f32` reciprocal the device's answers are consistent with, if one exists.
fn reciprocal_search() {
    for divisor in [3.0f32, 7.0, 2.5, 0.1] {
        let n = 4096;
        let a: Vec<f32> = (0..n).map(|m| 1.0 + m as f32 / 4096.0).collect();
        let (cpu, device) =
            measure(&quotient_program(n, &a, &vec![divisor; n], "  result = qa / qb"));
        let reciprocal = 1.0f32 / divisor;
        let low = (reciprocal * 0.75).to_bits();
        let high = (reciprocal * 1.25).to_bits();
        let mut open_low = low;
        let mut open_high = high;
        let mut empty = 0usize;
        for i in 0..n {
            let (lo, hi) = interval(a[i], device[i].value, low, high);
            if lo > hi {
                empty += 1;
                continue;
            }
            open_low = open_low.max(lo);
            open_high = open_high.min(hi);
        }
        let quiet = (0..n).filter(|&i| cpu[i].value == device[i].value).count();
        println!(
            "RECIP b={divisor} agreed={quiet}/{n} empty_intervals={empty} \
             r_low=0x{open_low:08x} r_high=0x{open_high:08x} width={} \
             correctly_rounded_recip=0x{:08x}",
            open_high as i64 - open_low as i64,
            reciprocal.to_bits(),
        );
    }
}

/// The key interval of `r` for which `a * r` rounds to `target`, for `a > 0`.
fn interval(a: f32, target: f32, low: u32, high: u32) -> (u32, u32) {
    let mut left = low;
    let mut right = high;
    while left < right {
        let middle = left + (right - left) / 2;
        if a * f32::from_bits(middle) < target {
            left = middle + 1;
        } else {
            right = middle;
        }
    }
    let first = left;
    let mut left = low;
    let mut right = high;
    while left < right {
        let middle = left + (right - left) / 2 + 1;
        if a * f32::from_bits(middle) <= target {
            left = middle;
        } else {
            right = middle - 1;
        }
    }
    (first, left)
}

fn grid(index: usize, shift: i32) -> f32 {
    (1.0 + (index % 8) as f32 / 8.0) * 2.0f32.powi((index / 8) as i32 + shift)
}

fn main() {
    let asked: Vec<String> = std::env::args().skip(1).collect();
    let asks: Vec<String> = if asked.is_empty() {
        [
            "selfcheck", "fixed", "grid", "dense", "recip", "ln", "rmsnorm", "softmax",
            "compensated",
        ]
        .iter()
        .map(|name| (*name).to_string())
        .collect()
    } else {
        asked
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
            "selfcheck" => selfcheck(),
            "fixed" => {
                for d in [1.0, 2.0, 4.0, 8.0, 3.0, 5.0, 7.0] {
                    fixed_divisor(d).report();
                }
            }
            "grid" => grid_product().report(),
            "dense" => {
                for d in [3.0, 5.0, 7.0, 4.0, 0.1, 2.5] {
                    dense(d).report();
                }
            }
            "recip" => reciprocal_search(),
            "ln" => {
                ln_band();
            }
            "rmsnorm" => {
                rmsnorm("sqrt");
                rmsnorm("div");
                rmsnorm("body");
            }
            "softmax" => {
                softmax_gathered();
            }
            "compensated" => {
                for spelling in ["plain", "recip", "rebind", "nr1", "nr2", "markstein"] {
                    let wanted = std::env::var("COMP_ONLY").unwrap_or_default();
                    if wanted.is_empty() || wanted == spelling {
                        compensated(spelling);
                    }
                }
            }
            other => println!("CASE {other}: no such case"),
        }));
        if outcome.is_err() {
            println!("CASE {ask}: PANICKED");
        }
    }
}

/// Is the host's own `f32` divide correctly rounded? Answered over a large sweep
/// with the integer reference above, and with the model every `f32` division
/// would have to beat.
fn selfcheck() {
    let mut exact = 0usize;
    let mut total = 0usize;
    let mut worst = 0i64;
    for e in -30..30 {
        for m in 0..100_000u32 {
            let significand =
                0x0080_0000u32.wrapping_add(m.wrapping_mul(2_654_435_761) % 0x007f_ffff);
            let a = f32::from_bits(((e + 127) as u32) << 23 | (significand & 0x007f_ffff));
            let b = f32::from_bits(
                ((e + 127) as u32) << 23 | (significand.wrapping_mul(7) & 0x007f_ffff),
            );
            if a == 0.0 || b == 0.0 {
                continue;
            }
            let q = a / b;
            if !(q.is_finite() && (q.to_bits() & 0x7f80_0000) != 0) {
                continue;
            }
            total += 1;
            let reference = correctly_rounded(a, b);
            if q.to_bits() == reference.to_bits() {
                exact += 1;
            } else {
                worst = worst.max(ulps_apart(q, reference));
            }
        }
    }
    println!(
        "SELFCHECK host_divide_is_exact={exact}/{total} worst_ulp_when_not={worst} \
         model_exact_is_cpu={}",
        model_exact(1.0, 3.0)
    );
}
