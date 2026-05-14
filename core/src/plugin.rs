pub use lichen_utils::group;
pub use lichen_utils::tokens;
pub use paste::paste;

#[macro_export]
macro_rules! plugin {
    (value{$($field:ident : $type: ident,)*}{$($unit_field:ident,)*}) => {
        $crate::plugin::paste!{
            pub trait Value: Sized + Copy{
                $(
                    fn $field(self)->core::option::Option<$type>;
                )*
                $(
                    fn $unit_field(self)->bool;
                )*
                $(
                    fn [<from_ $field>](variant: $type)->Self;
                )*
                $(
                    fn [<from_ $unit_field>]()->Self;
                )*
            }
        }
        pub const VALUE_COUNT: usize = $crate::plugin!(@count $($field)* $($unit_field)*);
        #[derive(Clone,Copy)]
        pub union ValueUnion{
            $(pub $field: $type,)*
            $(pub $unit_field:(),)*
        }
        #[allow(non_camel_case_types)]
        #[repr(usize)]
        enum ValueCode{
            $($field,)*
            $($unit_field,)*
        }
        pub mod value_code{
            use super::ValueCode;
            $(
                #[allow(non_upper_case_globals)]
                pub const $field: usize = ValueCode::$field as usize;
            )*
            $(
                #[allow(non_upper_case_globals)]
                pub const $unit_field: usize = ValueCode::$unit_field as usize;
            )*
        }
        pub mod value_type{
            $(
                #[allow(non_camel_case_types)]
                pub type $field = super::$type;
            )*
        }
        $crate::plugin::tokens!{
            #[macro_export]
            plugin_tokens{
                value{$($field)*}{$($unit_field)*}
            }
        }
    };
    (@count) => {
        0
    };
    (@count $first:ident $($rest:ident)*) => {
        1 + $crate::plugin!(@count $($rest)*)
    };
}

#[macro_export]
macro_rules! project {
    ($($plugin:ident,)*) => {
        #[derive(Clone,Copy)]
        pub struct ValueImpl{
            code: usize,
            data: ValueUnionImpl,
        }
        #[derive(Clone,Copy)]
        pub struct ValueUnionImpl{
            $($plugin: ::$plugin::plugin_define::ValueUnion,)*
        }
        $crate::project!{@offset {}, $($plugin,)*}
        $(
            ::$plugin::plugin_tokens!{$crate::project {@plugin_impl $plugin} {}}
        )*
        $crate::plugin::group!{token_groups{$(::$plugin::plugin_tokens : $plugin,)*}}
        token_groups!{$crate::project {@mono_impl}{}}
    };
    (@offset {$($plugin_prev:ident)?},) => {};
    (@offset {$($plugin_prev:ident)?}, $plugin:ident, $($rest_plugin:ident,)*) => {
        mod $plugin{
            pub(super) const OFFSET: usize = $(super::$plugin_prev + ::$plugin_prev::plugin_define::VALUE_COUNT +)? 0;
            ::$plugin::plugin_tokens!{$crate::project {@variant_code}{}}
        }
        $crate::project!{@offset {$plugin}, $($rest_plugin,)*}
    };
    (@variant_code {value{$($field:ident)*}{$($unit_field:ident)*}}) => {
        $crate::project!{@variant_code @internal $($field)* $($unit_field)*}
    };
    (@variant_code @internal $($field:ident)*) => {
        $(
            #[allow(non_upper_case_globals)]
            pub(super) const $field: usize = $crate::plugin_define::value_code::$field + OFFSET;
        )*
    };
    (@mono_impl {$($plugin:ident{value{$($field:ident)*}{$($unit_field:ident)*}})*}) => {
        impl std::fmt::Debug for ValueImpl{
            fn fmt(&self, f: &mut std::fmt::Formatter)->std::fmt::Result{
                match self.code{
                    $(
                        $(
                            self::$plugin::$field => {
                                write!(f,"{}::{}: ",stringify!($plugin),stringify!($field))?;
                                unsafe{self.data.$plugin.$field.fmt(f)}
                            }
                        )*
                        $(
                            self::$plugin::$field => {
                                write!(f,"{}::{}",stringify!($plugin),stringify!($field))
                            }
                        )*
                    )*
                    _=>unreachable!(),
                }
            }
        }
    };
    (@plugin_impl $plugin:ident {value{$($field:ident)*}{$($unit_field:ident)*}})=>{
        $crate::plugin::paste!{
            impl ::$plugin::plugin_define::Value for ValueImpl{
                $(
                    fn $field(self)->core::option::Option<::$plugin::plugin_define::value_type::$field>{
                        if self.code == self::$plugin::$field{
                            Some(unsafe{self.data.$plugin.$field})
                        }else{
                            None
                        }
                    }
                )*
                $(
                    fn $unit_field(self)->bool{
                        self.code == self::$plugin::$unit_field
                    }
                )*
                $(
                    fn [<from_ $field>](variant: ::$plugin::plugin_define::value_type::$field)->Self{
                        ValueImpl{
                            code: self::$plugin::$field,
                            data: ValueUnionImpl{
                                $plugin: ::$plugin::plugin_define::ValueUnion{
                                    $field: variant,
                                },
                            }
                        }
                    }
                )*
                $(
                    fn [<from_ $unit_field>]()->Self{
                        ValueImpl{
                            code: self::$plugin::$unit_field,
                            data: ValueUnionImpl{
                                $plugin: ::$plugin::plugin_define::ValueUnion{
                                    $unit_field: (),
                                },
                            }
                        }
                    }
                )*
            }
        }
    }
}
