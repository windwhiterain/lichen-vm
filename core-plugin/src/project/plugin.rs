use std::fmt::Display;

use crate::project::code::{Name, WrittenPath};
use crate::project::{PROJECT, PROJECT_TRAIT, PROJECT_VARIABLE};
use crate::project::{Plugin, Trait};

pub struct ProjectTrait {
    pub plugin: &'static Plugin,
}

impl Display for ProjectTrait {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}::{PROJECT}", self.plugin.lib_module)
    }
}

pub struct EqualityProjectGeneric<'a> {
    pub associated: &'a (dyn Display + Sync),
    pub target: &'a (dyn Display + Sync),
}

impl Display for EqualityProjectGeneric<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{PROJECT_VARIABLE}: {PROJECT_TRAIT}<{} = {}>",
            self.associated, self.target
        )
    }
}

pub struct Type {
    pub name: &'static Name,
    pub impl_: Option<WrittenPath>,
}
