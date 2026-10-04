//! Spike: can waffle lower a two-block loop with a carried value, and does the
//! result run under this workspace's `wasmi`?
//!
//! The question this answers is a version one. `wasmi` 2.0.0 validates every
//! module with `wasmparser` 0.228, and `waffle` 0.3.2 links `wasm-encoder` and
//! `wasmparser` 0.248. If 0.248's output is not accepted by 0.228's validator,
//! adopting waffle means changing the runtime too, which is a much larger
//! decision than changing the emitter.
//!
//! The loop built here is `countdown(n) -> n` walked to zero:
//!
//! ```text
//!   head(n): { cond = n == 0; }  if cond { done(count) } else { body(count) }
//!   body(n): { m = n - 1; }      br head(m)
//!   done(r): return r
//! ```
//!
//! `head`'s blockparam is the loop-carried value, and `body` passes `m` to it —
//! which is the whole feature, expressed as a CFG rather than as structured
//! control flow.

use waffle::{
    BlockTarget, Export, ExportKind, FuncDecl, FunctionBody, Module, Operator, Signature,
    SignatureData, Terminator, Type,
};

/// The function this spike compiles, as a `waffle` body.
///
/// **The entry block is the function's signature**: its blockparams *are* the
/// parameters (`FunctionBody::new` builds them), and they may not be added to. So
/// a loop needs a block of its own, entered through a preheader — which is the
/// same shape the kernel IR's `Flow::While` has, where the fragment's entry block
/// is the preheader and the loop's header is a block inside it.
///
/// ```text
///   entry(n): { }                    br head(n)
///   head(c):  { cond = c == 0 }      if cond { done(c) } else { body(c) }
///   body(c):  { one = 1; m = c - 1 } br head(m)
///   done(r):  return r
/// ```
fn countdown_body(module: &Module, sig: Signature) -> FunctionBody {
    let mut body = FunctionBody::new(module, sig);
    let entry = body.entry;
    let n = body.blocks[entry].params[0].1;

    let head = body.add_block();
    let body_block = body.add_block();
    let done = body.add_block();

    // entry: straight to the loop's head with the argument.
    body.set_terminator(
        entry,
        Terminator::Br {
            target: BlockTarget {
                block: head,
                args: vec![n],
            },
        },
    );

    // head: `cond = c == 0` — tested on every entry, including the first.
    //
    // **`i64.eqz` produces an `i32`**, which is what a `CondBr` condition is; the
    // kernel IR's `0`/`1` scalar is an `i64` and needs a narrowing, which is what
    // `KernelInstr::I32WrapI64` is for. This is the same fact the hand-written
    // emitter kept getting wrong, stated by the type checker instead.
    let c = body.add_blockparam(head, Type::I64);
    let cond = body.add_op(head, Operator::I64Eqz, &[c], &[Type::I32]);
    body.set_terminator(
        head,
        Terminator::CondBr {
            cond,
            if_true: BlockTarget {
                block: done,
                args: vec![c],
            },
            if_false: BlockTarget {
                block: body_block,
                args: vec![c],
            },
        },
    );

    // body: `m = c - 1`, then back to head with `m` — the carried value.
    let c2 = body.add_blockparam(body_block, Type::I64);
    let one = body.add_op(
        body_block,
        Operator::I64Const { value: 1 },
        &[],
        &[Type::I64],
    );
    let next = body.add_op(body_block, Operator::I64Sub, &[c2, one], &[Type::I64]);
    body.set_terminator(
        body_block,
        Terminator::Br {
            target: BlockTarget {
                block: head,
                args: vec![next],
            },
        },
    );

    // done: return the carried value.
    let r = body.add_blockparam(done, Type::I64);
    body.set_terminator(done, Terminator::Return { values: vec![r] });

    body
}

/// The entry block's blockparams are the function's parameters, so returning one
/// is the identity the spike asserts on.
#[test]
fn waffle_lowers_a_loop_and_wasmi_runs_it() {
    let mut module = Module::empty();
    let sig = module.signatures.push(SignatureData {
        params: vec![Type::I64],
        returns: vec![Type::I64],
    });
    let func = module.funcs.push(FuncDecl::None);
    module.funcs[func] = FuncDecl::Body(sig, "countdown".to_string(), {
        let body = countdown_body(&module, sig);
        body.validate().expect("the spike's IR must be SSA-valid");
        body.verify_reducible()
            .expect("the spike's CFG must be reducible");
        body
    });
    module.exports.push(Export {
        name: "countdown".to_string(),
        kind: ExportKind::Func(func),
    });

    let bytes = module
        .to_wasm_bytes()
        .expect("waffle must compile the module");
    std::fs::write("spike-countdown.wasm", &bytes).expect("write the spike's module");

    // **The question of the spike**: `wasmi` 2.0.0 validates with `wasmparser`
    // 0.228, and this module was produced by `wasm-encoder` 0.248.
    let engine = wasmi::Engine::default();
    let module = wasmi::Module::new(&engine, &bytes[..])
        .expect("the runtime's own validator must accept waffle's output");
    let mut store = wasmi::Store::new(&engine, ());
    let linker = wasmi::Linker::new(&engine);
    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .expect("the spike's module has no imports");
    let entry = instance
        .get_func(&store, "countdown")
        .expect("the export is there");

    for n in [0i64, 1, 4, 64, 1000] {
        let mut results = [wasmi::Val::I64(0)];
        entry
            .call(&mut store, &[wasmi::Val::I64(n)], &mut results)
            .expect("the loop must run");
        let wasmi::Val::I64(returned) = results[0] else {
            panic!("countdown returns an i64");
        };
        assert_eq!(
            returned, 0,
            "countdown({n}) must walk n to zero and return it"
        );
    }
}
