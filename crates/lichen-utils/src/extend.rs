//! Compose one enum out of a base definition and a set of extension enums —
//! the vocabulary-extension primitive behind the
//! `LowValue` + `TypeValue` → `HighProgramValue` composition.
//!
//! Every layer provides its own enum: the lowlevel's structural values, the
//! highlevel's type values, a language crate's own variants — each a plain
//! enum of just that layer's variants.  [`enum_ext!`] is the ONE macro, and
//! the composition site lists every extension it wants:
//!
//! - the base enum keeps its own variants (if any) and gains one *carry
//!   variant* per extension, holding that extension enum whole
//!   (`Base::Ext(Ext)`) — variants are never spliced, so every extension
//!   enum stays the single source of truth for its own variants, and a value
//!   has exactly one representation (the extension enums are disjoint);
//! - `From<Ext> for Base` and `impl AsEnum<Ext> for Base` per extension —
//!   the view is `None` on everything that is not that extension's branch.
//!
//! ```
//! use lichen_utils::enum_ext;
//! use lichen_utils::extend::AsEnum;
//!
//! // Each layer provides a plain enum of its own variants.
//! #[derive(Debug, Clone, PartialEq)]
//! pub enum Extra {
//!     Added(usize),
//! }
//!
//! enum_ext!(
//!     #[derive(Debug, Clone, PartialEq)]
//!     pub enum Base {
//!         Own,
//!     }
//!     + Extra as Extra;
//! );
//!
//! let base = Base::from(Extra::Added(3));
//! assert_eq!(base, Base::Extra(Extra::Added(3)));
//! assert_eq!(AsEnum::<Extra>::as_enum(&base), Some(Extra::Added(3)));
//! assert_eq!(AsEnum::<Extra>::as_enum(&Base::Own), None);
//! ```
//!
//! # Chains and merges are the same operation
//!
//! A downstream vocabulary lists every ancestor layer's enum plus its own —
//! one invocation, all extensions as siblings.  Two vocabularies over the
//! same ancestors merge by listing both extensions.  There is no nesting and
//! no delegation: every extension enum is flat, so no value is ever
//! representable under two branches, and `PartialEq` stays exact.
//!
//! ```
//! use lichen_utils::enum_ext;
//! use lichen_utils::extend::AsEnum;
//!
//! #[derive(Debug, Clone, PartialEq)]
//! pub enum Low {
//!     N(usize),
//! }
//!
//! #[derive(Debug, Clone, PartialEq)]
//! pub enum High {
//!     Marker,
//! }
//!
//! // The top of the chain names every layer directly.
//! enum_ext!(
//!     #[derive(Debug, Clone, PartialEq)]
//!     pub enum Probe {
//!         Added,
//!     }
//!     + Low as Low;
//!     + High as High;
//! );
//!
//! let probe = Probe::from(Low::N(3));
//! assert_eq!(probe, Probe::Low(Low::N(3)));
//! assert_eq!(AsEnum::<Low>::as_enum(&probe), Some(Low::N(3)));
//! assert_eq!(AsEnum::<High>::as_enum(&probe), None);
//! assert_eq!(AsEnum::<Low>::as_enum(&Probe::High(High::Marker)), None);
//! ```
//!
//! # Notes
//!
//! - The base's own variants must each end with a comma.
//! - Every extension is `path as Variant` — the carry variant's name; a bare
//!   `+ Ext;` is shorthand for a single `+ Ext as Ext;`.
//! - Extensions should be *plain* per-layer enums.  Composing an already
//!   composed enum is legal but nests it (a carry variant holding a union),
//!   which defeats the one-representation property — compose the leaf
//!   layers directly instead.
//! - A carry variant colliding with one of the base's own variant names is
//!   reported by rustc as a duplicate.
//! - `as_enum` clones the extension value out of the borrowed base value, so
//!   every extension enum must be `Clone` (a `Copy` enum is).

/// View a value as the extension enum it was built from.
///
/// [`enum_ext!`] implements this trait for the base enum against every
/// extension in the composition; each view reads only that extension's carry
/// variant.
pub trait AsEnum<B> {
    fn as_enum(&self) -> Option<B>;
}

/// Compose a base enum with one carry variant per extension enum.
///
/// See the [module documentation](self) for the generated items and the
/// chain/merge story.
#[macro_export]
macro_rules! enum_ext {
    // `+ Ext;` — bare-ident shorthand for a single `+ Ext as Ext;`.
    (
        $(#[$attr:meta])* $vis:vis enum $name:ident { $($own:tt)* }
        + $ext:ident;
    ) => {
        $crate::__enum_ext_emit!(
            $(#[$attr])* $vis enum $name { $($own)* }
            extensions = [ $ext as $ext; ],
        );
    };
    // `+ path as Variant; …` — one carry variant per extension.
    (
        $(#[$attr:meta])* $vis:vis enum $name:ident { $($own:tt)* }
        $(+ $ext:path as $variant:ident;)+
    ) => {
        $crate::__enum_ext_emit!(
            $(#[$attr])* $vis enum $name { $($own)* }
            extensions = [ $($ext as $variant;)* ],
        );
    };
}

/// The single emission site for [`enum_ext!`]. Internal — callers use
/// [`enum_ext!`].
#[doc(hidden)]
#[macro_export]
macro_rules! __enum_ext_emit {
    (
        $(#[$attr:meta])* $vis:vis enum $name:ident { $($own:tt)* }
        extensions = [ $( $ext:path as $variant:ident; )* ],
    ) => {
        $(#[$attr])*
        $vis enum $name {
            $($own)*
            $( $variant($ext), )*
        }
        $(
            impl ::core::convert::From<$ext> for $name {
                fn from(value: $ext) -> Self {
                    Self::$variant(value)
                }
            }
            impl $crate::extend::AsEnum<$ext> for $name {
                fn as_enum(&self) -> ::core::option::Option<$ext> {
                    match self {
                        Self::$variant(value) => {
                            ::core::option::Option::Some(::core::clone::Clone::clone(value))
                        }
                        _ => ::core::option::Option::None,
                    }
                }
            }
        )*
    };
}
