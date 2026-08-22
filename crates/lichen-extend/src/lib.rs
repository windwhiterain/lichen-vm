//! Proc-macro machinery for cross-crate enum extension.
//!
//! [`enum_ext`] is an attribute for the *extension* enum `B`, applied in its
//! own crate. It re-emits `B` unchanged and generates an exported `extend_B!`
//! macro_rules token carrier. The *base* crate then calls that carrier with
//! its own enum definition; the carrier forwards the base enum, the
//! extension's variant tokens, and the path `($crate::B)` to [`shape`], which
//! emits the spliced enum plus `From<B>` and `AsEnum<B>` impls.
//!
//! # Usage
//!
//! ```text
//! // crate y (depends on lichen-extend)
//! #[lichen_extend::enum_ext]
//! pub enum B { Alpha, Wrap(usize) }
//!
//! // crate x (depends on y and lichen-utils)
//! y::extend_B! { pub enum A { One, Two } }
//! ```
//!
//! expands to roughly
//!
//! ```text
//! pub enum A { One, Two, Alpha, Wrap(usize) }
//! impl From<y::B> for A { .. }    // moves the payload
//! impl AsEnum<y::B> for A { .. }  // clones the payload; base-only variants -> None
//! ```
//!
//! # Constraints
//!
//! - The extension enum must be `pub` and live at its crate's root (the
//!   carrier names it `$crate::B`).
//! - The extension enum must not be generic.
//! - Extension payloads must be `Clone` (the `AsEnum` view clones).
//! - One extension per base enum; to attach several in one crate, chain them
//!   with the same-crate `enum_ext!` macro from `lichen_utils::extend`.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Fields, Ident, ItemEnum, Token, Visibility, braced, parenthesized};

/// Tokens a generated carrier macro forwards to [`shape`]: the base enum, the
/// extension's variants (inside braces, so the list terminates cleanly), and
/// the extension's path (a parenthesized token group, so a `$crate` path
/// never has to be matched as tokens).
struct ShapeInput {
    base: ItemEnum,
    ext_variants: Punctuated<syn::Variant, Token![,]>,
    ext_path: TokenStream2,
}

impl Parse for ShapeInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let base: ItemEnum = input.parse()?;
        input.parse::<Token![|]>()?;
        let variants;
        braced!(variants in input);
        let ext_variants = Punctuated::<syn::Variant, Token![,]>::parse_terminated(&variants)?;
        input.parse::<Token![|]>()?;
        let content;
        parenthesized!(content in input);
        let ext_path: TokenStream2 = content.parse()?;
        Ok(ShapeInput {
            base,
            ext_variants,
            ext_path,
        })
    }
}

/// Attribute applied to the extension enum `B` in its own crate.
#[proc_macro_attribute]
pub fn enum_ext(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let ext = syn::parse_macro_input!(item as ItemEnum);
    if let Err(e) = validate(&ext) {
        return e.into_compile_error().into();
    }
    emit(ext).into()
}

fn validate(ext: &ItemEnum) -> syn::Result<()> {
    if !matches!(ext.vis, Visibility::Public(_)) {
        return Err(syn::Error::new_spanned(
            &ext.vis,
            "enum_ext: the extension enum must be `pub` (its variants are spliced into enums in other crates)",
        ));
    }
    if !ext.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &ext.generics,
            "enum_ext: generic extension enums are not supported",
        ));
    }
    Ok(())
}

fn emit(ext: ItemEnum) -> TokenStream2 {
    let name = &ext.ident;
    let carrier = format_ident!("extend_{}", name);
    let shape_alias = format_ident!("__extend_shape_{}", name);
    let variants_iter = ext.variants.iter();
    quote! {
        #ext

        #[doc(hidden)]
        pub use ::lichen_extend::shape as #shape_alias;

        #[macro_export]
        macro_rules! #carrier {
            ($($base:tt)*) => {
                $crate::#shape_alias! {
                    $($base)*
                    |
                    { #(#variants_iter),* }
                    |
                    ($crate::#name)
                }
            };
        }
    }
}

/// Function-like macro invoked by a generated carrier: takes the base enum,
/// the extension's variant tokens, and the extension's path, and emits the
/// spliced enum plus `From` and `AsEnum` impls.
#[proc_macro]
pub fn shape(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as ShapeInput);
    build(input).into()
}

fn build(
    ShapeInput {
        mut base,
        ext_variants,
        ext_path,
    }: ShapeInput,
) -> TokenStream2 {
    for variant in &ext_variants {
        base.variants.push(variant.clone());
    }
    let name = base.ident.clone();
    let (impl_generics, ty_generics, where_clause) = base.generics.split_for_impl();

    let from_arms = ext_variants.iter().map(|v| arm_from(v, &ext_path, &name));
    let as_arms = ext_variants.iter().map(|v| arm_as(v, &ext_path, &name));

    quote! {
        #base

        impl #impl_generics ::core::convert::From<#ext_path> for #name #ty_generics #where_clause {
            fn from(value: #ext_path) -> Self {
                match value {
                    #(#from_arms),*
                }
            }
        }

        impl #impl_generics ::lichen_utils::extend::AsEnum<#ext_path> for #name #ty_generics #where_clause {
            fn as_enum(&self) -> ::core::option::Option<#ext_path> {
                match self {
                    #(#as_arms,)*
                    _ => ::core::option::Option::None,
                }
            }
        }
    }
}

/// `B::V(..) => A::V(..)` — the payload is moved.
fn arm_from(v: &syn::Variant, path: &TokenStream2, base: &Ident) -> TokenStream2 {
    let name = &v.ident;
    match &v.fields {
        Fields::Unit => quote! { #path::#name => #base::#name },
        Fields::Named(f) => {
            let names: Vec<_> = f.named.iter().map(|f| f.ident.as_ref().unwrap()).collect();
            quote! { #path::#name { #(#names),* } => #base::#name { #(#names),* } }
        }
        Fields::Unnamed(f) => {
            let bindings: Vec<_> = (0..f.unnamed.len())
                .map(|i| format_ident!("__v{}", i))
                .collect();
            quote! { #path::#name ( #(#bindings),* ) => #base::#name ( #(#bindings),* ) }
        }
    }
}

/// `A::V(..) => Some(B::V(..))` — payloads are cloned out of the `&self` match.
fn arm_as(v: &syn::Variant, path: &TokenStream2, base: &Ident) -> TokenStream2 {
    let name = &v.ident;
    match &v.fields {
        Fields::Unit => quote! { #base::#name => ::core::option::Option::Some(#path::#name) },
        Fields::Named(f) => {
            let names: Vec<_> = f.named.iter().map(|f| f.ident.as_ref().unwrap()).collect();
            quote! {
                #base::#name { #(#names),* } =>
                    ::core::option::Option::Some(#path::#name { #(#names: ::core::clone::Clone::clone(#names)),* })
            }
        }
        Fields::Unnamed(f) => {
            let bindings: Vec<_> = (0..f.unnamed.len())
                .map(|i| format_ident!("__v{}", i))
                .collect();
            quote! {
                #base::#name ( #(#bindings),* ) =>
                    ::core::option::Option::Some(#path::#name ( #(::core::clone::Clone::clone(#bindings)),* ))
            }
        }
    }
}
