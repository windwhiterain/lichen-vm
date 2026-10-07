//! A ladder of GPU algorithms, run against whichever backend is named.
//!
//! This is exploration tooling, not a test: it prints what each program answers
//! and prints the refusal when it cannot run, so a gap in the language shows up
//! as a named refusal rather than as a failing assertion somewhere else.

use lichen_language::package::PackageStore;
use lichen_language::program::LangProgram;

fn run(source: &str) -> Result<String, Vec<String>> {
    let mut store = PackageStore::<LangProgram>::new();
    lichen_language::run::evaluate_raw(source, None, &mut store)
        .map_err(|diags| diags.into_iter().map(|d| d.message).collect())
}

/// `(name, source)` — each is a complete program.
const PROGRAMS: &[(&str, &str)] = &[
    (
        "1. fill — write a constant per index",
        r#"
--- compute = import "compute.lichen" ---
In  = struct<.a Int>
Out = struct<.z (compute.Buf _)>
Par = compute.P (compute.KT _)(.I In, .O Out)
f = (k : Par) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value 7))
}
k = compute.parallel f "BACKEND"
out = (compute.plrun k ((compute.A In)(.n 8, .I In(.a 0))) : Out)
compute.collect out.z
"#,
    ),
    (
        "2. axpy — y = a*x + y, two inputs one output",
        r#"
--- compute = import "compute.lichen" ---
In1  = struct<.a Int>
Out1 = struct<.z (compute.Buf _)>
Par1 = compute.P (compute.KT _)(.I In1, .O Out1)
mk = (k : Par1) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 1))
}
kx = compute.parallel mk "BACKEND"
x = (compute.plrun kx ((compute.A In1)(.n 8, .I In1(.a 0))) : Out1)
ky = compute.parallel mk "BACKEND"
y = (compute.plrun ky ((compute.A In1)(.n 8, .I In1(.a 0))) : Out1)
In2  = struct<.x (compute.Buf _), .y (compute.Buf _)>
Out2 = struct<.z (compute.Buf _)>
Par2 = compute.P (compute.KT _)(.I In2, .O Out2)
axpy = (k : Par2) => {
  i = compute.range k.n
  xv = compute.read ((compute.Read _)(.from k.in.x, .at i))
  yv = compute.read ((compute.Read _)(.from k.in.y, .at i))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value 3 * xv + yv))
}
ka = compute.parallel axpy "BACKEND"
out = (compute.plrun ka ((compute.A In2)(.n 8, .I In2(.x x.z, .y y.z))) : Out2)
compute.collect out.z
"#,
    ),
    (
        "3. transpose — gather, write index is not the read index",
        r#"
--- compute = import "compute.lichen" ---
In1  = struct<.a Int>
Out1 = struct<.z (compute.Buf _)>
Par1 = compute.P (compute.KT _)(.I In1, .O Out1)
n0 = (k : Par1) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i))
}
k0 = compute.parallel n0 "BACKEND"
src = (compute.plrun k0 ((compute.A In1)(.n 8, .I In1(.a 0))) : Out1)
In2  = struct<.s (compute.Buf _)>
Out2 = struct<.z (compute.Buf _)>
Par2 = compute.P (compute.KT _)(.I In2, .O Out2)
tr = (k : Par2) => {
  i = compute.range k.n
  v = compute.read ((compute.Read _)(.from k.in.s, .at i % 4))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value v))
}
kt = compute.parallel tr "BACKEND"
out = (compute.plrun kt ((compute.A In2)(.n 8, .I In2(.s src.z))) : Out2)
compute.collect out.z
"#,
    ),
    (
        "4. stencil — a neighbour read at a computed, clamped index",
        r#"
--- compute = import "compute.lichen" ---
In1  = struct<.a Int>
Out1 = struct<.z (compute.Buf _)>
Par1 = compute.P (compute.KT _)(.I In1, .O Out1)
n0 = (k : Par1) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value 1))
}
k0 = compute.parallel n0 "BACKEND"
src = (compute.plrun k0 ((compute.A In1)(.n 8, .I In1(.a 0))) : Out1)
In2  = struct<.s (compute.Buf _)>
Out2 = struct<.z (compute.Buf _)>
Par2 = compute.P (compute.KT _)(.I In2, .O Out2)
st = (k : Par2) => {
  i = compute.range k.n
  left = if i == 0 then 0 else i - 1
  v = compute.read ((compute.Read _)(.from k.in.s, .at left))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value v))
}
ks = compute.parallel st "BACKEND"
out = (compute.plrun ks ((compute.A In2)(.n 8, .I In2(.s src.z))) : Out2)
compute.collect out.z
"#,
    ),
    (
        "5. select-into-read — clamp by arithmetic, no conditional",
        r#"
--- compute = import "compute.lichen" ---
In1  = struct<.a Int>
Out1 = struct<.z (compute.Buf _)>
Par1 = compute.P (compute.KT _)(.I In1, .O Out1)
n0 = (k : Par1) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value 1))
}
k0 = compute.parallel n0 "BACKEND"
src = (compute.plrun k0 ((compute.A In1)(.n 8, .I In1(.a 0))) : Out1)
In2  = struct<.s (compute.Buf _)>
Out2 = struct<.z (compute.Buf _)>
Par2 = compute.P (compute.KT _)(.I In2, .O Out2)
st = (k : Par2) => {
  i = compute.range k.n
  j = i - (i == 0) * i
  v = compute.read ((compute.Read _)(.from k.in.s, .at j))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value v))
}
ks = compute.parallel st "BACKEND"
out = (compute.plrun ks ((compute.A In2)(.n 8, .I In2(.s src.z))) : Out2)
compute.collect out.z
"#,
    ),
    (
        "6. dot4 — an unrolled 4-term reduction, fixed length",
        r#"
--- compute = import "compute.lichen" ---
In1  = struct<.a Int>
Out1 = struct<.z (compute.Buf _)>
Par1 = compute.P (compute.KT _)(.I In1, .O Out1)
n0 = (k : Par1) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value 1))
}
k0 = compute.parallel n0 "BACKEND"
src = (compute.plrun k0 ((compute.A In1)(.n 4, .I In1(.a 0))) : Out1)
In2  = struct<.s (compute.Buf _)>
Out2 = struct<.z (compute.Buf _)>
Par2 = compute.P (compute.KT _)(.I In2, .O Out2)
dot = (k : Par2) => {
  i = compute.range k.n
  a = compute.read ((compute.Read _)(.from k.in.s, .at 0))
  b = compute.read ((compute.Read _)(.from k.in.s, .at 1))
  c = compute.read ((compute.Read _)(.from k.in.s, .at 2))
  d = compute.read ((compute.Read _)(.from k.in.s, .at 3))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value a * a + b * b + c * c + d * d))
}
kd = compute.parallel dot "BACKEND"
out = (compute.plrun kd ((compute.A In2)(.n 1, .I In2(.s src.z))) : Out2)
compute.read ((compute.Read _)(.from out.z, .at 0))
"#,
    ),
    (
        "7. mat2 — a correct 2x2 multiply",
        r#"
--- compute = import "compute.lichen" ---
In1  = struct<.a Int>
Out1 = struct<.z (compute.Buf _)>
Par1 = compute.P (compute.KT _)(.I In1, .O Out1)
n0 = (k : Par1) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 1))
}
k0 = compute.parallel n0 "BACKEND"
src = (compute.plrun k0 ((compute.A In1)(.n 4, .I In1(.a 0))) : Out1)
In2  = struct<.a (compute.Buf _), .b (compute.Buf _)>
Out2 = struct<.p (compute.Buf _), .q (compute.Buf _)>
Par2 = compute.P (compute.KT _)(.I In2, .O Out2)
mm = (k : Par2) => {
  i = compute.range k.n
  row = i / 2
  col = i % 2
  a00 = compute.read ((compute.Read _)(.from k.in.a, .at 0))
  a01 = compute.read ((compute.Read _)(.from k.in.a, .at 1))
  a10 = compute.read ((compute.Read _)(.from k.in.a, .at 2))
  a11 = compute.read ((compute.Read _)(.from k.in.a, .at 3))
  b0 = compute.read ((compute.Read _)(.from k.in.b, .at col * 2 + 0))
  b1 = compute.read ((compute.Read _)(.from k.in.b, .at col * 2 + 1))
  (compute.write ((compute.Write _)(.to k.out.p, .at i, .value a00 * b0 + a01 * b1)),
   compute.write ((compute.Write _)(.to k.out.q, .at i, .value a10 * b0 + a11 * b1)))
}
km = compute.parallel mm "BACKEND"
outs = (compute.plrun km ((compute.A In2)(.n 4, .I In2(.a src.z, .b src.z))) : Out2)
(compute.collect outs.p, compute.collect outs.q)
"#,
    ),
    (
        "8. reduction — a sum, the classic cross-index algorithm",
        r#"
--- compute = import "compute.lichen" ---
In1  = struct<.a Int>
Out1 = struct<.z (compute.Buf _)>
Par1 = compute.P (compute.KT _)(.I In1, .O Out1)
n0 = (k : Par1) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value 1))
}
k0 = compute.parallel n0 "BACKEND"
src = (compute.plrun k0 ((compute.A In1)(.n 8, .I In1(.a 0))) : Out1)
In2  = struct<.s (compute.Buf _)>
Out2 = struct<.z (compute.Buf _)>
Par2 = compute.P (compute.KT _)(.I In2, .O Out2)
red = (k : Par2) => {
  i = compute.range k.n
  a = compute.read ((compute.Read _)(.from k.in.s, .at i))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value a + 1))
}
kr = compute.parallel red "BACKEND"
out = (compute.plrun kr ((compute.A In2)(.n 8, .I In2(.s src.z))) : Out2)
compute.read ((compute.Read _)(.from out.z, .at 3))
"#,
    ),
    (
        "9. two-buffer contraction — does a write and a read share an index space?",
        r#"
--- compute = import "compute.lichen" ---
In1  = struct<.a Int>
Out1 = struct<.z (compute.Buf _)>
Par1 = compute.P (compute.KT _)(.I In1, .O Out1)
n0 = (k : Par1) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value 2))
}
k0 = compute.parallel n0 "BACKEND"
src = (compute.plrun k0 ((compute.A In1)(.n 8, .I In1(.a 0))) : Out1)
In2  = struct<.s (compute.Buf _)>
Out2 = struct<.z (compute.Buf _)>
Par2 = compute.P (compute.KT _)(.I In2, .O Out2)
mix = (k : Par2) => {
  i = compute.range k.n
  a = compute.read ((compute.Read _)(.from k.in.s, .at i))
  b = compute.read ((compute.Read _)(.from k.in.s, .at i - i))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value a + b))
}
km = compute.parallel mix "BACKEND"
out = (compute.plrun km ((compute.A In2)(.n 8, .I In2(.s src.z))) : Out2)
compute.read ((compute.Read _)(.from out.z, .at 3))
"#,
    ),
    (
        "8b. tree reduction — halve the length, four levels",
        r#"
--- compute = import "compute.lichen" ---
In1  = struct<.a Int>
Out1 = struct<.z (compute.Buf _)>
Par1 = compute.P (compute.KT _)(.I In1, .O Out1)
n0 = (k : Par1) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value 1))
}
k0 = compute.parallel n0 "BACKEND"
a = (compute.plrun k0 ((compute.A In1)(.n 16, .I In1(.a 0))) : Out1)
In2  = struct<.s (compute.Buf _)>
Out2 = struct<.z (compute.Buf _)>
Par2 = compute.P (compute.KT _)(.I In2, .O Out2)
pair = (k : Par2) => {
  i = compute.range k.n
  x = compute.read ((compute.Read _)(.from k.in.s, .at i * 2))
  y = compute.read ((compute.Read _)(.from k.in.s, .at i * 2 + 1))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value x + y))
}
kp = compute.parallel pair "BACKEND"
b = (compute.plrun kp ((compute.A In2)(.n 8, .I In2(.s a.z))) : Out2)
c = (compute.plrun kp ((compute.A In2)(.n 4, .I In2(.s b.z))) : Out2)
d = (compute.plrun kp ((compute.A In2)(.n 2, .I In2(.s c.z))) : Out2)
e = (compute.plrun kp ((compute.A In2)(.n 1, .I In2(.s d.z))) : Out2)
compute.read ((compute.Read _)(.from e.z, .at 0))
"#,
    ),
    (
        "8c. prefix sum — a scan, one write per index from many reads",
        r#"
--- compute = import "compute.lichen" ---
In1  = struct<.a Int>
Out1 = struct<.z (compute.Buf _)>
Par1 = compute.P (compute.KT _)(.I In1, .O Out1)
n0 = (k : Par1) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value 1))
}
k0 = compute.parallel n0 "BACKEND"
src = (compute.plrun k0 ((compute.A In1)(.n 4, .I In1(.a 0))) : Out1)
In2  = struct<.s (compute.Buf _)>
Out2 = struct<.z (compute.Buf _)>
Par2 = compute.P (compute.KT _)(.I In2, .O Out2)
scan = (k : Par2) => {
  i = compute.range k.n
  a = compute.read ((compute.Read _)(.from k.in.s, .at 0))
  b = compute.read ((compute.Read _)(.from k.in.s, .at 1))
  c = compute.read ((compute.Read _)(.from k.in.s, .at 2))
  d = compute.read ((compute.Read _)(.from k.in.s, .at 3))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value a + b + c + d))
}
ks = compute.parallel scan "BACKEND"
out = (compute.plrun ks ((compute.A In2)(.n 4, .I In2(.s src.z))) : Out2)
compute.collect out.z
"#,
    ),
    (
        "8d. out-of-range read — is a bad index refused?",
        r#"
--- compute = import "compute.lichen" ---
In1  = struct<.a Int>
Out1 = struct<.z (compute.Buf _)>
Par1 = compute.P (compute.KT _)(.I In1, .O Out1)
n0 = (k : Par1) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 1))
}
k0 = compute.parallel n0 "BACKEND"
src = (compute.plrun k0 ((compute.A In1)(.n 4, .I In1(.a 0))) : Out1)
In2  = struct<.s (compute.Buf _)>
Out2 = struct<.z (compute.Buf _)>
Par2 = compute.P (compute.KT _)(.I In2, .O Out2)
oob = (k : Par2) => {
  i = compute.range k.n
  v = compute.read ((compute.Read _)(.from k.in.s, .at 4))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value v))
}
ko = compute.parallel oob "BACKEND"
out = (compute.plrun ko ((compute.A In2)(.n 2, .I In2(.s src.z))) : Out2)
compute.collect out.z
"#,
    ),
    (
        "8e. scalar kernel parameter — is anything but a buffer allowed?",
        r#"
--- compute = import "compute.lichen" ---
In1  = struct<.a Int>
Out1 = struct<.z (compute.Buf _)>
Par1 = compute.P (compute.KT _)(.I In1, .O Out1)
n0 = (k : Par1) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 1))
}
k0 = compute.parallel n0 "BACKEND"
src = (compute.plrun k0 ((compute.A In1)(.n 4, .I In1(.a 0))) : Out1)
In2  = struct<.s (compute.Buf _), .k Int>
Out2 = struct<.z (compute.Buf _)>
Par2 = compute.P (compute.KT _)(.I In2, .O Out2)
sc = (k : Par2) => {
  i = compute.range k.n
  v = compute.read ((compute.Read _)(.from k.in.s, .at i))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value v * k.in.k))
}
ks = compute.parallel sc "BACKEND"
out = (compute.plrun ks ((compute.A In2)(.n 4, .I In2(.s src.z, .k 5))) : Out2)
compute.collect out.z
"#,
    ),
    (
        "9. cross-kernel call — a shared helper inside a parallel body",
        r#"
--- compute = import "compute.lichen" ---
helper = compute.jit (v : Int => v * v : Int)
In1  = struct<.a Int>
Out1 = struct<.z (compute.Buf _)>
Par1 = compute.P (compute.KT _)(.I In1, .O Out1)
n0 = (k : Par1) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 1))
}
k0 = compute.parallel n0 "BACKEND"
src = (compute.plrun k0 ((compute.A In1)(.n 8, .I In1(.a 0))) : Out1)
In2  = struct<.s (compute.Buf _)>
Out2 = struct<.z (compute.Buf _)>
Par2 = compute.P (compute.KT _)(.I In2, .O Out2)
use = (k : Par2) => {
  i = compute.range k.n
  v = compute.read ((compute.Read _)(.from k.in.s, .at i))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value helper.native v))
}
ku = compute.parallel use "BACKEND"
out = (compute.plrun ku ((compute.A In2)(.n 8, .I In2(.s src.z))) : Out2)
compute.collect out.z
"#,
    ),
    (
        "9b. histogram — a scatter-accumulate into a shared slot",
        r#"
--- compute = import "compute.lichen" ---
In1  = struct<.a Int>
Out1 = struct<.z (compute.Buf _)>
Par1 = compute.P (compute.KT _)(.I In1, .O Out1)
n0 = (k : Par1) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i % 3))
}
k0 = compute.parallel n0 "BACKEND"
src = (compute.plrun k0 ((compute.A In1)(.n 64, .I In1(.a 0))) : Out1)
In2  = struct<.s (compute.Buf _)>
Out2 = struct<.z (compute.Buf _)>
Par2 = compute.P (compute.KT _)(.I In2, .O Out2)
hist = (k : Par2) => {
  i = compute.range k.n
  key = compute.read ((compute.Read _)(.from k.in.s, .at i))
  compute.write ((compute.Write _)(.to k.out.z, .at key, .value 1))
}
kh = compute.parallel hist "BACKEND"
out = (compute.plrun kh ((compute.A In2)(.n 64, .I In2(.s src.z))) : Out2)
(compute.read ((compute.Read _)(.from out.z, .at 0)), compute.read ((compute.Read _)(.from out.z, .at 1)), compute.read ((compute.Read _)(.from out.z, .at 2)))
"#,
    ),
    (
        "10. host data in — an input array the program owns",
        r#"
--- compute = import "compute.lichen" ---
data = [3, 1, 4, 1, 5, 9, 2, 6]
In  = struct<.a (compute.Buf _)>
Out = struct<.z (compute.Buf _)>
Par = compute.P (compute.KT _)(.I In, .O Out)
f = (k : Par) => {
  i = compute.range k.n
  v = compute.read ((compute.Read _)(.from k.in.a, .at i))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value v + 1))
}
kk = compute.parallel f "BACKEND"
out = (compute.plrun kk ((compute.A In)(.n 8, .I In(.a data))) : Out)
compute.collect out.z
"#,
    ),
    (
        "11. float — is there a real number at all?",
        r#"
x = 1.5
x + 1
"#,
    ),
    (
        "12. graph — one submission for a two-link chain",
        r#"
--- compute = import "compute.lichen" ---
In1  = struct<.a Int>
Out1 = struct<.z (compute.Buf _)>
Par1 = compute.P (compute.KT _)(.I In1, .O Out1)
f1 = (k : Par1) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 10))
}
k1 = compute.parallel f1 "BACKEND"
In2  = struct<.b (compute.Buf _)>
Out2 = struct<.w (compute.Buf _)>
Par2 = compute.P (compute.KT _)(.I In2, .O Out2)
f2 = (k : Par2) => {
  i = compute.range k.n
  a = compute.read ((compute.Read _)(.from k.in.b, .at i))
  compute.write ((compute.Write _)(.to k.out.w, .at i, .value a + a))
}
k2 = compute.parallel f2 "BACKEND"
step = ins => {
  first = (compute.plrun k1 ((compute.A In1)(.n ins(0), .I In1(.a 0))) : Out1)
  compute.plrun k2 ((compute.A In2)(.n ins(0), .I In2(.b first.z)))
}
built = compute.graph step
out = compute.graphrun built (4,)
compute.collect out.z
"#,
    ),
];

fn main() {
    let backends: Vec<String> = std::env::args().skip(1).collect();
    let backends: Vec<String> = if backends.is_empty() {
        vec!["cpu".to_string()]
    } else {
        backends
    };

    for backend in backends {
        println!("\n=== backend: {backend} ===");
        let gpu = backend == "gpu";
        if gpu {
            match lichen_compute_gpu::install_default() {
                Ok(()) => println!("(device installed)"),
                Err(reason) => {
                    println!("no device: {reason}");
                    continue;
                }
            }
        }
        for (name, template) in PROGRAMS {
            let source = template.replace("BACKEND", &backend);
            print!("{name}\n  ");
            match run(&source) {
                Ok(out) => println!("OK  {out}"),
                Err(diags) => {
                    println!("REFUSED");
                    for message in diags {
                        println!("      {message}");
                    }
                }
            }
        }
        if gpu {
            lichen_compute_gpu::uninstall();
        }
    }
}
