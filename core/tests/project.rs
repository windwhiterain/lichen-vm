use ::lichen_core::plugin::Project as _;
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, Hash)]
pub struct Project;
impl ::lichen_core::plugin::Project for Project {
    type Value = Value;
    type Operator = Operator;
    type DiagnosticKind = DiagnosticKind;
}
mod unions {
    use super::Project;

    pub(super) union DiagnosticKind {
        pub(super) core__equality_error:
            std::mem::ManuallyDrop<::lichen_core::runtime::diagnostic::EqualityError>,
    }
    #[derive(Clone, Copy)]
    pub(super) union Value {
        pub(super) core__int: std::mem::ManuallyDrop<::lichen_core::runtime::value::Int>,
        pub(super) core__string: std::mem::ManuallyDrop<::lichen_core::runtime::StringId>,
        pub(super) core__array: std::mem::ManuallyDrop<::lichen_core::runtime::value::Array>,
        pub(super) core__table: std::mem::ManuallyDrop<::lichen_core::runtime::value::Table>,
        pub(super) core__unit: std::mem::ManuallyDrop<::lichen_core::runtime::value::Unit>,
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Operator(usize);

impl std::fmt::Debug for Operator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            0 => {
                write!(f, "core::sum")
            }
            1 => {
                write!(f, "core::index")
            }
            2 => {
                write!(f, "core::find")
            }
            _ => unreachable!(),
        }
    }
}
impl ::lichen_core::plugin::principal_traits::Operator<Project> for Operator {
    fn run(
        &self,
        solver: &mut ::lichen_core::runtime::solve::Solver<Project>,
        value: &<Project as ::lichen_core::plugin::Project>::Value,
        node: &::lichen_core::runtime::solve::LocalNodeId,
    ) -> Option<<Project as ::lichen_core::plugin::Project>::Value> {
        match self.0{
0=>{<::lichen_core::runtime::operation::Sum::<> as ::lichen_core::plugin::principal_traits::Operator::<Project>>::run(unsafe{& ::lichen_core::runtime::operation::Sum::<>},solver,value,node,)
}1=>{<::lichen_core::runtime::operation::Index::<> as ::lichen_core::plugin::principal_traits::Operator::<Project>>::run(unsafe{& ::lichen_core::runtime::operation::Index::<>},solver,value,node,)
}2=>{<::lichen_core::runtime::operation::Find::<> as ::lichen_core::plugin::principal_traits::Operator::<Project>>::run(unsafe{& ::lichen_core::runtime::operation::Find::<>},solver,value,node,)
}_=>unreachable!(),}
    }
}
impl ::lichen_core::plugin::Operator<Project> for Operator {
    fn sum() -> Self {
        Self(0)
    }
    fn index() -> Self {
        Self(1)
    }
    fn find() -> Self {
        Self(2)
    }
}

pub struct DiagnosticKind {
    code: usize,
    data: self::unions::DiagnosticKind,
}
impl Eq for DiagnosticKind {}

impl std::fmt::Debug for DiagnosticKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.code {
            0 => {
                write!(f, "core::equality_error({:?})", unsafe {
                    &*self.data.core__equality_error
                })
            }
            _ => unreachable!(),
        }
    }
}
impl PartialEq for DiagnosticKind {
    fn eq(&self, other: &Self) -> bool {
        if self.code != other.code {
            return false;
        }
        match self.code {
            0 => unsafe { self.data.core__equality_error == other.data.core__equality_error },
            _ => unreachable!(),
        }
    }
    fn ne(&self, other: &Self) -> bool {
        if self.code != other.code {
            return true;
        }
        match self.code {
            0 => unsafe { self.data.core__equality_error != other.data.core__equality_error },
            _ => unreachable!(),
        }
    }
}
impl std::hash::Hash for DiagnosticKind {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.code.hash(state);
        match self.code {
            0 => {
                unsafe { &self.data.core__equality_error }.hash(state);
            }
            _ => unreachable!(),
        }
    }
}
impl Clone for DiagnosticKind {
    fn clone(&self) -> Self {
        match self.code {
            0 => Self {
                code: self.code,
                data: self::unions::DiagnosticKind {
                    core__equality_error: unsafe { &self.data.core__equality_error }.clone(),
                },
            },
            _ => unreachable!(),
        }
    }
}
impl ::lichen_core::plugin::principal_traits::DiagnosticKind<Project> for DiagnosticKind {
    fn message(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.code{
0=>{<::lichen_core::runtime::diagnostic::EqualityError::<> as ::lichen_core::plugin::principal_traits::DiagnosticKind::<Project>>::message(unsafe{& self.data.core__equality_error},f,)
}_=>unreachable!(),}
    }
}
impl ::lichen_core::plugin::DiagnosticKind<Project> for DiagnosticKind {
    fn equality_error(&self) -> Option<&::lichen_core::runtime::diagnostic::EqualityError> {
        if self.code == 0 {
            Some(unsafe { &self.data.core__equality_error })
        } else {
            None
        }
    }
    fn from_equality_error(data: ::lichen_core::runtime::diagnostic::EqualityError) -> Self {
        Self {
            code: 0,
            data: self::unions::DiagnosticKind {
                core__equality_error: std::mem::ManuallyDrop::new(data),
            },
        }
    }
}
#[derive(Clone, Copy)]
pub struct Value {
    code: usize,
    data: self::unions::Value,
}
impl Eq for Value {}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        if self.code != other.code {
            return false;
        }
        match self.code {
            0 => unsafe { self.data.core__int == other.data.core__int },
            1 => unsafe { self.data.core__string == other.data.core__string },
            2 => unsafe { self.data.core__array == other.data.core__array },
            3 => unsafe { self.data.core__table == other.data.core__table },
            4 => unsafe { self.data.core__unit == other.data.core__unit },
            _ => unreachable!(),
        }
    }
    fn ne(&self, other: &Self) -> bool {
        if self.code != other.code {
            return true;
        }
        match self.code {
            0 => unsafe { self.data.core__int != other.data.core__int },
            1 => unsafe { self.data.core__string != other.data.core__string },
            2 => unsafe { self.data.core__array != other.data.core__array },
            3 => unsafe { self.data.core__table != other.data.core__table },
            4 => unsafe { self.data.core__unit != other.data.core__unit },
            _ => unreachable!(),
        }
    }
}
impl std::fmt::Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.code {
            0 => {
                write!(f, "core::int({:?})", unsafe { &*self.data.core__int })
            }
            1 => {
                write!(f, "core::string({:?})", unsafe { &*self.data.core__string })
            }
            2 => {
                write!(f, "core::array({:?})", unsafe { &*self.data.core__array })
            }
            3 => {
                write!(f, "core::table({:?})", unsafe { &*self.data.core__table })
            }
            4 => {
                write!(f, "core::unit")
            }
            _ => unreachable!(),
        }
    }
}
impl ::lichen_core::plugin::principal_traits::Value for Value {
    fn fields(&self) -> impl Iterator<Item = &::lichen_core::runtime::NodeIdLocal> {
        match self.code{
0=>{std::boxed::Box::new(<::lichen_core::runtime::value::Int::<> as ::lichen_core::plugin::principal_traits::Value::<>>::fields(unsafe{& self.data.core__int},)
) as std::boxed::Box<dyn Iterator<Item=&::lichen_core::runtime::NodeIdLocal>>}1=>{std::boxed::Box::new(<::lichen_core::runtime::StringId::<> as ::lichen_core::plugin::principal_traits::Value::<>>::fields(unsafe{& self.data.core__string},)
) as std::boxed::Box<dyn Iterator<Item=&::lichen_core::runtime::NodeIdLocal>>}2=>{std::boxed::Box::new(<::lichen_core::runtime::value::Array::<> as ::lichen_core::plugin::principal_traits::Value::<>>::fields(unsafe{& self.data.core__array},)
) as std::boxed::Box<dyn Iterator<Item=&::lichen_core::runtime::NodeIdLocal>>}3=>{std::boxed::Box::new(<::lichen_core::runtime::value::Table::<> as ::lichen_core::plugin::principal_traits::Value::<>>::fields(unsafe{& self.data.core__table},)
) as std::boxed::Box<dyn Iterator<Item=&::lichen_core::runtime::NodeIdLocal>>}4=>{std::boxed::Box::new(<::lichen_core::runtime::value::Unit::<> as ::lichen_core::plugin::principal_traits::Value::<>>::fields(unsafe{& self.data.core__unit},)
) as std::boxed::Box<dyn Iterator<Item=&::lichen_core::runtime::NodeIdLocal>>}_=>unreachable!(),}
    }
    fn for_fields(&self, mut action: impl FnMut(&::lichen_core::runtime::NodeIdLocal)) {
        match self.code{
0=>{<::lichen_core::runtime::value::Int::<> as ::lichen_core::plugin::principal_traits::Value::<>>::for_fields(unsafe{& self.data.core__int},action,)
}1=>{<::lichen_core::runtime::StringId::<> as ::lichen_core::plugin::principal_traits::Value::<>>::for_fields(unsafe{& self.data.core__string},action,)
}2=>{<::lichen_core::runtime::value::Array::<> as ::lichen_core::plugin::principal_traits::Value::<>>::for_fields(unsafe{& self.data.core__array},action,)
}3=>{<::lichen_core::runtime::value::Table::<> as ::lichen_core::plugin::principal_traits::Value::<>>::for_fields(unsafe{& self.data.core__table},action,)
}4=>{<::lichen_core::runtime::value::Unit::<> as ::lichen_core::plugin::principal_traits::Value::<>>::for_fields(unsafe{& self.data.core__unit},action,)
}_=>unreachable!(),}
    }
    fn for_field_pairs(
        &self,
        other: &Self,
        mut action: impl FnMut(
            &::lichen_core::runtime::NodeIdLocal,
            &::lichen_core::runtime::NodeIdLocal,
        ),
    ) {
        match self.code{
0=>{<::lichen_core::runtime::value::Int::<> as ::lichen_core::plugin::principal_traits::Value::<>>::for_field_pairs(unsafe{& self.data.core__int},unsafe{& other.data.core__int},action,)
}1=>{<::lichen_core::runtime::StringId::<> as ::lichen_core::plugin::principal_traits::Value::<>>::for_field_pairs(unsafe{& self.data.core__string},unsafe{& other.data.core__string},action,)
}2=>{<::lichen_core::runtime::value::Array::<> as ::lichen_core::plugin::principal_traits::Value::<>>::for_field_pairs(unsafe{& self.data.core__array},unsafe{& other.data.core__array},action,)
}3=>{<::lichen_core::runtime::value::Table::<> as ::lichen_core::plugin::principal_traits::Value::<>>::for_field_pairs(unsafe{& self.data.core__table},unsafe{& other.data.core__table},action,)
}4=>{<::lichen_core::runtime::value::Unit::<> as ::lichen_core::plugin::principal_traits::Value::<>>::for_field_pairs(unsafe{& self.data.core__unit},unsafe{& other.data.core__unit},action,)
}_=>unreachable!(),}
    }
}
impl ::lichen_core::plugin::Value for Value {
    fn int(&self) -> Option<&::lichen_core::runtime::value::Int> {
        if self.code == 0 {
            Some(unsafe { &self.data.core__int })
        } else {
            None
        }
    }
    fn from_int(data: ::lichen_core::runtime::value::Int) -> Self {
        Self {
            code: 0,
            data: self::unions::Value {
                core__int: std::mem::ManuallyDrop::new(data),
            },
        }
    }
    fn string(&self) -> Option<&::lichen_core::runtime::StringId> {
        if self.code == 1 {
            Some(unsafe { &self.data.core__string })
        } else {
            None
        }
    }
    fn from_string(data: ::lichen_core::runtime::StringId) -> Self {
        Self {
            code: 1,
            data: self::unions::Value {
                core__string: std::mem::ManuallyDrop::new(data),
            },
        }
    }
    fn array(&self) -> Option<&::lichen_core::runtime::value::Array> {
        if self.code == 2 {
            Some(unsafe { &self.data.core__array })
        } else {
            None
        }
    }
    fn from_array(data: ::lichen_core::runtime::value::Array) -> Self {
        Self {
            code: 2,
            data: self::unions::Value {
                core__array: std::mem::ManuallyDrop::new(data),
            },
        }
    }
    fn table(&self) -> Option<&::lichen_core::runtime::value::Table> {
        if self.code == 3 {
            Some(unsafe { &self.data.core__table })
        } else {
            None
        }
    }
    fn from_table(data: ::lichen_core::runtime::value::Table) -> Self {
        Self {
            code: 3,
            data: self::unions::Value {
                core__table: std::mem::ManuallyDrop::new(data),
            },
        }
    }
    fn unit(&self) -> bool {
        self.code == 4
    }
    fn from_unit() -> Self {
        Self {
            code: 4,
            data: self::unions::Value {
                core__unit: std::mem::ManuallyDrop::new(::lichen_core::runtime::value::Unit),
            },
        }
    }
}
