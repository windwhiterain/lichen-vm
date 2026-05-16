use std::fmt::Debug;

pub use lichen_utils::group;
pub use lichen_utils::tokens;
pub use paste::paste;

pub trait Project: Debug + Clone + Copy + Eq + Default {
    type Value: Copy + Debug + Eq;
    type Operator: crate::runtime::operation::Operator<Self>;
}

#[macro_export]
macro_rules! plugin {
    (value{$($field:ident : $type: ident,)*}{$($unit_field:ident,)*}operator{$($name:ident : $func:path,)*}) => {
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
        pub trait Operator<P:$crate::plugin::Project>: $crate::runtime::operation::Operator<P>{
            $(fn $name()->Self;)*
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
        pub mod operation_func{
            $(
                #[allow(non_camel_case_types)]
                pub use $func as $name;
            )*
        }
        $crate::plugin::tokens!{
            #[macro_export]
            plugin_tokens{
                value{$($field)*}{$($unit_field)*}
                operation{$($name)*}
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
        impl core::cmp::Eq for ValueImpl{}
        #[derive(Clone,Copy)]
        pub struct ValueUnionImpl{
            $($plugin: ::$plugin::plugin_define::ValueUnion,)*
        }
        #[derive(Clone,Copy)]
        pub struct OperatorImpl(usize);
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
        pub struct ProjectImpl;
        impl $crate::plugin::Project for ProjectImpl{
            type Value = ValueImpl;
            type Operator = OperatorImpl;
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
    (@variant_code {value{$($field:ident)*}{$($unit_field:ident)*}$($_:tt)*}) => {
        $crate::project!{@variant_code @internal $($field)* $($unit_field)*}
    };
    (@variant_code @internal $($field:ident)*) => {
        $(
            #[allow(non_upper_case_globals)]
            pub(super) const $field: usize = $crate::plugin_define::value_code::$field + OFFSET;
        )*
    };
    (@mono_impl {$($plugin:ident{value{$($field:ident)*}{$($unit_field:ident)*}operation{$($name:ident)*}})*}) => {
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
                            self::$plugin::$unit_field => {
                                write!(f,"{}::{}",stringify!($plugin),stringify!($unit_field))
                            }
                        )*
                    )*
                    _=>unreachable!(),
                }
            }
        }
        impl core::cmp::PartialEq for ValueImpl{
            fn eq(&self,other:&Self)->bool{
                if self.code!=other.code{return false;}
                match self.code{
                    $(
                        $(
                            self::$plugin::$field => unsafe{self.data.$plugin.$field == other.data.$plugin.$field},
                        )*
                        $(
                            self::$plugin::$unit_field => true,
                        )*
                    )*
                    _=>unreachable!(),
                }
            }
            fn ne(&self,other:&Self)->bool{
                if self.code!=other.code{return true;}
                match self.code{
                    $(
                        $(
                            self::$plugin::$field => unsafe{self.data.$plugin.$field != other.data.$plugin.$field},
                        )*
                        $(
                            self::$plugin::$unit_field => false,
                        )*
                    )*
                    _=>unreachable!(),
                }
            }
        }
        $crate::plugin::paste!{
            #[allow(non_camel_case_types)]
            #[repr(usize)]
            enum OperationCode{
                $($(
                    [<$plugin __ $name>],
                )*)*
            }
            mod operation_code{
                $(
                    pub(super) mod $plugin{
                        $(
                            #[allow(non_upper_case_globals)]
                            pub(in super::super) const $name:usize = super::super::OperationCode::[<$plugin __ $name>] as usize;
                        )*
                    }
                )*
            }
        }

        impl $crate::runtime::operation::Operator<ProjectImpl> for OperatorImpl{
            fn run(self, param:ValueImpl, operation_id: $crate::runtime::NodeId<ProjectImpl>)->Option<ValueImpl>{
                match self.0{
                    $($(
                        self::operation_code::$plugin::$name => ::$plugin::plugin_define::operation_func::$name::<ProjectImpl>(param, operation_id),
                    )*)*
                    _=>unreachable!()
                }
            }
        }
        impl std::fmt::Debug for OperatorImpl{
            fn fmt(&self, f: &mut std::fmt::Formatter)->std::fmt::Result{
                match self.0{
                    0=>write!(f,"none"),
                    $($(
                        self::operation_code::$plugin::$name => write!(f,"{}:{}",stringify!($plugin),stringify!($name)),
                    )*)*
                    _=>unreachable!(),
                }
            }
        }
    };
    (@plugin_impl $plugin:ident {value{$($field:ident)*}{$($unit_field:ident)*}operation{$($name:ident)*}})=>{
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
        impl ::$plugin::plugin_define::Operator<ProjectImpl> for OperatorImpl{
            $(
                fn $name()->Self{
                    Self(self::operation_code::$plugin::$name)
                }
            )*
        }

    }
}
