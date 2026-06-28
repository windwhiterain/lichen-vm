use crate::system::UNION;
use crate::system::sytax::{Bracket, DeKeyword};
use crate::system::{EnumType, Function, Plugin, Variant, utils::generated_principle_trait};

pub trait EnumImpl: Send + Sync {
    fn generate_pre_match(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        enum_type: &EnumType,
        function: &Function,
    ) -> std::fmt::Result;
    fn generate_match_branch(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        enum_type: &EnumType,
        function: &Function,
        variant: &Variant,
        plugin: &Plugin,
    ) -> std::fmt::Result;
}

pub struct Delegate;

impl EnumImpl for Delegate {
    fn generate_pre_match(
        &self,
        _f: &mut std::fmt::Formatter<'_>,
        _enum_type: &EnumType,
        _function: &Function,
    ) -> std::fmt::Result {
        Ok(())
    }

    fn generate_match_branch(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        enum_type: &EnumType,
        function: &Function,
        variant: &Variant,
        plugin: &Plugin,
    ) -> std::fmt::Result {
        if function.return_.is_some() && function.return_impl {
            write!(f, "std::boxed::Box::new(")?;
        }
        write!(
            f,
            "<{} as {}>::{}{}(",
            variant.path,
            generated_principle_trait(enum_type.plugin, *enum_type.name),
            function.name,
            Bracket(function.params.generics)
        )?;
        if let Some(self_) = &function.self_ {
            if enum_type.is_unit {
                write!(f, "unsafe{{{}{}}}", self_.0, variant.path)?;
            } else {
                write!(
                    f,
                    "unsafe{{{}self.data.{}__{}}}",
                    self_.0, plugin.name, variant.name
                )?;
            }
            write!(f, ",")?;
        }
        for param in function.params.this {
            if format!("{}", param.type_) == "Self" {
                write!(
                    f,
                    "unsafe{{{}{}.data.{}__{}}},",
                    param.pass_mode, param.name, plugin.name, variant.name
                )?;
            } else {
                write!(f, "{}", param)?;
            }
        }
        writeln!(f, ")")?;
        if let Some(return_) = function.return_
            && function.return_impl
        {
            writeln!(f, ") as std::boxed::Box<dyn {}>", return_)?;
        }
        Ok(())
    }
}

pub struct PartialEq {
    pub eq_or_ne: bool,
}

impl EnumImpl for PartialEq {
    fn generate_pre_match(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        _enum_type: &EnumType,
        _function: &Function,
    ) -> std::fmt::Result {
        writeln!(f, "if self.code!=other.code{{return {}}}", !self.eq_or_ne)
    }

    fn generate_match_branch(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        _enum_type: &EnumType,
        _function: &Function,
        variant: &Variant,
        plugin: &Plugin,
    ) -> std::fmt::Result {
        writeln!(
            f,
            "unsafe{{self.data.{0}__{1}{2}other.data.{0}__{1}}}",
            plugin.name,
            variant.name,
            if self.eq_or_ne { "==" } else { "!=" }
        )
    }
}

pub struct Hash;

impl EnumImpl for Hash {
    fn generate_pre_match(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        _enum_type: &EnumType,
        _function: &Function,
    ) -> std::fmt::Result {
        writeln!(f, "self.code.hash(state);")
    }

    fn generate_match_branch(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        _enum_type: &EnumType,
        _function: &Function,
        variant: &Variant,
        plugin: &Plugin,
    ) -> std::fmt::Result {
        writeln!(
            f,
            "unsafe{{&self.data.{0}__{1}}}.hash(state);",
            plugin.name, variant.name,
        )
    }
}

pub struct DebugBody;

impl EnumImpl for DebugBody {
    fn generate_pre_match(
        &self,
        _f: &mut std::fmt::Formatter<'_>,
        _enum_type: &EnumType,
        _function: &Function,
    ) -> std::fmt::Result {
        Ok(())
    }

    fn generate_match_branch(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        enum_type: &EnumType,
        _function: &Function,
        variant: &Variant,
        plugin: &Plugin,
    ) -> std::fmt::Result {
        write!(f, "write!(f,\"{}::{}", plugin.name, DeKeyword(variant.name))?;
        if enum_type.is_unit || variant.is_unit {
            write!(f, "\")")?;
        } else {
            write!(
                f,
                "({{:?}})\",unsafe{{&*self.data.{}__{}}})",
                plugin.name, variant.name
            )?;
        }
        writeln!(f, "")?;
        Ok(())
    }
}

pub struct Clone;

impl EnumImpl for Clone {
    fn generate_pre_match(
        &self,
        _f: &mut std::fmt::Formatter<'_>,
        _enum_type: &EnumType,
        _function: &Function,
    ) -> std::fmt::Result {
        Ok(())
    }

    fn generate_match_branch(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        enum_type: &EnumType,
        _function: &Function,
        variant: &Variant,
        plugin: &Plugin,
    ) -> std::fmt::Result {
        writeln!(
            f,
            "Self{{code:self.code,data:self::{UNION}::{0}{{{1}__{2}:unsafe{{&self.data.{1}__{2}}}.clone() }} }}",
            DeKeyword(enum_type.name.name),
            plugin.name,
            variant.name
        )
    }
}
