//! A fused kernel reads back as itself, and a text naming nothing is refused by
//! field (`docs/notes/graph-fusion.md` §5).
use lichen_compute::{FusedKernel, FusedRefusal, read_fused_kernel, write_fused_kernel};
use lichen_kernel_ir::{
    IntWidth, KernelBin, KernelBody, KernelClaim, KernelFragment, KernelInstr, KernelMark,
    KernelRoles, KernelShape, LaneWidth, MarkField, Plan, PlanRefusal, ScalarClass, Slice, Space,
    Staging, TargetFacts, Terminator, ValueId,
};

/// The lane count one workgroup runs.
const LANES: u32 = 64;

/// The mark the staged value carries.
const TILE: &str = "tile";

/// A body that stages `7` into the slice at its own index.
///
/// # Invariant
/// The mark rides the slice write's **result** — the version — because that
/// value's producer is the staging itself, which is the value a plan names.
fn staged_body() -> (KernelBody, ValueId) {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let _extent = body.add_param(entry);
    let index = body.add_param(entry);
    let ordinal = body.add_const(entry, ScalarClass::Int, 0);
    let written = body.add_const(entry, ScalarClass::Int, 7);
    let staged = body.add_op(
        entry,
        KernelInstr::SliceWriteCall(ScalarClass::Int),
        vec![ordinal, index, written],
        vec![ScalarClass::Int],
    );
    body.set_terminator(
        entry,
        Terminator::Return {
            values: vec![staged],
        },
    );
    (body, staged)
}

/// The fragment that body is dispatched as, staging into `space`.
fn fragment(space: Space) -> (KernelFragment, ValueId) {
    let (body, staged) = staged_body();
    let fragment = KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body,
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Int],
        output_classes: vec![ScalarClass::Int],
        result_classes: vec![ScalarClass::Int],
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(LANES),
        },
        slices: vec![Slice {
            class: ScalarClass::Int,
            count: LANES,
            space,
        }],
        marks: vec![KernelMark {
            value: staged,
            payload: vec![MarkField {
                name: None,
                text: TILE.to_string(),
            }],
        }],
    };
    (fragment, staged)
}

/// The plan that stages the marked value in `space`.
fn plan(space: Space) -> Plan {
    Plan {
        staging: vec![Staging {
            name: TILE.to_string(),
            space,
            group_width: LANES,
        }],
    }
}

/// The kernel the hand-built fragment and plan name.
fn staged_kernel() -> FusedKernel {
    let (fragment, _) = fragment(Space::Shared);
    FusedKernel {
        fragment,
        claim: Some(KernelClaim {
            interface: "Reduction".to_string(),
            roles: vec!["combine".to_string(), "identity".to_string()],
        }),
        plan: plan(Space::Shared),
    }
}

#[test]
fn a_fused_kernel_reads_back_as_the_kernel_it_wrote() {
    let kernel = staged_kernel();
    let text = write_fused_kernel(&kernel);
    assert_eq!(
        read_fused_kernel(&text),
        Ok(kernel),
        "the round trip must hand back the kernel the text named"
    );
}

#[test]
fn a_kernel_with_no_claim_and_an_empty_plan_reads_back() {
    let (fragment, _) = fragment(Space::Register);
    let kernel = FusedKernel {
        fragment,
        claim: None,
        plan: Plan::default(),
    };
    let text = write_fused_kernel(&kernel);
    assert!(
        text.contains("\"claim\": null"),
        "a kernel no claim named says so: {text}"
    );
    assert_eq!(read_fused_kernel(&text), Ok(kernel));
}

#[test]
fn a_hand_written_literal_reads_back_as_the_fragment_it_spells() {
    let text = r#"
    {
      "fragment": {
        "param_shape": { "tuple": [{ "scalar": "int" }, { "scalar": "int" }] },
        "roles": {
          "scalars": [[0], [1]],
          "inputs": [[0]],
          "outputs": [[1]],
          "input_classes": ["float"],
          "output_classes": ["int"]
        },
        "body": {
          "values": [
            { "block_param": { "block": 0 } },
            { "block_param": { "block": 0 } },
            { "instr": { "op": { "const": { "class": "int", "bits": 0 } }, "args": [], "classes": ["int"] } },
            { "instr": { "op": { "const": { "class": "int", "bits": 7 } }, "args": [], "classes": ["int"] } },
            { "instr": { "op": { "slice_write_call": { "class": "int" } }, "args": [2, 1, 3], "classes": ["int"] } }
          ],
          "blocks": [
            {
              "params": [0, 1],
              "instrs": [2, 3, 4],
              "terminator": { "return": { "values": [4] } }
            }
          ],
          "entry": 0,
          "barriers": []
        },
        "inputs": 1,
        "outputs": 1,
        "input_classes": ["float"],
        "output_classes": ["int"],
        "result_classes": ["int"],
        "target": { "int_width": "i64", "lane_width": 64 },
        "slices": [{ "class": "int", "count": 64, "space": "shared" }],
        "marks": [{ "value": 4, "payload": [{ "name": null, "text": "tile" }] }]
      },
      "claim": null,
      "plan": { "staging": [{ "name": "tile", "space": "shared", "group_width": 64 }] }
    }
    "#;
    let kernel = read_fused_kernel(text).expect("the literal is a well-formed fused kernel");
    let expected = KernelFragment {
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        roles: KernelRoles {
            scalars: vec![vec![0], vec![1]],
            inputs: vec![vec![0]],
            outputs: vec![vec![1]],
            input_classes: vec![Some(ScalarClass::Float)],
            output_classes: vec![Some(ScalarClass::Int)],
        },
        body: staged_body().0,
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Float],
        output_classes: vec![ScalarClass::Int],
        result_classes: vec![ScalarClass::Int],
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(LANES),
        },
        slices: vec![Slice {
            class: ScalarClass::Int,
            count: LANES,
            space: Space::Shared,
        }],
        marks: vec![KernelMark {
            value: ValueId(4),
            payload: vec![MarkField {
                name: None,
                text: TILE.to_string(),
            }],
        }],
    };
    assert_eq!(kernel.fragment, expected);
    assert_eq!(kernel.claim, None);
    assert_eq!(kernel.plan, plan(Space::Shared));
    assert_eq!(
        read_fused_kernel(&write_fused_kernel(&kernel)),
        Ok(kernel),
        "a text a human wrote round trips like one this crate wrote"
    );
}

#[test]
fn text_that_is_not_json_is_refused_with_where() {
    let refusal = read_fused_kernel("fragment = 3").expect_err("a non-JSON text names no kernel");
    assert!(
        matches!(&refusal, FusedRefusal::Malformed { at: Some(_), .. }),
        "the refusal points into the text: {refusal}"
    );
    assert!(
        refusal.message().contains("bytes"),
        "the refusal says where in the text it stopped: {refusal}"
    );
}

#[test]
fn json_missing_a_required_field_is_refused_by_path() {
    let text = r#"{"fragment": {"param_shape": {"scalar": "int"}}, "plan": {"staging": []}}"#;
    let refusal = read_fused_kernel(text).expect_err("a fragment without a body names no fragment");
    assert!(
        matches!(&refusal, FusedRefusal::Malformed { path: Some(_), .. }),
        "the refusal carries the path the parser knew: {refusal}"
    );
    assert!(
        refusal.message().contains("fragment"),
        "the refusal names the field the reader was in: {refusal}"
    );
}

#[test]
fn an_unknown_enum_spelling_is_refused_by_name_and_path() {
    let text = r#"
    {
      "fragment": {
        "param_shape": { "scalar": "int" },
        "roles": { "scalars": [], "inputs": [], "outputs": [], "input_classes": [], "output_classes": [] },
        "body": { "values": [], "blocks": [], "entry": 0, "barriers": [] },
        "inputs": 0, "outputs": 0,
        "input_classes": [], "output_classes": [], "result_classes": [],
        "target": { "int_width": "i64", "lane_width": 64 },
        "slices": [{ "class": "int", "count": 64, "space": "global" }],
        "marks": []
      },
      "claim": null,
      "plan": { "staging": [] }
    }
    "#;
    let refusal = read_fused_kernel(text).expect_err("`global` is not a space this plan admits");
    let message = refusal.message();
    assert!(
        message.contains("global"),
        "the refusal quotes the spelling it met: {message}"
    );
    assert!(
        message.contains("fragment.slices[0].space"),
        "the refusal points at the field it met it in: {message}"
    );
}

#[test]
fn a_body_the_ir_refuses_is_refused_with_the_ir_message() {
    let text = r#"
    {
      "fragment": {
        "param_shape": { "scalar": "int" },
        "roles": { "scalars": [], "inputs": [], "outputs": [], "input_classes": [], "output_classes": [] },
        "body": {
          "values": [
            { "block_param": { "block": 0 } },
            { "instr": { "op": { "const": { "class": "int", "bits": 1 } }, "args": [], "classes": ["int"] } },
            { "instr": { "op": { "const": { "class": "int", "bits": 7 } }, "args": [], "classes": ["int"] } }
          ],
          "blocks": [
            {
              "params": [0],
              "instrs": [1],
              "terminator": { "cond_br": { "cond": 1, "if_true": { "target": 1, "args": [] }, "if_false": { "target": 2, "args": [] } } }
            },
            { "params": [], "instrs": [2], "terminator": { "return": { "values": [2] } } },
            { "params": [], "instrs": [], "terminator": { "return": { "values": [2] } } }
          ],
          "entry": 0,
          "barriers": []
        },
        "inputs": 0, "outputs": 0,
        "input_classes": [], "output_classes": [], "result_classes": ["int"],
        "target": { "int_width": "i64", "lane_width": 64 },
        "slices": [],
        "marks": []
      },
      "claim": null,
      "plan": { "staging": [] }
    }
    "#;
    let refusal = read_fused_kernel(text).expect_err("a use no path defines is refused");
    assert!(
        matches!(refusal, FusedRefusal::FragmentRefused { .. }),
        "the refusal comes from the IR's own gate: {refusal}"
    );
    assert!(
        refusal.message().contains("does not dominate"),
        "the IR's own message travels: {refusal}"
    );
}

#[test]
fn a_plan_naming_a_mark_the_body_never_published_is_refused() {
    let kernel = FusedKernel {
        plan: Plan {
            staging: vec![Staging {
                name: "absent".to_string(),
                space: Space::Shared,
                group_width: LANES,
            }],
        },
        ..staged_kernel()
    };
    let refusal = read_fused_kernel(&write_fused_kernel(&kernel))
        .expect_err("a plan naming no mark names no value");
    assert!(
        matches!(
            refusal,
            FusedRefusal::PlanRefused {
                refusal: PlanRefusal::UnknownStagedValue { ref name }
            } if name == "absent"
        ),
        "the IR's own refusal travels: {refusal}"
    );
}

#[test]
fn an_operand_past_the_value_table_is_refused_without_indexing_it() {
    let text = r#"
    {
      "fragment": {
        "param_shape": { "scalar": "int" },
        "roles": { "scalars": [], "inputs": [], "outputs": [], "input_classes": [], "output_classes": [] },
        "body": {
          "values": [
            { "instr": { "op": { "const": { "class": "int", "bits": 1 } }, "args": [], "classes": ["int"] } }
          ],
          "blocks": [
            { "params": [], "instrs": [0], "terminator": { "return": { "values": [7] } } }
          ],
          "entry": 0,
          "barriers": []
        },
        "inputs": 0, "outputs": 0,
        "input_classes": [], "output_classes": [], "result_classes": ["int"],
        "target": { "int_width": "i64", "lane_width": 64 },
        "slices": [],
        "marks": []
      },
      "claim": null,
      "plan": { "staging": [] }
    }
    "#;
    let refusal = read_fused_kernel(text).expect_err("value 7 is not in a one-entry table");
    let message = refusal.message();
    assert!(
        message.contains("fragment.body.blocks[0].terminator.return.values[0]"),
        "the refusal names the operand and where it sits: {message}"
    );
    assert!(
        message.contains("holds 1 value"),
        "the refusal names what it named: {message}"
    );
}

#[test]
fn a_field_the_schema_does_not_have_is_refused_by_path() {
    let text = r#"
    {
      "fragment": {
        "param_shape": { "scalar": "int" },
        "roles": { "scalars": [], "inputs": [], "outputs": [], "input_classes": [], "output_classes": [] },
        "body": { "values": [], "blocks": [], "entry": 0, "barriers": [] },
        "inputs": 0, "outputs": 0,
        "input_classes": [], "output_classes": [], "result_classes": [],
        "target": { "int_width": "i64", "lane_width": 64 },
        "slices": [{ "class": "int", "count": 64, "space": "shared", "element": "int" }],
        "marks": []
      },
      "claim": null,
      "plan": { "staging": [] }
    }
    "#;
    let refusal = read_fused_kernel(text).expect_err("`element` is not a field this schema has");
    let message = refusal.message();
    assert!(
        message.contains("element"),
        "the refusal quotes the field it met: {message}"
    );
    assert!(
        message.contains("fragment.slices[0]"),
        "the refusal points at where it met it: {message}"
    );
}

#[test]
fn every_instruction_the_ir_has_is_expressed_by_the_text() {
    let text = r#"
    {
      "fragment": {
        "param_shape": { "tuple": [
          { "scalar": "int" }, { "scalar": "int" }, { "scalar": "float" }
        ] },
        "roles": { "scalars": [], "inputs": [], "outputs": [], "input_classes": [], "output_classes": [] },
        "body": {
          "values": [
            { "block_param": { "block": 0 } },
            { "block_param": { "block": 0 } },
            { "block_param": { "block": 0 } },
            { "instr": { "op": { "const": { "class": "int", "bits": 3 } }, "args": [], "classes": ["int"] } },
            { "instr": { "op": { "const": { "class": "float", "bits": 0 } }, "args": [], "classes": ["float"] } },
            { "instr": { "op": { "bin": { "class": "int", "op": "add" } }, "args": [0, 3], "classes": ["int"] } },
            { "instr": { "op": "i32_wrap_i64", "args": [5], "classes": ["int"] } },
            { "instr": { "op": { "conv": { "from": "int", "to": "float" } }, "args": [6], "classes": ["float"] } },
            { "instr": { "op": { "buffer_read_call": { "class": "float" } }, "args": [3, 0], "classes": ["float"] } },
            { "instr": { "op": { "bin": { "class": "float", "op": "add" } }, "args": [8, 4], "classes": ["float"] } },
            { "instr": { "op": "select", "args": [7, 9, 0] , "classes": ["float"] } },
            { "instr": { "op": "exp", "args": [10], "classes": ["float"] } },
            { "instr": { "op": "sqrt", "args": [11], "classes": ["float"] } },
            { "instr": { "op": "floor", "args": [12], "classes": ["float"] } },
            { "instr": { "op": { "call_kernel": { "kernel": 3 } }, "args": [0], "classes": ["int"] } },
            { "instr": { "op": { "buffer_write_call": { "class": "int" } }, "args": [0, 1, 14], "classes": [] } },
            { "instr": { "op": { "slice_write_call": { "class": "int" } }, "args": [3, 1, 14], "classes": ["int"] } },
            { "instr": { "op": { "slice_read_call": { "class": "int" } }, "args": [3, 1, 16], "classes": ["int"] } }
          ],
          "blocks": [
            {
              "params": [0, 1, 2],
              "instrs": [3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17],
              "terminator": { "return": { "values": [17] } }
            }
          ],
          "entry": 0,
          "barriers": []
        },
        "inputs": 1, "outputs": 1,
        "input_classes": ["float"], "output_classes": ["int"], "result_classes": ["int"],
        "target": { "int_width": "i64", "lane_width": 64 },
        "slices": [{ "class": "int", "count": 64, "space": "register" }],
        "marks": []
      },
      "claim": null,
      "plan": { "staging": [] }
    }
    "#;
    let kernel = read_fused_kernel(text).expect("every instruction the IR has is expressible");
    let id = ValueId;
    assert_eq!(
        kernel.fragment.body.blocks[0].instrs,
        vec![
            id(3),
            id(4),
            id(5),
            id(6),
            id(7),
            id(8),
            id(9),
            id(10),
            id(11),
            id(12),
            id(13),
            id(14),
            id(15),
            id(16),
            id(17),
        ],
        "the text's instruction order is the emitter's order, not the definition order"
    );
    assert_eq!(
        kernel.fragment.body.instrs(),
        vec![
            &KernelInstr::Const(ScalarClass::Int, 3),
            &KernelInstr::Const(ScalarClass::Float, 0),
            &KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
            &KernelInstr::I32WrapI64,
            &KernelInstr::Conv {
                from: ScalarClass::Int,
                to: ScalarClass::Float,
            },
            &KernelInstr::BufferReadCall(ScalarClass::Float),
            &KernelInstr::Bin(ScalarClass::Float, KernelBin::Add),
            &KernelInstr::Select,
            &KernelInstr::Exp,
            &KernelInstr::Sqrt,
            &KernelInstr::Floor,
            &KernelInstr::CallKernel(3),
            &KernelInstr::BufferWriteCall(ScalarClass::Int),
            &KernelInstr::SliceWriteCall(ScalarClass::Int),
            &KernelInstr::SliceReadCall(ScalarClass::Int),
        ],
        "each spelling reads back as the instruction it names"
    );
}
