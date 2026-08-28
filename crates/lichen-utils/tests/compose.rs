use lichen_utils::compose::AsField;
use lichen_utils::compose_ext;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Counter {
    n: usize,
}
impl Counter {
    fn bump(&mut self) -> usize {
        let n = self.n;
        self.n += 1;
        n
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Tag {
    label: usize,
}

compose_ext! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct Host(
        Counter,
        Tag,
    );
}

#[test]
fn compose_ext_generates_a_tuple_struct_of_components_with_field_accessors() {
    let mut h = Host::default();
    // Each component's inherent method is reached via AsField get_mut — no
    // per-component accessor trait is wired by the macro, and the tuple
    // positions mean no field-name collision.
    assert_eq!(AsField::<Counter>::get_mut(&mut h).bump(), 0);
    assert_eq!(AsField::<Counter>::get_mut(&mut h).bump(), 1);
    assert_eq!(AsField::<Counter>::get(&h).n, 2);

    // A different component is read independently.
    assert_eq!(AsField::<Tag>::get(&h).label, 0);

    assert_eq!(h, Host(Counter { n: 2 }, Tag { label: 0 }));
}
