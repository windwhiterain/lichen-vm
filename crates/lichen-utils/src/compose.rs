//! Compose a struct out of extension component types and expose each
//! component through the [`AsField`] view trait — the struct analogue of
//! [`enum_ext`](crate::extend::enum_ext).
//!
//! An "extension" is an ordinary struct carrying its own state and its own
//! behaviour (inherent methods).  [`compose_ext!`] composes several such
//! extensions into one *tuple* host struct — a host field is a bare extension
//! type, so there is no field-name to collide — and, for every extension type
//! `T`, generates `impl AsField<T> for Host` with `get`/`get_mut`.  The host
//! is then read or mutated per extension by pulling that extension out through
//! `AsField` and calling the extension's own methods; the macro wires no
//! per-extension accessor trait.

/// View a composed (tuple) struct as one of its extension component types.
///
/// [`compose_ext!`] implements this for a composed struct against each of its
/// extension component types.  `get` borrows the component immutably; `get_mut`
/// borrows it mutably — so a component is reached and its own behaviour
/// (inherent methods) invoked without the macro wiring any per-component
/// accessor trait.
pub trait AsField<T> {
    fn get(&self) -> &T;
    fn get_mut(&mut self) -> &mut T;
}

/// Compose a tuple struct out of extension component types and, for every
/// component type `T`, generate `impl AsField<T>` (get/get_mut) for the
/// composed struct.
///
/// Each listed type becomes one positional field of the tuple struct; the
/// component is reached by its type, so field names never collide.  The
/// struct keeps the leading attributes, so the caller can derive `Default`
/// (each component must be `Default`) and whatever else.  No trait is
/// implemented on the components — reach a component through `AsField` and
/// call its inherent methods.
///
/// # Example
///
/// ```
/// use lichen_utils::compose::AsField;
/// use lichen_utils::compose_ext;
///
/// #[derive(Debug, Default)]
/// struct Counter { n: usize }
/// impl Counter {
///     fn bump(&mut self) -> usize { let n = self.n; self.n += 1; n }
/// }
///
/// compose_ext! {
///     #[derive(Debug, Default)]
///     pub struct Host(
///         Counter,
///     );
/// }
///
/// let mut h = Host::default();
/// assert_eq!(AsField::<Counter>::get_mut(&mut h).bump(), 0);
/// assert_eq!(AsField::<Counter>::get_mut(&mut h).bump(), 1);
/// assert_eq!(AsField::<Counter>::get(&h).n, 2);
/// ```
///
/// # Notes
///
/// - A component type may appear at most once, else the `AsField<T>` impls
///   overlap; compose each extension with a distinct type.
/// - The composed struct is not generic; use distinct concrete component types.
/// - `get`/`get_mut` are ambiguous when a host has several components; qualify
///   with `AsField::<T>::get(&host)`.
#[macro_export]
macro_rules! compose_ext {
    // ── entry: a single tuple host struct of component types ──
    (
        $(#[$struct_attr:meta])*
        $vis:vis struct $name:ident(
            $( $fld_ty:ty ,)*
        );
    ) => {
        $(#[$struct_attr])*
        $vis struct $name(
            $( $fld_ty ,)*
        );
        $crate::__compose_ext_as_field!($name; 0; $($fld_ty ,)*);
    };
}

/// Generates an `AsField` impl per tuple position.  Internal helper —
/// callers use [`compose_ext!`].  Each arm pins a literal index and recurses
/// with the next literal, so a tuple position is never computed mid-match.
#[doc(hidden)]
#[macro_export]
macro_rules! __compose_ext_as_field {
    // terminal: no more component types.
    ($name:ident; $idx:tt; ) => {};
    // index 0
    ($name:ident; 0; $ty:ty, $($rest:tt)*) => {
        $crate::__compose_ext_one!($name; 0; $ty);
        $crate::__compose_ext_as_field!($name; 1; $($rest)*);
    };
    ($name:ident; 1; $ty:ty, $($rest:tt)*) => {
        $crate::__compose_ext_one!($name; 1; $ty);
        $crate::__compose_ext_as_field!($name; 2; $($rest)*);
    };
    ($name:ident; 2; $ty:ty, $($rest:tt)*) => {
        $crate::__compose_ext_one!($name; 2; $ty);
        $crate::__compose_ext_as_field!($name; 3; $($rest)*);
    };
    ($name:ident; 3; $ty:ty, $($rest:tt)*) => {
        $crate::__compose_ext_one!($name; 3; $ty);
        $crate::__compose_ext_as_field!($name; 4; $($rest)*);
    };
    ($name:ident; 4; $ty:ty, $($rest:tt)*) => {
        $crate::__compose_ext_one!($name; 4; $ty);
        $crate::__compose_ext_as_field!($name; 5; $($rest)*);
    };
    ($name:ident; 5; $ty:ty, $($rest:tt)*) => {
        $crate::__compose_ext_one!($name; 5; $ty);
        $crate::__compose_ext_as_field!($name; 6; $($rest)*);
    };
    ($name:ident; 6; $ty:ty, $($rest:tt)*) => {
        $crate::__compose_ext_one!($name; 6; $ty);
        $crate::__compose_ext_as_field!($name; 7; $($rest)*);
    };
    ($name:ident; 7; $ty:ty, $($rest:tt)*) => {
        $crate::__compose_ext_one!($name; 7; $ty);
        $crate::__compose_ext_as_field!($name; 8; $($rest)*);
    };
    ($name:ident; 8; $ty:ty, $($rest:tt)*) => {
        $crate::__compose_ext_one!($name; 8; $ty);
        $crate::__compose_ext_as_field!($name; 9; $($rest)*);
    };
    ($name:ident; 9; $ty:ty, $($rest:tt)*) => {
        $crate::__compose_ext_one!($name; 9; $ty);
        $crate::__compose_ext_as_field!($name; 10; $($rest)*);
    };
    ($name:ident; 10; $ty:ty, $($rest:tt)*) => {
        $crate::__compose_ext_one!($name; 10; $ty);
        $crate::__compose_ext_as_field!($name; 11; $($rest)*);
    };
    ($name:ident; 11; $ty:ty, $($rest:tt)*) => {
        $crate::__compose_ext_one!($name; 11; $ty);
        $crate::__compose_ext_as_field!($name; 12; $($rest)*);
    };
    ($name:ident; 12; $ty:ty, $($rest:tt)*) => {
        $crate::__compose_ext_one!($name; 12; $ty);
        $crate::__compose_ext_as_field!($name; 13; $($rest)*);
    };
    ($name:ident; 13; $ty:ty, $($rest:tt)*) => {
        $crate::__compose_ext_one!($name; 13; $ty);
        $crate::__compose_ext_as_field!($name; 14; $($rest)*);
    };
    ($name:ident; 14; $ty:ty, $($rest:tt)*) => {
        $crate::__compose_ext_one!($name; 14; $ty);
        $crate::__compose_ext_as_field!($name; 15; $($rest)*);
    };
    ($name:ident; 15; $ty:ty, $($rest:tt)*) => {
        $crate::__compose_ext_one!($name; 15; $ty);
        $crate::__compose_ext_as_field!($name; 16; $($rest)*);
    };
    // overflow.
    ($name:ident; $idx:tt; $ty:ty, $($rest:tt)*) => {
        compile_error!("compose_ext: too many components (max 16)");
    };
}

/// The body of a single `AsField` impl.  Internal helper.
#[doc(hidden)]
#[macro_export]
macro_rules! __compose_ext_one {
    ($name:ident; $idx:tt; $ty:ty) => {
        impl $crate::compose::AsField<$ty> for $name {
            fn get(&self) -> &$ty {
                &self.$idx
            }
            fn get_mut(&mut self) -> &mut $ty {
                &mut self.$idx
            }
        }
    };
}
