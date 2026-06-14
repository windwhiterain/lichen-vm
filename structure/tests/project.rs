#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, Hash)]
pub struct Project;
impl ::lichen_structure::plugin::Project for Project {}
impl ::lichen_core::plugin::Project for Project {
    type Value = self::Value;
    type Operator = self::Operator<Self>;
    type DiagnosticKind = self::DiagnosticKind<Self>;
    type Ast = self::Ast<Self>;
}
mod unions {
    #[derive(Clone, Copy)]
    pub(super) union Value {
        pub(super) core__int: std::mem::ManuallyDrop<::lichen_core::runtime::value::Int>,
        pub(super) core__string: std::mem::ManuallyDrop<::lichen_core::runtime::StringId>,
        pub(super) core__array: std::mem::ManuallyDrop<::lichen_core::runtime::value::Array>,
        pub(super) core__table: std::mem::ManuallyDrop<::lichen_core::runtime::value::Table>,
        pub(super) core__unit: std::mem::ManuallyDrop<::lichen_core::runtime::value::Unit>,
        pub(super) structure__named_array: std::mem::ManuallyDrop<::lichen_structure::NamedArray>,
        pub(super) structure__name_set: std::mem::ManuallyDrop<::lichen_structure::NameSet>,
        pub(super) structure__structure: std::mem::ManuallyDrop<::lichen_structure::Structure>,
        _p: core::marker::PhantomData<()>,
    }

    pub(super) union DiagnosticKind<P: ::lichen_structure::plugin::Project> {
        pub(super) core__equality_error:
            std::mem::ManuallyDrop<::lichen_core::runtime::diagnostic::EqualityError>,
        _p: core::marker::PhantomData<(P)>,
    }
}
#[derive(Clone, Copy)]
pub struct Value {
    code: usize,
    data: self::unions::Value,
}
impl Eq for self::Value {}

impl PartialEq for self::Value {
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
            5 => unsafe { self.data.structure__named_array == other.data.structure__named_array },
            6 => unsafe { self.data.structure__name_set == other.data.structure__name_set },
            7 => unsafe { self.data.structure__structure == other.data.structure__structure },
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
            5 => unsafe { self.data.structure__named_array != other.data.structure__named_array },
            6 => unsafe { self.data.structure__name_set != other.data.structure__name_set },
            7 => unsafe { self.data.structure__structure != other.data.structure__structure },
            _ => unreachable!(),
        }
    }
}
impl std::fmt::Debug for self::Value {
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
            5 => {
                write!(f, "structure::named_array({:?})", unsafe {
                    &*self.data.structure__named_array
                })
            }
            6 => {
                write!(f, "structure::name_set({:?})", unsafe {
                    &*self.data.structure__name_set
                })
            }
            7 => {
                write!(f, "structure::structure({:?})", unsafe {
                    &*self.data.structure__structure
                })
            }
            _ => unreachable!(),
        }
    }
}
impl ::lichen_core::plugin::principal_traits::Value for self::Value {
    fn fields(&self) -> impl Iterator<Item = &::lichen_core::runtime::NodeIdLocal> {
        match self.code{
0=>{std::boxed::Box::new(<::lichen_core::runtime::value::Int::<> as ::lichen_core::plugin::principal_traits::Value::<>>::fields(unsafe{& self.data.core__int},)
) as std::boxed::Box<dyn Iterator<Item=&::lichen_core::runtime::NodeIdLocal>>}1=>{std::boxed::Box::new(<::lichen_core::runtime::StringId::<> as ::lichen_core::plugin::principal_traits::Value::<>>::fields(unsafe{& self.data.core__string},)
) as std::boxed::Box<dyn Iterator<Item=&::lichen_core::runtime::NodeIdLocal>>}2=>{std::boxed::Box::new(<::lichen_core::runtime::value::Array::<> as ::lichen_core::plugin::principal_traits::Value::<>>::fields(unsafe{& self.data.core__array},)
) as std::boxed::Box<dyn Iterator<Item=&::lichen_core::runtime::NodeIdLocal>>}3=>{std::boxed::Box::new(<::lichen_core::runtime::value::Table::<> as ::lichen_core::plugin::principal_traits::Value::<>>::fields(unsafe{& self.data.core__table},)
) as std::boxed::Box<dyn Iterator<Item=&::lichen_core::runtime::NodeIdLocal>>}4=>{std::boxed::Box::new(<::lichen_core::runtime::value::Unit::<> as ::lichen_core::plugin::principal_traits::Value::<>>::fields(unsafe{& self.data.core__unit},)
) as std::boxed::Box<dyn Iterator<Item=&::lichen_core::runtime::NodeIdLocal>>}_=>unreachable!(),5=>{std::boxed::Box::new(<::lichen_structure::NamedArray::<> as ::lichen_core::plugin::principal_traits::Value::<>>::fields(unsafe{& self.data.structure__named_array},)
) as std::boxed::Box<dyn Iterator<Item=&::lichen_core::runtime::NodeIdLocal>>}6=>{std::boxed::Box::new(<::lichen_structure::NameSet::<> as ::lichen_core::plugin::principal_traits::Value::<>>::fields(unsafe{& self.data.structure__name_set},)
) as std::boxed::Box<dyn Iterator<Item=&::lichen_core::runtime::NodeIdLocal>>}7=>{std::boxed::Box::new(<::lichen_structure::Structure::<> as ::lichen_core::plugin::principal_traits::Value::<>>::fields(unsafe{& self.data.structure__structure},)
) as std::boxed::Box<dyn Iterator<Item=&::lichen_core::runtime::NodeIdLocal>>}_=>unreachable!(),}
    }
    fn for_fields(&self, mut action: impl FnMut(&::lichen_core::runtime::NodeIdLocal)) {
        match self.code{
0=>{<::lichen_core::runtime::value::Int::<> as ::lichen_core::plugin::principal_traits::Value::<>>::for_fields(unsafe{& self.data.core__int},action,)
}1=>{<::lichen_core::runtime::StringId::<> as ::lichen_core::plugin::principal_traits::Value::<>>::for_fields(unsafe{& self.data.core__string},action,)
}2=>{<::lichen_core::runtime::value::Array::<> as ::lichen_core::plugin::principal_traits::Value::<>>::for_fields(unsafe{& self.data.core__array},action,)
}3=>{<::lichen_core::runtime::value::Table::<> as ::lichen_core::plugin::principal_traits::Value::<>>::for_fields(unsafe{& self.data.core__table},action,)
}4=>{<::lichen_core::runtime::value::Unit::<> as ::lichen_core::plugin::principal_traits::Value::<>>::for_fields(unsafe{& self.data.core__unit},action,)
}_=>unreachable!(),5=>{<::lichen_structure::NamedArray::<> as ::lichen_core::plugin::principal_traits::Value::<>>::for_fields(unsafe{& self.data.structure__named_array},action,)
}6=>{<::lichen_structure::NameSet::<> as ::lichen_core::plugin::principal_traits::Value::<>>::for_fields(unsafe{& self.data.structure__name_set},action,)
}7=>{<::lichen_structure::Structure::<> as ::lichen_core::plugin::principal_traits::Value::<>>::for_fields(unsafe{& self.data.structure__structure},action,)
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
}_=>unreachable!(),5=>{<::lichen_structure::NamedArray::<> as ::lichen_core::plugin::principal_traits::Value::<>>::for_field_pairs(unsafe{& self.data.structure__named_array},unsafe{& other.data.structure__named_array},action,)
}6=>{<::lichen_structure::NameSet::<> as ::lichen_core::plugin::principal_traits::Value::<>>::for_field_pairs(unsafe{& self.data.structure__name_set},unsafe{& other.data.structure__name_set},action,)
}7=>{<::lichen_structure::Structure::<> as ::lichen_core::plugin::principal_traits::Value::<>>::for_field_pairs(unsafe{& self.data.structure__structure},unsafe{& other.data.structure__structure},action,)
}_=>unreachable!(),}
    }
}
impl ::lichen_core::plugin::Value for self::Value {
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
impl ::lichen_structure::plugin::Value for self::Value {
    fn named_array(&self) -> Option<&::lichen_structure::NamedArray> {
        if self.code == 5 {
            Some(unsafe { &self.data.structure__named_array })
        } else {
            None
        }
    }
    fn from_named_array(data: ::lichen_structure::NamedArray) -> Self {
        Self {
            code: 5,
            data: self::unions::Value {
                structure__named_array: std::mem::ManuallyDrop::new(data),
            },
        }
    }
    fn name_set(&self) -> Option<&::lichen_structure::NameSet> {
        if self.code == 6 {
            Some(unsafe { &self.data.structure__name_set })
        } else {
            None
        }
    }
    fn from_name_set(data: ::lichen_structure::NameSet) -> Self {
        Self {
            code: 6,
            data: self::unions::Value {
                structure__name_set: std::mem::ManuallyDrop::new(data),
            },
        }
    }
    fn structure(&self) -> Option<&::lichen_structure::Structure> {
        if self.code == 7 {
            Some(unsafe { &self.data.structure__structure })
        } else {
            None
        }
    }
    fn from_structure(data: ::lichen_structure::Structure) -> Self {
        Self {
            code: 7,
            data: self::unions::Value {
                structure__structure: std::mem::ManuallyDrop::new(data),
            },
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Operator<P: ::lichen_structure::plugin::Project>(usize, core::marker::PhantomData<P>);

impl<P: ::lichen_structure::plugin::Project> std::fmt::Debug for self::Operator<P> {
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
impl<P: ::lichen_structure::plugin::Project> ::lichen_core::plugin::principal_traits::Operator<P>
    for self::Operator<P>
{
    fn run(
        &self,
        solver: &mut ::lichen_core::runtime::solve::Solver<P>,
        value: &<P as ::lichen_core::plugin::Project>::Value,
        node: &::lichen_core::runtime::solve::LocalNodeId,
    ) -> Option<<P as ::lichen_core::plugin::Project>::Value> {
        match self.0{
0=>{<::lichen_core::runtime::operation::Sum::<> as ::lichen_core::plugin::principal_traits::Operator::<P>>::run(unsafe{& ::lichen_core::runtime::operation::Sum::<>},solver,value,node,)
}1=>{<::lichen_core::runtime::operation::Index::<> as ::lichen_core::plugin::principal_traits::Operator::<P>>::run(unsafe{& ::lichen_core::runtime::operation::Index::<>},solver,value,node,)
}2=>{<::lichen_core::runtime::operation::Find::<> as ::lichen_core::plugin::principal_traits::Operator::<P>>::run(unsafe{& ::lichen_core::runtime::operation::Find::<>},solver,value,node,)
}_=>unreachable!(),}
    }
}
impl<P: ::lichen_structure::plugin::Project> ::lichen_core::plugin::Operator<P>
    for self::Operator<P>
{
    fn sum() -> Self {
        Self(0, core::marker::PhantomData)
    }
    fn index() -> Self {
        Self(1, core::marker::PhantomData)
    }
    fn find() -> Self {
        Self(2, core::marker::PhantomData)
    }
}
impl<P: ::lichen_structure::plugin::Project> ::lichen_structure::plugin::Operator<P>
    for self::Operator<P>
{
}

pub struct DiagnosticKind<P: ::lichen_structure::plugin::Project> {
    code: usize,
    data: self::unions::DiagnosticKind<P>,
}
impl<P: ::lichen_structure::plugin::Project> Eq for self::DiagnosticKind<P> {}

impl<P: ::lichen_structure::plugin::Project> std::fmt::Debug for self::DiagnosticKind<P> {
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
impl<P: ::lichen_structure::plugin::Project> PartialEq for self::DiagnosticKind<P> {
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
impl<P: ::lichen_structure::plugin::Project> std::hash::Hash for self::DiagnosticKind<P> {
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
impl<P: ::lichen_structure::plugin::Project> Clone for self::DiagnosticKind<P> {
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
impl<P: ::lichen_structure::plugin::Project>
    ::lichen_core::plugin::principal_traits::DiagnosticKind<P> for self::DiagnosticKind<P>
{
    fn message(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.code{
0=>{<::lichen_core::runtime::diagnostic::EqualityError::<> as ::lichen_core::plugin::principal_traits::DiagnosticKind::<P>>::message(unsafe{& self.data.core__equality_error},f,)
}_=>unreachable!(),}
    }
}
impl<P: ::lichen_structure::plugin::Project> ::lichen_core::plugin::DiagnosticKind<P>
    for self::DiagnosticKind<P>
{
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
impl<P: ::lichen_structure::plugin::Project> ::lichen_structure::plugin::DiagnosticKind<P>
    for self::DiagnosticKind<P>
{
}
pub struct Ast<P: ::lichen_structure::plugin::Project>(pub ::lichen_core::AstImpl<P>);
impl<P: ::lichen_structure::plugin::Project<Ast = Ast<P>>> ::lichen_structure::plugin::Ast<P>
    for Ast<P>
{
    fn structure(&self, expr: &::lichen_core::ExprId) -> ::lichen_core::runtime::NodeIdLocal {
        self.0.property(expr, 0)
    }
    fn add_literal(
        &mut self,
        value: Option<P::Value>,
        structure: Option<P::Value>,
    ) -> ::lichen_core::ExprId {
        let expr = <Self as ::lichen_core::Ast<P>>::add_auto(self);
        if let Some(value) = value {
            let node = <Self as ::lichen_core::plugin::Ast<P>>::value(self, &expr);
            *<Self as ::lichen_core::Ast<P>>::impl_mut(self)
                .module
                .evaluation_mut(&node) = ::lichen_core::runtime::value::Evaluation::Value(value)
        }
        if let Some(structure) = structure {
            let node = <Self as ::lichen_structure::plugin::Ast<P>>::structure(self, &expr);
            *<Self as ::lichen_core::Ast<P>>::impl_mut(self)
                .module
                .evaluation_mut(&node) = ::lichen_core::runtime::value::Evaluation::Value(structure)
        }
        expr
    }
    fn add_member(
        &mut self,
        structure: &::lichen_core::ExprId,
        name: &::lichen_core::ExprId,
    ) -> ::lichen_core::ExprId {
        let output = <Self as ::lichen_core::Ast<P>>::add_auto(self);
        <::lichen_structure::MemberExprImpl as ::lichen_structure::plugin::expr::member<P>>::build(
            self, &output, structure, name,
        );
        output
    }
}
impl<P: ::lichen_structure::plugin::Project<Ast = Ast<P>>> ::lichen_core::plugin::Ast<P>
    for Ast<P>
{
    fn value(&self, expr: &::lichen_core::ExprId) -> ::lichen_core::runtime::NodeIdLocal {
        self.0.property(expr, 1)
    }
    fn add_literal(&mut self, value: Option<P::Value>) -> ::lichen_core::ExprId {
        let expr = <Self as ::lichen_core::Ast<P>>::add_auto(self);
        if let Some(value) = value {
            let node = <Self as ::lichen_core::plugin::Ast<P>>::value(self, &expr);
            *<Self as ::lichen_core::Ast<P>>::impl_mut(self)
                .module
                .evaluation_mut(&node) = ::lichen_core::runtime::value::Evaluation::Value(value)
        }
        expr
    }
    fn add_sum(&mut self, addends: &::lichen_core::ExprId) -> ::lichen_core::ExprId {
        let output = <Self as ::lichen_core::Ast<P>>::add_auto(self);
        <::lichen_core::Sum as ::lichen_core::plugin::expr::sum<P>>::build(self, &output, addends);
        output
    }
    fn add_index(
        &mut self,
        array: &::lichen_core::ExprId,
        index: &::lichen_core::ExprId,
    ) -> ::lichen_core::ExprId {
        let output = <Self as ::lichen_core::Ast<P>>::add_auto(self);
        <::lichen_core::Index as ::lichen_core::plugin::expr::index<P>>::build(
            self, &output, array, index,
        );
        output
    }
    fn add_find(
        &mut self,
        table: &::lichen_core::ExprId,
        name: &::lichen_core::ExprId,
    ) -> ::lichen_core::ExprId {
        let output = <Self as ::lichen_core::Ast<P>>::add_auto(self);
        <::lichen_core::Find as ::lichen_core::plugin::expr::find<P>>::build(
            self, &output, table, name,
        );
        output
    }
}
impl<P: ::lichen_structure::plugin::Project> ::lichen_core::Ast<P> for Ast<P> {
    const PROPERTIES_LEN: usize = 2;
    fn impl_(&self) -> &::lichen_core::AstImpl<P> {
        &self.0
    }
    fn impl_mut(&mut self) -> &mut ::lichen_core::AstImpl<P> {
        &mut self.0
    }
    fn add_auto(&mut self) -> ::lichen_core::ExprId {
        Self::impl_mut(self).add_auto()
    }
    fn add_entry(&mut self, expr: &::lichen_core::ExprId) {
        Self::impl_mut(self).add_entry(expr)
    }
}
