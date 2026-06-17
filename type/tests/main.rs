mod project;

use lichen_core::{
    ast::{Ast as _, AstImpl},
    plugin::{Ast as _, Value as _},
    runtime::{Module, evaluation::Evaluation},
};
use lichen_type::plugin::{Ast as _, Value as _};
use project::{Ast, Project, Value};
