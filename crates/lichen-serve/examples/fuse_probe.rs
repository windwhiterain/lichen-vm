//! The pre-attention region under the floor's own derivation: the class pack beside a fuse pack.
//!
//! # Invariant
//! **The fuse pack names a region the floor does not derive**: the sibling derivation splits the
//! shared producer out of the component, so the pack steers nothing, and the merge is the floor's
//! own — one kernel reading the shared producer's output once.

use lichen_compute::compute::clear_graph_pass;
use lichen_compute::placement_pack::read_placement_pack;
use lichen_serve::{PRE_STEP, Session};

/// The class pack the composed step is prepared with: bf16 for the three weight slots.
const CLASS_PACK: &str = r#"{"name":"narrow","requires":{"class":null,"nodes":[2,6,7],"classes":[{"at":1,"class":"bfloat16"}]},"plan":{"staging":[]}}"#;

/// The fuse pack that names the pre-attention region and asks for one kernel.
const FUSE_PACK: &str = r#"{"schema":null,"name":"fuse-pre-step","requires":{"class":"float"},"fuse":{"region":"sha256:7c4649dbe5642915bc1cc6c2a52f225dc255e6f461fcce85248429d7aaafbfc3","as":"one_kernel"},"plan":{"staging":[]}}"#;

fn main() {
    let session = Session::open().expect("a device");
    let class_pack = read_placement_pack(CLASS_PACK).expect("the class pack");
    let fuse_pack = read_placement_pack(FUSE_PACK).expect("the fuse pack");

    clear_graph_pass();
    let plain = session
        .prepare_with_packs(PRE_STEP, &[class_pack.clone()])
        .expect("the class pack alone prepares");
    println!("=== the class pack alone: {} node(s)", plain.nodes());
    for line in plain.report() {
        println!("{line}");
    }

    clear_graph_pass();
    let fused = session
        .prepare_with_packs(PRE_STEP, &[class_pack, fuse_pack])
        .expect("the fuse pack prepares");
    println!(
        "=== the class pack and the fuse pack: {} node(s)",
        fused.nodes()
    );
    for line in fused.report() {
        println!("{line}");
    }
}
