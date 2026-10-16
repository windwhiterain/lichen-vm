//! The two refusals a workgroup taller than one row meets, at the device gate.
//!
//! # Invariant
//! These need no device: they are properties of the gate a fragment is checked against, and a
//! refusal that only appeared once a GPU was present would be one nobody could test on a machine
//! without one. Every other fact the gate reads is answered by a fragment that is empty on it, so
//! each refusal below is about the `y` axis and about nothing the fragment also happens to have.

use lichen_kernel_ir::{
    FlatOp, IntWidth, KernelBody, KernelFragment, KernelInstr, KernelRoles, KernelShape,
    LaneHeight, LaneWidth, Plan, ScalarClass, TargetFacts,
};
use lichen_runtime::{DeviceFacts, DeviceRefusal};

/// The rows one workgroup may run on [`device`]: the `y` half of its ceiling.
const CEILING: u32 = 64;

/// A device whose second axis is narrower than its first.
///
/// # Invariant
/// **Hand-made, and said so**: a table of numbers is exactly what a [`DeviceFacts`] is, so the
/// ceiling a height is refused against is stated rather than waited for. The two axes differ, so
/// a refusal naming `64` is naming the second one and not the first.
fn device() -> DeviceFacts {
    DeviceFacts {
        int_bits: 64,
        max_workgroup_lanes: 1024,
        max_workgroup_lanes_y: CEILING,
        subgroup_width: Some(32),
        max_shared_bytes: 48 * 1024,
        max_storage_buffers: 4,
    }
}

/// A fragment declaring `rows` rows of 64 lanes, and nothing else the gate reads.
///
/// # Invariant
/// Empty on every other axis — no buffers, no slices, a plan that stages nothing — so a refusal
/// here is about `lane_height` alone rather than about the first number to disagree.
fn fragment(rows: u32) -> KernelFragment {
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(Vec::new()),
        body: KernelBody::from_flat(0, &[FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 1))]),
        inputs: 0,
        outputs: 0,
        input_classes: Vec::new(),
        output_classes: Vec::new(),
        result_classes: vec![ScalarClass::Int; 1],
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(64),
            lane_height: LaneHeight::new(rows),
        },
        slices: Vec::new(),
        marks: Vec::new(),
    }
}

/// What [`device`] makes of a workgroup `rows` rows tall.
///
/// # Invariant
/// The gate is called rather than the loader, so the refusal is the gate's own and no dispatch,
/// queue or graph stands between the declaration and the answer.
fn refused(rows: u32) -> DeviceRefusal {
    device()
        .check(&fragment(rows), &Plan::default())
        .expect_err("only one row is scheduled, so a second row has no answer here")
}

/// One row is the shape every backend here runs, so one row is accepted.
#[test]
fn a_single_row_workgroup_is_accepted() {
    device()
        .check(&fragment(1), &Plan::default())
        .expect("one row is the only height this build schedules");
}

/// A rectangle within the device's ceiling is refused by this build's own one-row rule.
///
/// # Invariant
/// Both heights are under [`CEILING`], so the device agrees with both and the refusal is the
/// build's: it names **one row**, a fact no device reports, rather than the 64 rows this one
/// offers — a rectangle the ceiling cannot be the thing that catches
/// (`docs/notes/two-dimensional-workgroup.md` §9.3).
#[test]
fn a_rectangle_within_the_device_ceiling_is_refused_as_taller_than_one_row() {
    for rows in [4u32, 16] {
        let refusal = refused(rows);
        assert_eq!(
            refusal.name(),
            "WorkgroupTallerThanOneRow",
            "a rectangle is named"
        );
        assert_eq!(
            refusal,
            DeviceRefusal::WorkgroupTallerThanOneRow {
                asked: rows,
                one_row: 1
            },
            "and it carries the rows asked for against the one row this build schedules"
        );
    }
}

/// A height past the device's ceiling is refused by the device, ahead of the one-row rule.
///
/// # Invariant
/// `CEILING + 1` is both past this device **and** more than one row, so naming the device rather
/// than the build is what says which gate read the height first
/// (`docs/notes/two-dimensional-workgroup.md` §9.3).
#[test]
fn a_height_past_the_device_ceiling_is_refused_as_too_wide() {
    let refusal = refused(CEILING + 1);
    assert_eq!(
        refusal.name(),
        "LaneHeightTooWide",
        "an over-tall height is named"
    );
    assert_eq!(
        refusal,
        DeviceRefusal::LaneHeightTooWide {
            asked: CEILING + 1,
            tallest: CEILING
        },
        "and it carries the rows asked for against the rows this device schedules"
    );
}
