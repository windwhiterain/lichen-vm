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
        $crate::plugin::tokens!{
            #[macro_export]
            plugin_tokens{
                value{$($field)*}{$($unit_field)*}
            }
        }
        #[macro_export]
        macro_rules! value_code{
            ($offset: ident)=>{
                $(
                    #[allow(non_upper_case_globals)]
                    pub(super) const $field: usize = $crate::plugin_define::value_code::$field + $offset;
                )*
                $(
                    #[allow(non_upper_case_globals)]
                    pub(super) const $unit_field: usize = $crate::plugin_define::value_code::$unit_field + $offset;
                )*
            }
        }
        #[macro_export]
        macro_rules! impl_plugin{
            ($module:ident)=>{
                $crate::plugin::paste!{
                    impl ::$module::plugin_define::Value for ValueImpl{
                        $(
                            fn $field(self)->core::option::Option<::$module::plugin_define::$type>{
                                if self.code == self::$module::$field{
                                    Some(unsafe{self.data.$module.$field})
                                }else{
                                    None
                                }
                            }
                        )*
                        $(
                            fn $unit_field(self)->bool{
                                self.code == self::$module::$unit_field
                            }
                        )*
                        $(
                            fn [<from_ $field>](variant: ::$module::plugin_define::$type)->Self{
                                ValueImpl{
                                    code: self::$module::$field,
                                    data: ValueUnionImpl{
                                        $module: ::$module::plugin_define::ValueUnion{
                                            $field: variant,
                                        },
                                    }
                                }
                            }
                        )*
                        $(
                            fn [<from_ $unit_field>]()->Self{
                                ValueImpl{
                                    code: self::$module::$unit_field,
                                    data: ValueUnionImpl{
                                        $module: ::$module::plugin_define::ValueUnion{
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
        #[macro_export]
        macro_rules! match_case{
            ($module:ident, $content:path)=>{
                $(
                    self::$module::$field => {$content!{$field,data.$field}},
                )*
                $(
                    self::$module::$unit_field => {$content!{$unit_field,()}},
                )*
            };
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
    ($($module:ident,)*) => {
        #[derive(Clone,Copy)]
        pub struct ValueImpl{
            code: usize,
            data: ValueUnionImpl,
        }
        #[derive(Clone,Copy)]
        pub struct ValueUnionImpl{
            $($module: ::$module::plugin_define::ValueUnion,)*
        }
        $crate::project!{@offset {}, $($module,)*}
        $(
            ::$module::impl_plugin!{$module}
        )*
        $crate::plugin::group!{token_groups{$(::$module::plugin_tokens : $module,)*}}
        token_groups!{$crate::project {@internal}{}}
    };
    (@offset {$($module_prev:ident)?},) => {};
    (@offset {$($module_prev:ident)?}, $module:ident, $($rest_module:ident,)*) => {
        mod $module{
            pub(super) const OFFSET: usize = $(super::$module_prev + ::$module_prev::plugin_define::VALUE_COUNT +)? 0;
            ::$module::value_code!{OFFSET}
        }
        $crate::project!{@offset {$module}, $($rest_module,)*}
    };
    (@internal {$($module:ident{value{$($field:ident)*}{$($unit_field:ident)*}})*}) => {
        impl std::fmt::Debug for ValueImpl{
            fn fmt(&self, f: &mut std::fmt::Formatter)->std::fmt::Result{
                match self.code{
                    $(
                        $(
                            self::$module::$field => {
                                write!(f,"{}::{}: ",stringify!($module),stringify!($field))?;
                                unsafe{self.data.$module.$field.fmt(f)}
                            }
                        )*
                        $(
                            self::$module::$field => {
                                write!(f,"{}::{}",stringify!($module),stringify!($field))
                            }
                        )*
                    )*
                    _=>unreachable!(),
                }
            }
        }
    };
}

#[macro_export]
macro_rules!  plugin_value_fmt{
    ($name:ident, $value:expr) => {
        f.write!("{}=>",stringify!($name))?;
        value.fmt(f)
    };
}
