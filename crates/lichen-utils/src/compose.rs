//! Compose a struct of extension component types, exposed through
//! [`AsField`].  See docs/notes/compiler-plugin.md.
//!
//! # Invariant
//!
//! A downstream extending an upstream host lists its components flat alongside
//! its own, in one [`compose_ext!`]: `AsField` needs a flat tuple.

/// View a composed (tuple) struct as one of its component types.
///
/// # Invariant
///
/// [`compose_ext!`] implements this for a composed struct against each of its
/// component types.  `get` borrows immutably, `get_mut` mutably, so a
/// component's methods are reached through the view with no accessor trait.
pub trait AsField<T> {
    fn get(&self) -> &T;
    fn get_mut(&mut self) -> &mut T;
}

/// Compose a tuple struct of component types, generating `impl AsField<T>` for
/// each.
///
/// # Invariant
///
/// Each listed type is one positional field, so field names never collide.  The
/// leading attributes are kept; a component type may appear at most once.
///
/// ```
/// use lichen_utils::compose_ext;
/// struct Component;
/// compose_ext! { struct Host(Component,); }
///
/// let _: &Component = lichen_utils::compose::AsField::<Component>::get(
/// &Host { 0: Component });
/// ```
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

/// Generates an `AsField` impl per tuple position.  Internal helper; callers use
/// [`compose_ext!`].
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
