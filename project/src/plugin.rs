use std::fmt::Display;

use crate::Plugin;
use crate::{PROJECT, PROJECT_TRAIT, PROJECT_VARIABLE};

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
