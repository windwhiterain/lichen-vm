use std::fmt::Display;

use crate::project::{Crate, Delegate, Generics, Impl, PROJECT_GENERIC, PROJECT_VARIABLE, Plugin};

pub struct GenericsOf<'a, T>(pub &'a T);
struct Raw<'a, T>(pub &'a T);

pub struct Name {
    pub name: &'static str,
    pub generics: &'static Generics,
    pub project_generic: bool,
}

impl Name {
    pub fn non_project_generic(&self) -> Self {
        Self {
            name: self.name,
            generics: self.generics,
            project_generic: false,
        }
    }
}

impl Display for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}<{}", self.name, self.generics)?;
        if self.project_generic {
            write!(f, "{}", PROJECT_GENERIC)?;
        }
        write!(f, ">",)?;
        Ok(())
    }
}

impl Display for Delegate<'_, Name> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}::<{}", self.0.name, Delegate(self.0.generics))?;
        if self.0.project_generic {
            write!(f, "{}", PROJECT_GENERIC.name)?;
        }
        write!(f, ">")?;
        Ok(())
    }
}

impl<P: Display> Display for Delegate<'_, Impl<'_, Name, P>> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}::<{}",
            self.0.this.name,
            Delegate(self.0.this.generics)
        )?;
        if self.0.this.project_generic {
            write!(f, "{}", self.0.project)?;
        }
        write!(f, ">")?;
        Ok(())
    }
}

impl Display for GenericsOf<'_, Name> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.generics)?;
        if self.0.project_generic {
            write!(f, "{}", PROJECT_GENERIC)?;
        }
        Ok(())
    }
}

impl Display for Delegate<'_, GenericsOf<'_, Name>> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", Delegate(self.0.0.generics))?;
        if self.0.0.project_generic {
            write!(f, "{}", PROJECT_VARIABLE)?;
        }
        Ok(())
    }
}

pub struct GeneratedLibPath {
    pub plugin: &'static Plugin,
    pub relative: &'static str,
    pub name: &'static Name,
}

impl Display for Raw<'_, GeneratedLibPath> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.plugin.lib_module)?;
        if !self.0.relative.is_empty() {
            write!(f, "::{}", self.0.relative)?;
        }
        Ok(())
    }
}

impl Display for Delegate<'_, GeneratedLibPath> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}::{}", Raw(self.0), Delegate(self.0.name))
    }
}

impl<P: Display> Display for Delegate<'_, Impl<'_, GeneratedLibPath, P>> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}::{}",
            Raw(self.0.this),
            Delegate(&Impl {
                this: self.0.this.name,
                project: self.0.project
            })
        )
    }
}

pub struct GeneratedBinPath {
    pub relative: &'static str,
    pub name: &'static Name,
}

impl Display for Raw<'_, GeneratedBinPath> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "self")?;
        if !self.0.relative.is_empty() {
            write!(f, "::{}", self.0.relative)?;
        }
        Ok(())
    }
}

impl Display for Delegate<'_, GeneratedBinPath> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}::{}", Raw(self.0), Delegate(self.0.name))
    }
}

impl<P: Display> Display for Delegate<'_, Impl<'_, GeneratedBinPath, P>> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}::{}",
            Raw(self.0.this),
            Delegate(&Impl {
                this: self.0.this.name,
                project: self.0.project
            })
        )
    }
}

pub struct WrittenPath {
    pub crate_: &'static str,
    pub path: &'static str,
    pub generics: &'static Generics,
    pub project_generic: bool,
}

impl Display for Raw<'_, WrittenPath> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}::{}", Crate(self.0.crate_), self.0.path)
    }
}

impl Display for Delegate<'_, WrittenPath> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}::<{}", Raw(self.0), Delegate(self.0.generics))?;
        if self.0.project_generic {
            write!(f, "{}", PROJECT_GENERIC.name)?;
        }
        write!(f, ">")?;
        Ok(())
    }
}
