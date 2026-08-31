use lichen_utils::enum_ext;
use lichen_utils::extend::AsEnum;

// The extension enums are ordinary enums — plain definitions, located
// wherever they belong (same crate here; another crate for the carriers).
#[derive(Debug, Clone, PartialEq)]
pub enum Extra {
    Alpha,
    Wrap(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Only {
    Y,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Text {
    S(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Tagged {
    A = 1,
    B = 2,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Small {
    S,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Nil {}

enum_ext!(
    #[derive(Debug, Clone, PartialEq)]
    pub enum Base {
        Own,
        Payload(u32),
    }
    + Extra;
);

enum_ext!(
    #[derive(Debug, Clone, PartialEq)]
    pub enum FromNothing {
    }
    + Only;
);

enum_ext!(
    #[derive(Debug, Clone, PartialEq)]
    pub enum WithString {
        Empty,
    }
    + Text;
);

enum_ext!(
    #[derive(Debug, Clone, PartialEq)]
    pub enum WithDisc {
        Own,
    }
    + Tagged;
);

enum_ext!(
    #[derive(Debug, Clone, PartialEq)]
    pub enum MixedBase {
        Plain,
        Rec { x: u32, y: u32 },
    }
    + Small;
);

// The path form: the carry variant is named explicitly, so the extension can
// be referenced from another module (or another crate, as the carriers do).
mod hidden {
    #[derive(Debug, Clone, PartialEq)]
    pub enum Secret {
        S,
    }
}

enum_ext!(
    #[derive(Debug, Clone, PartialEq)]
    pub enum ViaPath {
        Own,
    }
    + hidden::Secret as Hidden;
);

// --- chains and merges are the same operation ----------------------------

// Every layer provides a plain enum of its own variants.
#[derive(Debug, Clone, PartialEq)]
pub enum Root {
    N(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Mid {
    Marker,
}

// The top of the chain names every layer directly — all siblings, no
// nesting, and the views/From impls for both layers are generated.
enum_ext!(
    #[derive(Debug, Clone, PartialEq)]
    pub enum Top {
        Extra,
    }
    + Root as Root;
    + Mid as Mid;
);

// Two vocabularies over the same ancestors merge by listing both
// extensions — the same operation as a chain.
#[derive(Debug, Clone, PartialEq)]
pub enum BranchA {
    A,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BranchB {
    B,
}

enum_ext!(
    #[derive(Debug, Clone, PartialEq)]
    pub enum Merged {
    }
    + BranchA as A;
    + BranchB as B;
);

// --- tests ---------------------------------------------------------------

#[test]
fn base_gains_carry_variant() {
    let values = [
        format!("{:?}", Base::Own),
        format!("{:?}", Base::Payload(3)),
        format!("{:?}", Base::Extra(Extra::Alpha)),
        format!("{:?}", Base::Extra(Extra::Wrap(1))),
    ];
    assert_eq!(
        values,
        ["Own", "Payload(3)", "Extra(Alpha)", "Extra(Wrap(1))",]
    );
}

#[test]
fn from_extension_wraps() {
    assert_eq!(Base::from(Extra::Alpha), Base::Extra(Extra::Alpha));
    assert_eq!(Base::from(Extra::Wrap(5)), Base::Extra(Extra::Wrap(5)));
}

#[test]
fn as_enum_views_carry_variant() {
    assert_eq!(
        AsEnum::<Extra>::as_enum(&Base::Extra(Extra::Alpha)),
        Some(Extra::Alpha)
    );
    assert_eq!(
        AsEnum::<Extra>::as_enum(&Base::Extra(Extra::Wrap(5))),
        Some(Extra::Wrap(5))
    );
    // The base's own variants are not part of the extension.
    assert_eq!(AsEnum::<Extra>::as_enum(&Base::Own), None);
    assert_eq!(AsEnum::<Extra>::as_enum(&Base::Payload(1)), None);
}

#[test]
fn path_form_names_carry_variant() {
    assert_eq!(
        ViaPath::from(hidden::Secret::S),
        ViaPath::Hidden(hidden::Secret::S)
    );
    assert_eq!(
        AsEnum::<hidden::Secret>::as_enum(&ViaPath::Hidden(hidden::Secret::S)),
        Some(hidden::Secret::S)
    );
    assert_eq!(AsEnum::<hidden::Secret>::as_enum(&ViaPath::Own), None);
}

#[test]
fn empty_base_gets_only_carry_variant() {
    match FromNothing::Only(Only::Y) {
        FromNothing::Only(Only::Y) => {}
    }
    assert_eq!(
        AsEnum::<Only>::as_enum(&FromNothing::Only(Only::Y)),
        Some(Only::Y)
    );
}

#[test]
fn empty_extension_compiles() {
    // An extension with no variants: the carry variant exists but is
    // uninhabited, and the `From`/`AsEnum` impls still compile.
    enum_ext!(
        #[derive(Debug, Clone, PartialEq)]
        pub enum WithNil {
            Own,
        }
        + Nil;
    );
    assert_eq!(std::mem::size_of::<Nil>(), 0);
    assert_eq!(AsEnum::<Nil>::as_enum(&WithNil::Own), None);
}

#[test]
fn as_enum_clones_heap_payloads() {
    let a = WithString::from(Text::S("hi".into()));
    assert_eq!(a, WithString::Text(Text::S("hi".into())));
    assert_eq!(AsEnum::<Text>::as_enum(&a), Some(Text::S("hi".into())));
    assert_eq!(AsEnum::<Text>::as_enum(&WithString::Empty), None);
}

#[test]
fn ext_discriminants_stay_in_ext() {
    // Discriminants live on the extension enum itself; the composed enum
    // never re-numbers them because it never splices variants.
    assert_eq!(Tagged::A as usize, 1);
    assert_eq!(Tagged::B as usize, 2);
    assert_eq!(WithDisc::from(Tagged::B), WithDisc::Tagged(Tagged::B));
    assert_eq!(
        AsEnum::<Tagged>::as_enum(&WithDisc::Tagged(Tagged::B)),
        Some(Tagged::B)
    );
    assert_eq!(AsEnum::<Tagged>::as_enum(&WithDisc::Own), None);
}

#[test]
fn base_variants_keep_arbitrary_shape() {
    // The base's own struct variants pass through untouched.
    let rec = MixedBase::Rec { x: 1, y: 2 };
    match rec {
        MixedBase::Rec { x, y } => {
            assert_eq!((x, y), (1, 2));
        }
        _ => unreachable!(),
    }
    assert_eq!(MixedBase::from(Small::S), MixedBase::Small(Small::S));
    assert_eq!(AsEnum::<Small>::as_enum(&MixedBase::Plain), None);
}

#[test]
fn chain_composition_lists_every_layer() {
    let top = Top::from(Root::N(3));
    assert_eq!(top, Top::Root(Root::N(3)));
    assert_eq!(AsEnum::<Root>::as_enum(&top), Some(Root::N(3)));
    assert_eq!(AsEnum::<Root>::as_enum(&Top::Extra), None);
    assert_eq!(AsEnum::<Mid>::as_enum(&top), None);
    assert_eq!(
        AsEnum::<Mid>::as_enum(&Top::Mid(Mid::Marker)),
        Some(Mid::Marker)
    );
    // Each layer's From wraps into its own branch.
    assert_eq!(Top::from(Mid::Marker), Top::Mid(Mid::Marker));
}

#[test]
fn merge_is_the_same_operation() {
    let a = Merged::from(BranchA::A);
    let b = Merged::from(BranchB::B);
    assert_eq!(a, Merged::A(BranchA::A));
    assert_eq!(AsEnum::<BranchA>::as_enum(&a), Some(BranchA::A));
    assert_eq!(AsEnum::<BranchB>::as_enum(&a), None);
    assert_eq!(AsEnum::<BranchB>::as_enum(&b), Some(BranchB::B));
}
