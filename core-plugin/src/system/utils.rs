use crate::system::sytax::{GeneratedBinPath, GeneratedLibPath, Name};
use crate::system::{Expr, Generics, PRINCIPAL_TRAITS, PROJECT_NAME, Plugin};

pub const fn generated_project_trait(plugin: &'static Plugin) -> GeneratedLibPath {
    GeneratedLibPath {
        plugin,
        relative: "",
        name: PROJECT_NAME,
    }
}

pub const fn generated_trait(plugin: &'static Plugin, name: Name) -> GeneratedLibPath {
    GeneratedLibPath {
        plugin,
        relative: "",
        name,
    }
}

pub const fn generated_struct(name: Name) -> GeneratedBinPath {
    GeneratedBinPath { relative: "", name }
}

pub const fn generated_expr(plugin: &'static Plugin, expr: &'static Expr) -> GeneratedLibPath {
    GeneratedLibPath {
        plugin,
        relative: "expr",
        name: Name {
            name: expr.name,
            generics: &Generics::NONE,
            project_generic: true,
        },
    }
}

pub const fn generated_union(name: Name) -> GeneratedBinPath {
    GeneratedBinPath {
        relative: "union_",
        name,
    }
}

pub const fn generated_principle_trait(plugin: &'static Plugin, name: Name) -> GeneratedLibPath {
    GeneratedLibPath {
        plugin,
        relative: PRINCIPAL_TRAITS,
        name,
    }
}
