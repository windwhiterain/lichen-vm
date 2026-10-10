//! Compose one enum out of a base definition and a set of extension enums.  See
//! docs/notes/compiler-plugin.md.

/// View a value as the extension enum it was built from.
///
/// # Invariant
///
/// [`enum_ext!`] implements this for the base enum against every extension;
/// each view reads only that extension's carry variant, so a value of another
/// branch reads `None`.
pub trait AsEnum<B> {
    fn as_enum(&self) -> Option<B>;
}

/// Compose a base enum with one carry variant per extension enum.
///
/// # Invariant
///
/// Every extension enum stays the single source of truth for its own variants,
/// which are never spliced; one `From`/`AsEnum` pair is generated per
/// extension.  A downstream lists every ancestor layer's enum flat, plus its
/// own, in one invocation — no nesting, no delegation, so no value is
/// representable under two branches.  Every extension enum must be `Clone`.
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
