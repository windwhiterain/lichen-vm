use std::fmt::Debug;

pub use lichen_utils::group;
pub use lichen_utils::tokens;
pub use paste::paste;

pub trait Project: Debug + Clone + Copy + Eq + Default {
    type Value: Copy + Debug + Eq;
    type Operator: crate::runtime::operation::Operator<Self>;
    type DiagnosticKind: crate::runtime::diagnostic::DiagnosticKind<Self>;
}

#[macro_export]
macro_rules! plugin {
    (value{$($value_variant:ident : $value_type: path,)*}{$($value_unit_variant:ident,)*}operator{$($operator:ident : $func:path,)*}diagnostic_kind{$($diagnostic_kind_variant:ident : $diagnostic_kind_type: path,)*}{$($diagnostic_kind_unit_variant:ident,)*}) => {
        pub mod as_plugin{
            #![allow(non_snake_case)]
            #![allow(non_camel_case_types)]
            #![allow(non_upper_case_globals)]
            $crate::plugin!{@enum Value:$crate::runtime::value::Value{$($value_variant : $value_type,)*}{$($value_unit_variant,)*}}
            $crate::plugin!{@enum DiagnosticKind<P>:$crate::runtime::diagnostic::DiagnosticKind<P>{$($diagnostic_kind_variant : $diagnostic_kind_type,)*}{$($diagnostic_kind_unit_variant,)*}}
            pub trait Operator<P:$crate::plugin::Project>: $crate::runtime::operation::Operator<P>{
                $(fn $operator()->Self;)*
            }
            pub mod operator_func{
                $(
                    pub use $func as $operator;
                )*
            }
            $crate::plugin::tokens!{
                #[macro_export]
                plugin_tokens{
                    value{$($value_variant)*}{$($value_unit_variant)*}
                    operator{$($operator)*}
                    diagnostic_kind{$($diagnostic_kind_variant)*}{$($diagnostic_kind_unit_variant)*}
                }
            }
        }
    };
    (@enum $trait:ident $(<$P:ident>)? : $base_trait:ty{$($variant:ident : $type: path,)*}{$($unit_variant:ident,)*})=>{
        $crate::plugin::paste!{
            pub trait $trait$(<$P: $crate::plugin::Project>)?: $base_trait{
                $(
                    fn $variant(self)->core::option::Option<$type>;
                )*
                $(
                    fn $unit_variant(self)->bool;
                )*
                $(
                    fn [<from_ $variant>](variant: $type)->Self;
                )*
                $(
                    fn [<from_ $unit_variant>]()->Self;
                )*
            }
            pub mod [<$trait _ type>]{
                $(
                    pub use $type as $variant;
                )*
            }
        }
    }
}

#[macro_export]
macro_rules! project {
    ($($plugin:ident,)*) => {
        mod project{
            #![allow(non_snake_case)]
            #![allow(non_camel_case_types)]
            #![allow(non_upper_case_globals)]
            $crate::project!{@auto_enum Value{Clone Copy}{core::cmp::Eq}}
            $crate::project!{@auto_enum DiagnosticKind{}{}}
            #[derive(Clone,Copy)]
            pub struct OperatorImpl(usize);
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
            pub struct ProjectImpl;
            impl $crate::plugin::Project for ProjectImpl{
                type Value = ValueImpl;
                type Operator = OperatorImpl;
                type DiagnosticKind = DiagnosticKindImpl;
            }
            $(
                ::$plugin::plugin_tokens!{$crate::project {@plugin_impl $plugin} {}}
            )*
            $crate::plugin::group!{token_groups{$(::$plugin::plugin_tokens : $plugin,)*}}
            token_groups!{$crate::project {@mono}{}}
        }
    };
    (@auto_enum $trait:ident{$($derive:path)*}{$($marker:path)*})=>{
        $crate::plugin::paste!{
            #[derive($($derive,)*)]
            pub struct [<$trait Impl>]{
                code: usize,
                data: [<$trait Union>]
            }
            $(
                impl $marker for [<$trait Impl>]{}
            )*
        }
    };
    (@mono_enum $trait:ident{$($plugin:ident{$($variant:ident)*}{$($unit_variant:ident)*})*}) =>{
        $crate::plugin::paste!{
            #[derive(Clone,Copy)]
            pub union [<$trait Union>]{
                $($([<$plugin __ $variant>]: ::$plugin::as_plugin::[<$trait _ type>]::$variant,)*)*
                __unit: (),
            }
            mod [<$trait _ code>]{
                #[repr(usize)]
                enum [<$trait  Code>]{
                    $(
                        $([<$plugin __ $variant>],)*
                        $([<$plugin __ $unit_variant>],)*
                    )*
                }
                $(
                    $(
                        pub(super) const [<$plugin __ $variant>]:usize = [<$trait Code>]::[<$plugin __ $variant>] as usize;
                    )*
                    $(
                        pub(super) const [<$plugin __ $unit_variant>]:usize = [<$trait Code>]::[<$plugin __ $unit_variant>] as usize;
                    )*
                )*
            }
        }
    };
    (@mono_enum_debug $trait:ident{$($plugin:ident{$($variant:ident)*}{$($unit_variant:ident)*})*})=>{
        $crate::plugin::paste!{
            impl std::fmt::Debug for [<$trait Impl>]{
                fn fmt(&self, f: &mut std::fmt::Formatter)->std::fmt::Result{
                    match self.code{
                        $(
                            $(
                                self::[<$trait _ code>]::[<$plugin __ $variant>] => {
                                    write!(f,"{}::{}: ",stringify!($plugin),stringify!($variant))?;
                                    unsafe{self.data.[<$plugin __ $variant>].fmt(f)}
                                }
                            )*
                            $(
                                self::[<$trait _ code>]::[<$plugin __ $unit_variant>] => {
                                    write!(f,"{}::{}",stringify!($plugin),stringify!($unit_variant))
                                }
                            )*
                        )*
                        _=>unreachable!(),
                    }
                }
            }
        }
    };
    (@mono_enum_partial_eq $trait:ident{$($plugin:ident{$($variant:ident)*}{$($unit_variant:ident)*})*})=>{
        $crate::plugin::paste!{
            impl core::cmp::PartialEq for ValueImpl{
                fn eq(&self,other:&Self)->bool{
                    if self.code!=other.code{return false;}
                    match self.code{
                        $(
                            $(
                                self::[<$trait _ code>]::[<$plugin __ $variant>] => unsafe{self.data.[<$plugin __ $variant>] == other.data.[<$plugin __ $variant>]},
                            )*
                            $(
                                self::[<$trait _ code>]::[<$plugin __ $unit_variant>] => true,
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
                                self::[<$trait _ code>]::[<$plugin __ $variant>] => unsafe{self.data.[<$plugin __ $variant>] != other.data.[<$plugin __ $variant>]},
                            )*
                            $(
                                self::[<$trait _ code>]::[<$plugin __ $unit_variant>] => false,
                            )*
                        )*
                        _=>unreachable!(),
                    }
                }
            }
        }
    };
    (@mono {$($plugin:ident{value{$($value_variant:ident)*}{$($value_unit_variant:ident)*}operator{$($operator:ident)*}diagnostic_kind{$($diagnostic_kind_variant:ident)*}{$($diagnostic_kind_unit_variant:ident)*}})*}) => {
        $crate::project!{@mono_enum Value{$($plugin{$($value_variant)*}{$($value_unit_variant)*})*}}
        $crate::project!{@mono_enum_debug Value{$($plugin{$($value_variant)*}{$($value_unit_variant)*})*}}
        $crate::project!{@mono_enum_partial_eq Value{$($plugin{$($value_variant)*}{$($value_unit_variant)*})*}}

        $crate::project!{@mono_enum DiagnosticKind{$($plugin{$($diagnostic_kind_variant)*}{$($diagnostic_kind_unit_variant)*})*}}
        $crate::project!{@mono_enum_debug DiagnosticKind{$($plugin{$($diagnostic_kind_variant)*}{$($diagnostic_kind_unit_variant)*})*}}

        impl $crate::runtime::value::Value for ValueImpl{}
        impl $crate::runtime::diagnostic::DiagnosticKind<ProjectImpl> for DiagnosticKindImpl{}

        $crate::plugin::paste!{
            mod operator_code{
                #[repr(usize)]
                enum operatorCode{
                    $($(
                        [<$plugin __ $operator>],
                    )*)*
                }
                $(
                    $(
                        pub(super) const [<$plugin __ $operator>]:usize = operatorCode::[<$plugin __ $operator>] as usize;
                    )*
                )*
            }

            impl $crate::runtime::operation::Operator<ProjectImpl> for OperatorImpl{
                fn run(self, operand:ValueImpl, operator_id: $crate::runtime::NodeId<ProjectImpl>)->Option<ValueImpl>{
                    match self.0{
                        $($(
                            self::operator_code::[<$plugin __ $operator>] => ::$plugin::as_plugin::operator_func::$operator::<ProjectImpl>(operand, operator_id),
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
                            self::operator_code::[<$plugin __ $operator>] => write!(f,"{}:{}",stringify!($plugin),stringify!($operator)),
                        )*)*
                        _=>unreachable!(),
                    }
                }
            }
        }
    };
    (@plugin_enum $trait:ident $(<$P:ident>)? $plugin:ident {$($variant:ident)*}{$($unit_variant:ident)*})=>{
        $crate::plugin::paste!{
            impl ::$plugin::as_plugin::$trait$(<$P>)? for [<$trait Impl>]{
                $(
                    fn $variant(self)->core::option::Option<::$plugin::as_plugin::[<$trait _type>]::$variant>{
                        if self.code == self::[<$trait _code>]::[<$plugin __ $variant>]{
                            Some(unsafe{self.data.[<$plugin __ $variant>]})
                        }else{
                            None
                        }
                    }
                )*
                $(
                    fn $unit_variant(self)->bool{
                        self.code == self::[<$trait _code>]::[<$plugin __ $unit_variant>]
                    }
                )*
                $(
                    fn [<from_ $variant>](variant: ::$plugin::as_plugin::[<$trait _type>]::$variant)->Self{
                        [<$trait Impl>]{
                            code: self::[<$trait _code>]::[<$plugin __ $variant>],
                            data: [<$trait Union>]{
                                [<$plugin __ $variant>]: variant,
                            }
                        }
                    }
                )*
                $(
                    fn [<from_ $unit_variant>]()->Self{
                        ValueImpl{
                            code: self::[<$trait _code>]::[<$plugin __ $unit_variant>],
                            data: ValueUnion{
                                __unit: (),
                            }
                        }
                    }
                )*
            }
        }
    };
    (@plugin_impl $plugin:ident {value{$($value_variant:ident)*}{$($value_unit_variant:ident)*}operator{$($operator:ident)*}diagnostic_kind{$($diagnostic_kind_variant:ident)*}{$($diagnostic_kind_unit_variant:ident)*}})=>{
        $crate::project!{@plugin_enum Value $plugin {$($value_variant)*}{$($value_unit_variant)*}}
        $crate::project!{@plugin_enum DiagnosticKind<ProjectImpl> $plugin {$($diagnostic_kind_variant)*}{$($diagnostic_kind_unit_variant)*}}
        $crate::plugin::paste!{
            impl ::$plugin::as_plugin::Operator<ProjectImpl> for OperatorImpl{
                $(
                    fn $operator()->Self{
                        Self(self::operator_code::[<$plugin __ $operator>])
                    }
                )*
            }
        }
    }
}
