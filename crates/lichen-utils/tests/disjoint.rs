use lichen_vm::utils::disjoint::{self, Node, Meta};
use slotmap::{SlotMap, new_key_type};

new_key_type! {pub struct TestKey;}

#[derive(Clone, Copy, Default)]
struct TestNode {
    set: Meta<TestKey>,
}

impl Node for TestNode {
    type Key = TestKey;
    fn meta(&self) -> &Meta<TestKey> {
        &self.set
    }
    fn meta_mut(&mut self) -> &mut Meta<TestKey> {
        &mut self.set
    }
}

// --- helpers ---------------------------------------------------------

fn add(nodes: &mut SlotMap<TestKey, TestNode>) -> TestKey {
    let id = nodes.insert(TestNode::default());
    disjoint::make_set(nodes, id);
    id
}

/// Link `child` directly under `parent`, bypassing [`disjoint::union`], to
/// build a deliberately deep path for compression tests.
fn link(nodes: &mut SlotMap<TestKey, TestNode>, child: TestKey, parent: TestKey) {
    nodes[child].meta_mut().parent = Some(parent);
}

// --- tests ------------------------------------------------------------

#[test]
fn make_set_initializes_singleton() {
    let mut nodes = SlotMap::with_key();
    let key = add(&mut nodes);
    assert_eq!(nodes[key].meta().parent, None);
    assert_eq!(nodes[key].meta().next, None);
    assert_eq!(nodes[key].meta().tail, Some(key));
    assert_eq!(nodes[key].meta().size, 1);
    assert_eq!(disjoint::find(&mut nodes, key), key);
}

#[test]
fn find_returns_representative_and_compresses_path() {
    let mut nodes = SlotMap::with_key();
    let ids: Vec<_> = (0..5).map(|_| add(&mut nodes)).collect();
    for pair in ids.windows(2) {
        link(&mut nodes, pair[1], pair[0]); // 0 <- 1 <- 2 <- 3 <- 4
    }

    let root = disjoint::find(&mut nodes, *ids.last().unwrap());

    assert_eq!(root, ids[0]);
    for &id in &ids {
        assert_eq!(nodes[id].meta().parent, (id != ids[0]).then_some(ids[0]));
    }
}

#[test]
fn find_is_stack_safe_on_deep_chains() {
    let mut nodes = SlotMap::with_key();
    let mut chain = Vec::with_capacity(100_000);
    for _ in 0..100_000 {
        chain.push(add(&mut nodes));
    }
    for pair in chain.windows(2) {
        link(&mut nodes, pair[1], pair[0]);
    }

    let root = disjoint::find(&mut nodes, *chain.last().unwrap());

    assert_eq!(root, chain[0]);
    for &id in &chain {
        assert_eq!(nodes[id].meta().parent, (id != chain[0]).then_some(chain[0]));
    }
}

#[test]
fn union_merges_lists_in_append_order() {
    let mut nodes = SlotMap::with_key();
    let a = add(&mut nodes);
    let b = add(&mut nodes);
    let c = add(&mut nodes);
    disjoint::union(&mut nodes, a, b);
    disjoint::union(&mut nodes, b, c);

    let rep = disjoint::find(&mut nodes, c);
    let list: Vec<_> = disjoint::members(&nodes, rep).collect();
    assert_eq!(list, vec![a, b, c]);
}

#[test]
fn union_attaches_smaller_under_larger() {
    let mut nodes = SlotMap::with_key();
    let big = [add(&mut nodes), add(&mut nodes), add(&mut nodes)];
    let small = [add(&mut nodes), add(&mut nodes)];
    disjoint::union(&mut nodes, big[0], big[1]);
    disjoint::union(&mut nodes, big[1], big[2]);
    disjoint::union(&mut nodes, small[0], small[1]);

    let rep = disjoint::union(&mut nodes, small[0], big[0]);

    assert_eq!(rep, big[0]); // the larger set's root stays representative
    assert_eq!(nodes[rep].meta().size, 5);
    let list: Vec<_> = disjoint::members(&nodes, rep).collect();
    assert_eq!(list, vec![big[0], big[1], big[2], small[0], small[1]]);
}

#[test]
fn union_of_joined_sets_is_noop() {
    let mut nodes = SlotMap::with_key();
    let a = add(&mut nodes);
    let b = add(&mut nodes);
    let rep = disjoint::union(&mut nodes, a, b);
    let before: Vec<_> = disjoint::members(&nodes, rep).collect();

    let again = disjoint::union(&mut nodes, a, b);

    assert_eq!(again, rep);
    let after: Vec<_> = disjoint::members(&nodes, rep).collect();
    assert_eq!(after, before); // no member is linked in twice
    assert_eq!(nodes[rep].meta().size, 2);
}

#[test]
fn members_visits_each_node_exactly_once() {
    let mut nodes = SlotMap::with_key();
    let n = 20;
    let ids: Vec<_> = (0..n).map(|_| add(&mut nodes)).collect();
    let mut reps = ids.clone();
    while reps.len() > 1 {
        let mut next = Vec::new();
        for pair in reps.chunks(2) {
            let rep = if pair.len() == 2 {
                disjoint::union(&mut nodes, pair[0], pair[1])
            } else {
                pair[0]
            };
            next.push(rep);
        }
        reps = next;
    }

    let list: Vec<_> = disjoint::members(&nodes, reps[0]).collect();
    assert_eq!(list.len(), n);
    let mut sorted = list.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), n);
}

#[test]
fn compression_does_not_disturb_member_lists() {
    let mut nodes = SlotMap::with_key();
    let a = add(&mut nodes);
    let a2 = add(&mut nodes);
    let a3 = add(&mut nodes);
    disjoint::union(&mut nodes, a, a2);
    disjoint::union(&mut nodes, a, a3);
    // A deliberately deep path a3 <- x <- y <- z built by hand, outside the
    // member list of a.
    let x = add(&mut nodes);
    let y = add(&mut nodes);
    let z = add(&mut nodes);
    link(&mut nodes, x, a3);
    link(&mut nodes, y, x);
    link(&mut nodes, z, y);
    let before: Vec<_> = disjoint::members(&nodes, a).collect();

    let root = disjoint::find(&mut nodes, z);

    assert_eq!(root, a);
    for &id in &[z, y, x, a3] {
        assert_eq!(nodes[id].meta().parent, Some(a)); // path flattened
    }
    let after: Vec<_> = disjoint::members(&nodes, a).collect();
    assert_eq!(after, before); // find rewrites only parent pointers
}
