//! Extend one enum with the variants of one or more other enums.
//!
//! The [`enum_ext!`] macro is the single shaping site: the base enum's own
//! definition is followed by a chain of `+ enum ...` extension definitions
//! written as literal tokens — the only form Rust accepts in a variant list.
//! For every extension the macro generates:
//!
//! - the base enum with the extension's variants spliced in;
//! - the extension enum as its own type;
//! - `From<Ext> for Base`, so an extension value constructs a base value;
//! - `impl AsEnum<Ext> for Base`, so a base value can be viewed as the
//!   extension value it was built from.
//!
//! # Example
//!
//! ```
//! use lichen_utils::extend::AsEnum;
//! use lichen_utils::enum_ext;
//!
//! enum_ext!(
//!     #[derive(Debug, Clone, PartialEq)]
//!     pub enum Base {
//!         Own,
//!     }
//!     + #[derive(Debug, Clone, PartialEq)]
//!     pub enum Extra {
//!         Added(usize),
//!     }
//! );
//!
//! let b = Base::from(Extra::Added(3));
//! assert_eq!(b, Base::Added(3));
//! assert_eq!(AsEnum::<Extra>::as_enum(&b), Some(Extra::Added(3)));
//! assert_eq!(AsEnum::<Extra>::as_enum(&Base::Own), None);
//! ```
//!
//! # Limitations
//!
//! - Same crate only: the base enum is fixed at the invocation site, so an
//!   existing enum in another crate cannot be extended here. For a base and
//!   extension in different crates, use the companion proc-macro crate
//!   `lichen-extend` instead (`#[lichen_extend::enum_ext]` on the extension
//!   enum, then its generated carrier at the base enum's call site).
//! - Extension variants must be unit, tuple, or carry a discriminant. A
//!   struct variant in an extension is a compile error, because the `From`
//!   and `AsEnum` impls cannot be generated for it. The base enum's own
//!   variants may have any shape.
//! - Extension payloads must be [`Clone`]: `as_enum` clones the payload out
//!   of the borrowed base value.
//! - Extension variant names must not collide with the base's own variants
//!   or with each other; Rust reports the duplicate.
//! - With several extensions, a plain `a.as_enum()` call is ambiguous; use
//!   `AsEnum::<B>::as_enum(&a)`.
//! - No generics on the enums, and no attributes on individual variants.

/// View a value as the extension enum it was built from.
///
/// [`enum_ext!`] implements this trait for the base enum against each
/// extension enum in the chain. `Some(ext)` means `self` is one of `B`'s
/// variants and `ext` is the corresponding value of `B`.
pub trait AsEnum<B> {
    fn as_enum(&self) -> Option<B>;
}

/// Shape one enum from a base definition plus a chain of extension enums.
///
/// See the [module documentation](self) for the generated items and the
/// limitations.
#[macro_export]
macro_rules! enum_ext {
    // ── entry ──
    (
        $(#[$base_attr:meta])*
        $base_vis:vis enum $base:ident {
            $($bv:ident $(($($bpt:tt)*))? $({$($bft:tt)*})? $(= $bdisc:expr)?),* $(,)?
        }
        $(
            + $(#[$ext_attr:meta])* $ext_vis:vis enum $ext:ident {
                $($ev:ident $(($($ept:tt)*))? $({$($eft:tt)*})? $(= $edisc:expr)?),* $(,)?
            }
        )*
    ) => {
        $(#[$base_attr])*
        $base_vis enum $base {
            $($bv $(($($bpt)*))? $({$($bft)*})? $(= $bdisc)?,)*
            $($($ev $(($($ept)*))? $({$($eft)*})? $(= $edisc)?,)*)*
        }
        $(
            $(#[$ext_attr])*
            $ext_vis enum $ext {
                $($ev $(($($ept)*))? $({$($eft)*})? $(= $edisc)?,)*
            }
            impl From<$ext> for $base {
                fn from(__value: $ext) -> $base {
                    match __value {
                        $(
                            $crate::enum_ext!(@pat_from $ext $ev __v $(($($ept)*))? $({$($eft)*})?)
                            => $crate::enum_ext!(@body_from $base $ev __v $(($($ept)*))? $({$($eft)*})?),
                        )*
                    }
                }
            }
            impl $crate::extend::AsEnum<$ext> for $base {
                fn as_enum(&self) -> Option<$ext> {
                    match self {
                        $(
                            $crate::enum_ext!(@pat_as $base $ev __v $(($($ept)*))? $({$($eft)*})?)
                            => $crate::enum_ext!(@body_as $ext $ev __v $(($($ept)*))? $({$($eft)*})?),
                        )*
                        _ => None,
                    }
                }
            }
        )*
    };
    // ── pattern/body pieces for the `From` impl (payload moves) ──
    (@pat_from $ext:ident $tag:ident $v:ident ( $($pt:tt)* ) ) => {
        $ext::$tag($v)
    };
    (@pat_from $ext:ident $tag:ident $v:ident) => {
        $ext::$tag
    };
    (@body_from $base:ident $tag:ident $v:ident ( $($pt:tt)* ) ) => {
        $base::$tag($v)
    };
    (@body_from $base:ident $tag:ident $v:ident) => {
        $base::$tag
    };
    // ── pattern/body pieces for the `AsEnum` impl (payload clones) ──
    (@pat_as $base:ident $tag:ident $v:ident ( $($pt:tt)* ) ) => {
        $base::$tag($v)
    };
    (@pat_as $base:ident $tag:ident $v:ident) => {
        $base::$tag
    };
    (@body_as $ext:ident $tag:ident $v:ident ( $($pt:tt)* ) ) => {
        Some($ext::$tag(Clone::clone($v)))
    };
    (@body_as $ext:ident $tag:ident $v:ident) => {
        Some($ext::$tag)
    };
    // ── struct variants in an extension: the impls cannot be generated ──
    (@pat_from $ext:ident $tag:ident $v:ident { $($ft:tt)* } ) => {
        compile_error!("enum_ext: struct variants are not supported in extension enums");
    };
    (@body_from $base:ident $tag:ident $v:ident { $($ft:tt)* } ) => {
        compile_error!("enum_ext: struct variants are not supported in extension enums");
    };
    (@pat_as $base:ident $tag:ident $v:ident { $($ft:tt)* } ) => {
        compile_error!("enum_ext: struct variants are not supported in extension enums");
    };
    (@body_as $ext:ident $tag:ident $v:ident { $($ft:tt)* } ) => {
        compile_error!("enum_ext: struct variants are not supported in extension enums");
    };
}
