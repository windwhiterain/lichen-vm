use lichen_utils::enum_ext;
use lichen_utils::extend::AsEnum;

enum_ext!(
    #[derive(Debug, Clone, PartialEq)]
    pub enum Base {
        Own,
        Payload(u32),
    }
    + #[derive(Debug, Clone, PartialEq)]
    pub enum Extra {
        Alpha,
        Wrap(usize),
    }
    + #[derive(Debug, Clone, PartialEq)]
    pub enum Other {
        X1,
    }
);

enum_ext!(
    #[derive(Debug, Clone, PartialEq)]
    pub enum Compact { A, B }
    + #[derive(Debug, Clone, PartialEq)]
    pub enum Sub { S1, S2 }
);

enum_ext!(
    #[derive(Debug, Clone, PartialEq)]
    pub enum EmptyExt {
    }
    + pub enum Nil {}
);

enum_ext!(
    #[derive(Debug, Clone, PartialEq)]
    pub enum FromNothing {
    }
    + #[derive(Debug, Clone, PartialEq)]
    pub enum Only { Y }
);

enum_ext!(
    #[derive(Debug, Clone, PartialEq)]
    pub enum WithString {
        Empty,
    }
    + #[derive(Debug, Clone, PartialEq)]
    pub enum Text { S(String) }
);

enum_ext!(
    #[derive(Debug, Clone, PartialEq)]
    pub enum WithDisc {
        Own,
    }
    + #[derive(Debug, Clone, PartialEq)]
    pub enum Tagged { A = 1, B = 2 }
);

enum_ext!(
    #[derive(Debug, Clone, PartialEq)]
    pub enum MixedBase {
        Plain,
        Rec { x: u32, y: u32 },
    }
    + #[derive(Debug, Clone, PartialEq)]
    pub enum Small { S }
);

// --- tests ------------------------------------------------------------

#[test]
fn base_enum_gains_extension_variants() {
    let values = [
        format!("{:?}", Base::Own),
        format!("{:?}", Base::Payload(3)),
        format!("{:?}", Base::Alpha),
        format!("{:?}", Base::Wrap(1)),
        format!("{:?}", Base::X1),
    ];
    assert_eq!(values, ["Own", "Payload(3)", "Alpha", "Wrap(1)", "X1"]);
}

#[test]
fn all_variants_are_matchable() {
    let describe = |a: &Base| -> String {
        match a {
            Base::Own => "own".into(),
            Base::Payload(n) => format!("payload:{n}"),
            Base::Alpha => "alpha".into(),
            Base::Wrap(n) => format!("wrap:{n}"),
            Base::X1 => "x1".into(),
        }
    };
    assert_eq!(describe(&Base::Own), "own");
    assert_eq!(describe(&Base::Payload(2)), "payload:2");
    assert_eq!(describe(&Base::Wrap(7)), "wrap:7");
    assert_eq!(describe(&Base::X1), "x1");
}

#[test]
fn from_extension_builds_base_value() {
    assert_eq!(Base::from(Extra::Alpha), Base::Alpha);
    assert_eq!(Base::from(Extra::Wrap(5)), Base::Wrap(5));
}

#[test]
fn as_enum_views_base_value_as_its_extension() {
    assert_eq!(AsEnum::<Extra>::as_enum(&Base::Alpha), Some(Extra::Alpha));
    assert_eq!(
        AsEnum::<Extra>::as_enum(&Base::Wrap(5)),
        Some(Extra::Wrap(5))
    );
    // The base's own variants are not part of any extension.
    assert_eq!(AsEnum::<Extra>::as_enum(&Base::Own), None);
    assert_eq!(AsEnum::<Extra>::as_enum(&Base::X1), None);
    // A second extension gets its own view.
    assert_eq!(AsEnum::<Other>::as_enum(&Base::X1), Some(Other::X1));
    assert_eq!(AsEnum::<Other>::as_enum(&Base::Alpha), None);
}

#[test]
fn base_without_trailing_comma() {
    assert_eq!(Compact::from(Sub::S2), Compact::S2);
    assert_eq!(AsEnum::<Sub>::as_enum(&Compact::S1), Some(Sub::S1));
    assert_eq!(AsEnum::<Sub>::as_enum(&Compact::A), None);
}

#[test]
fn empty_base_gets_only_extension_variants() {
    assert_eq!(FromNothing::Y, FromNothing::Y);
    match FromNothing::Y {
        FromNothing::Y => {}
    }
    assert_eq!(AsEnum::<Only>::as_enum(&FromNothing::Y), Some(Only::Y));
}

#[test]
fn empty_extension_contributes_nothing() {
    // EmptyExt and Nil are both uninhabited; the invocation must still
    // compile, including the `From`/`AsEnum` impls for the empty extension.
    assert_eq!(std::mem::size_of::<EmptyExt>(), 0);
    assert_eq!(std::mem::size_of::<Nil>(), 0);
}

#[test]
fn as_enum_clones_heap_payloads() {
    let a = WithString::from(Text::S("hi".into()));
    assert_eq!(a, WithString::S("hi".into()));
    assert_eq!(AsEnum::<Text>::as_enum(&a), Some(Text::S("hi".into())));
    assert_eq!(AsEnum::<Text>::as_enum(&WithString::Empty), None);
}

#[test]
fn extension_discriminants_are_preserved() {
    assert_eq!(Tagged::A as usize, 1);
    assert_eq!(Tagged::B as usize, 2);
    assert_eq!(WithDisc::from(Tagged::B), WithDisc::B);
    assert_eq!(AsEnum::<Tagged>::as_enum(&WithDisc::B), Some(Tagged::B));
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
    assert_eq!(MixedBase::from(Small::S), MixedBase::S);
    assert_eq!(AsEnum::<Small>::as_enum(&MixedBase::Plain), None);
}
