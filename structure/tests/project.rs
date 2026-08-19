// This file is @generated. Do not edit by hand.
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]
#![allow(unused_variables)]
#![allow(dead_code)]
#![allow(unused_unsafe)]
#![allow(unused_mut)]

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, Hash)]
pub struct Project;
impl ::lichen_core::plugin::Project for self::Project {
    type Value = self::Value;
    type Operator = self::Operator<Self>;
    type DiagnosticKind = self::DiagnosticKind<Self>;
    type Ast = self::Ast<Self>;
}
impl ::lichen_structure::plugin::Project for self::Project {}
mod code {
    pub(super) mod Value {
        pub(in super::super) const core__int: usize = 0;
        pub(in super::super) const core__string: usize = 1;
        pub(in super::super) const core__tuple: usize = 2;
        pub(in super::super) const core__table: usize = 3;
        pub(in super::super) const core__unit: usize = 4;
        pub(in super::super) const structure__name_set: usize = 5;
        pub(in super::super) const structure__offsets: usize = 6;
    }
    pub(super) mod Operator {
        pub(in super::super) const structure__compose: usize = 0;
        pub(in super::super) const structure__match: usize = 1;
        pub(in super::super) const structure__transform: usize = 2;
        pub(in super::super) const core__sum: usize = 3;
        pub(in super::super) const core__index: usize = 4;
        pub(in super::super) const core__find: usize = 5;
    }
    pub(super) mod DiagnosticKind {
        pub(in super::super) const structure__member_name_repetition: usize = 0;
        pub(in super::super) const structure__member_name_missing: usize = 1;
        pub(in super::super) const core__unequality: usize = 2;
        pub(in super::super) const core__index_out_of_bounds: usize = 3;
    }
}
mod union_ {
    #[derive(Clone, Copy)]
    pub(super) union Value {
        pub(super) core__int: std::mem::ManuallyDrop<::lichen_core::value::Int>,
        pub(super) core__string: std::mem::ManuallyDrop<::lichen_core::value::StringId>,
        pub(super) core__tuple: std::mem::ManuallyDrop<::lichen_core::value::Tuple>,
        pub(super) core__table: std::mem::ManuallyDrop<::lichen_core::value::Table>,
        pub(super) core__unit: std::mem::ManuallyDrop<::lichen_core::value::Unit>,
        pub(super) structure__name_set: std::mem::ManuallyDrop<::lichen_structure::value::NameSet>,
        pub(super) structure__offsets: std::mem::ManuallyDrop<::lichen_structure::value::Offsets>,
        _p: core::marker::PhantomData<()>,
    }
    #[derive(Clone, Copy)]
    pub(super) union Operator<P: ::lichen_structure::plugin::Project> {
        pub(super) structure__compose:
            std::mem::ManuallyDrop<::lichen_structure::operator::Compose>,
        pub(super) structure__match: std::mem::ManuallyDrop<::lichen_structure::operator::Match>,
        pub(super) structure__transform:
            std::mem::ManuallyDrop<::lichen_structure::operator::Transform>,
        pub(super) core__sum: std::mem::ManuallyDrop<::lichen_core::operator::Sum>,
        pub(super) core__index: std::mem::ManuallyDrop<::lichen_core::operator::Index>,
        pub(super) core__find: std::mem::ManuallyDrop<::lichen_core::operator::Find>,
        _p: core::marker::PhantomData<(P,)>,
    }

    pub(super) union DiagnosticKind<P: ::lichen_structure::plugin::Project> {
        pub(super) structure__member_name_repetition:
            std::mem::ManuallyDrop<::lichen_structure::diagnostic_kind::MemberNameRepetition>,
        pub(super) structure__member_name_missing:
            std::mem::ManuallyDrop<::lichen_structure::diagnostic_kind::MemberNameMissing>,
        pub(super) core__unequality:
            std::mem::ManuallyDrop<::lichen_core::diagnostic_kind::Unequality<P>>,
        pub(super) core__index_out_of_bounds:
            std::mem::ManuallyDrop<::lichen_core::diagnostic_kind::IndexOutOfBounds>,
        _p: core::marker::PhantomData<(P,)>,
    }
}
#[derive(Clone, Copy)]
pub struct Value {
    code: usize,
    data: self::union_::Value,
}
impl Eq for self::Value {}

impl PartialEq for self::Value {
    fn eq(&self, other: &Self) -> bool {
        if self.code != other.code {
            return false;
        }
        match self.code {
            self::code::Value::core__int => unsafe { self.data.core__int == other.data.core__int },
            self::code::Value::core__string => unsafe {
                self.data.core__string == other.data.core__string
            },
            self::code::Value::core__tuple => unsafe {
                self.data.core__tuple == other.data.core__tuple
            },
            self::code::Value::core__table => unsafe {
                self.data.core__table == other.data.core__table
            },
            self::code::Value::core__unit => unsafe {
                self.data.core__unit == other.data.core__unit
            },
            self::code::Value::structure__name_set => unsafe {
                self.data.structure__name_set == other.data.structure__name_set
            },
            self::code::Value::structure__offsets => unsafe {
                self.data.structure__offsets == other.data.structure__offsets
            },
            _ => unreachable!(),
        }
    }
    fn ne(&self, other: &Self) -> bool {
        if self.code != other.code {
            return true;
        }
        match self.code {
            self::code::Value::core__int => unsafe { self.data.core__int != other.data.core__int },
            self::code::Value::core__string => unsafe {
                self.data.core__string != other.data.core__string
            },
            self::code::Value::core__tuple => unsafe {
                self.data.core__tuple != other.data.core__tuple
            },
            self::code::Value::core__table => unsafe {
                self.data.core__table != other.data.core__table
            },
            self::code::Value::core__unit => unsafe {
                self.data.core__unit != other.data.core__unit
            },
            self::code::Value::structure__name_set => unsafe {
                self.data.structure__name_set != other.data.structure__name_set
            },
            self::code::Value::structure__offsets => unsafe {
                self.data.structure__offsets != other.data.structure__offsets
            },
            _ => unreachable!(),
        }
    }
}
impl std::fmt::Debug for self::Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.code {
            self::code::Value::core__int => {
                write!(f, "core::int({:?})", unsafe { &*self.data.core__int })
            }
            self::code::Value::core__string => {
                write!(f, "core::string({:?})", unsafe { &*self.data.core__string })
            }
            self::code::Value::core__tuple => {
                write!(f, "core::tuple({:?})", unsafe { &*self.data.core__tuple })
            }
            self::code::Value::core__table => {
                write!(f, "core::table({:?})", unsafe { &*self.data.core__table })
            }
            self::code::Value::core__unit => {
                write!(f, "core::unit")
            }
            self::code::Value::structure__name_set => {
                write!(f, "structure::name_set({:?})", unsafe {
                    &*self.data.structure__name_set
                })
            }
            self::code::Value::structure__offsets => {
                write!(f, "structure::offsets({:?})", unsafe {
                    &*self.data.structure__offsets
                })
            }
            _ => unreachable!(),
        }
    }
}
impl std::hash::Hash for self::Value {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.code.hash(state);
        match self.code {
            self::code::Value::core__int => {
                unsafe { &self.data.core__int }.hash(state);
            }
            self::code::Value::core__string => {
                unsafe { &self.data.core__string }.hash(state);
            }
            self::code::Value::core__tuple => {
                unsafe { &self.data.core__tuple }.hash(state);
            }
            self::code::Value::core__table => {
                unsafe { &self.data.core__table }.hash(state);
            }
            self::code::Value::core__unit => {
                unsafe { &self.data.core__unit }.hash(state);
            }
            self::code::Value::structure__name_set => {
                unsafe { &self.data.structure__name_set }.hash(state);
            }
            self::code::Value::structure__offsets => {
                unsafe { &self.data.structure__offsets }.hash(state);
            }
            _ => unreachable!(),
        }
    }
}
impl ::lichen_core::plugin::principal_traits::Value for self::Value {
    fn fields(&self) -> impl Iterator<Item = &::lichen_core::runtime::NodeIdLocal> {
        match self.code{
self::code::Value::core__int=>{
std::boxed::Box::new(<::lichen_core::value::Int::<> as ::lichen_core::plugin::principal_traits::Value<>>::fields(unsafe{& self.data.core__int},)
) as std::boxed::Box<dyn Iterator<Item=&::lichen_core::runtime::NodeIdLocal>>
}
self::code::Value::core__string=>{
std::boxed::Box::new(<::lichen_core::value::StringId::<> as ::lichen_core::plugin::principal_traits::Value<>>::fields(unsafe{& self.data.core__string},)
) as std::boxed::Box<dyn Iterator<Item=&::lichen_core::runtime::NodeIdLocal>>
}
self::code::Value::core__tuple=>{
std::boxed::Box::new(<::lichen_core::value::Tuple::<> as ::lichen_core::plugin::principal_traits::Value<>>::fields(unsafe{& self.data.core__tuple},)
) as std::boxed::Box<dyn Iterator<Item=&::lichen_core::runtime::NodeIdLocal>>
}
self::code::Value::core__table=>{
std::boxed::Box::new(<::lichen_core::value::Table::<> as ::lichen_core::plugin::principal_traits::Value<>>::fields(unsafe{& self.data.core__table},)
) as std::boxed::Box<dyn Iterator<Item=&::lichen_core::runtime::NodeIdLocal>>
}
self::code::Value::core__unit=>{
std::boxed::Box::new(<::lichen_core::value::Unit::<> as ::lichen_core::plugin::principal_traits::Value<>>::fields(unsafe{& self.data.core__unit},)
) as std::boxed::Box<dyn Iterator<Item=&::lichen_core::runtime::NodeIdLocal>>
}
self::code::Value::structure__name_set=>{
std::boxed::Box::new(<::lichen_structure::value::NameSet::<> as ::lichen_core::plugin::principal_traits::Value<>>::fields(unsafe{& self.data.structure__name_set},)
) as std::boxed::Box<dyn Iterator<Item=&::lichen_core::runtime::NodeIdLocal>>
}
self::code::Value::structure__offsets=>{
std::boxed::Box::new(<::lichen_structure::value::Offsets::<> as ::lichen_core::plugin::principal_traits::Value<>>::fields(unsafe{& self.data.structure__offsets},)
) as std::boxed::Box<dyn Iterator<Item=&::lichen_core::runtime::NodeIdLocal>>
}
_=>unreachable!(),}
    }
    fn for_fields(&self, mut action: impl FnMut(&::lichen_core::runtime::NodeIdLocal)) {
        match self.code{
self::code::Value::core__int=>{
<::lichen_core::value::Int::<> as ::lichen_core::plugin::principal_traits::Value<>>::for_fields(unsafe{& self.data.core__int},action,)
}
self::code::Value::core__string=>{
<::lichen_core::value::StringId::<> as ::lichen_core::plugin::principal_traits::Value<>>::for_fields(unsafe{& self.data.core__string},action,)
}
self::code::Value::core__tuple=>{
<::lichen_core::value::Tuple::<> as ::lichen_core::plugin::principal_traits::Value<>>::for_fields(unsafe{& self.data.core__tuple},action,)
}
self::code::Value::core__table=>{
<::lichen_core::value::Table::<> as ::lichen_core::plugin::principal_traits::Value<>>::for_fields(unsafe{& self.data.core__table},action,)
}
self::code::Value::core__unit=>{
<::lichen_core::value::Unit::<> as ::lichen_core::plugin::principal_traits::Value<>>::for_fields(unsafe{& self.data.core__unit},action,)
}
self::code::Value::structure__name_set=>{
<::lichen_structure::value::NameSet::<> as ::lichen_core::plugin::principal_traits::Value<>>::for_fields(unsafe{& self.data.structure__name_set},action,)
}
self::code::Value::structure__offsets=>{
<::lichen_structure::value::Offsets::<> as ::lichen_core::plugin::principal_traits::Value<>>::for_fields(unsafe{& self.data.structure__offsets},action,)
}
_=>unreachable!(),}
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
self::code::Value::core__int=>{
<::lichen_core::value::Int::<> as ::lichen_core::plugin::principal_traits::Value<>>::for_field_pairs(unsafe{& self.data.core__int},unsafe{& other.data.core__int},action,)
}
self::code::Value::core__string=>{
<::lichen_core::value::StringId::<> as ::lichen_core::plugin::principal_traits::Value<>>::for_field_pairs(unsafe{& self.data.core__string},unsafe{& other.data.core__string},action,)
}
self::code::Value::core__tuple=>{
<::lichen_core::value::Tuple::<> as ::lichen_core::plugin::principal_traits::Value<>>::for_field_pairs(unsafe{& self.data.core__tuple},unsafe{& other.data.core__tuple},action,)
}
self::code::Value::core__table=>{
<::lichen_core::value::Table::<> as ::lichen_core::plugin::principal_traits::Value<>>::for_field_pairs(unsafe{& self.data.core__table},unsafe{& other.data.core__table},action,)
}
self::code::Value::core__unit=>{
<::lichen_core::value::Unit::<> as ::lichen_core::plugin::principal_traits::Value<>>::for_field_pairs(unsafe{& self.data.core__unit},unsafe{& other.data.core__unit},action,)
}
self::code::Value::structure__name_set=>{
<::lichen_structure::value::NameSet::<> as ::lichen_core::plugin::principal_traits::Value<>>::for_field_pairs(unsafe{& self.data.structure__name_set},unsafe{& other.data.structure__name_set},action,)
}
self::code::Value::structure__offsets=>{
<::lichen_structure::value::Offsets::<> as ::lichen_core::plugin::principal_traits::Value<>>::for_field_pairs(unsafe{& self.data.structure__offsets},unsafe{& other.data.structure__offsets},action,)
}
_=>unreachable!(),}
    }
}
impl ::lichen_core::plugin::Value for self::Value {
    fn as_int(&self) -> Option<&::lichen_core::value::Int> {
        if self.code == self::code::Value::core__int {
            Some(unsafe { &self.data.core__int })
        } else {
            None
        }
    }
    fn int(data: ::lichen_core::value::Int) -> Self {
        Self {
            code: self::code::Value::core__int,
            data: self::union_::Value {
                core__int: std::mem::ManuallyDrop::new(data),
            },
        }
    }
    fn as_string(&self) -> Option<&::lichen_core::value::StringId> {
        if self.code == self::code::Value::core__string {
            Some(unsafe { &self.data.core__string })
        } else {
            None
        }
    }
    fn string(data: ::lichen_core::value::StringId) -> Self {
        Self {
            code: self::code::Value::core__string,
            data: self::union_::Value {
                core__string: std::mem::ManuallyDrop::new(data),
            },
        }
    }
    fn as_tuple(&self) -> Option<&::lichen_core::value::Tuple> {
        if self.code == self::code::Value::core__tuple {
            Some(unsafe { &self.data.core__tuple })
        } else {
            None
        }
    }
    fn tuple(data: ::lichen_core::value::Tuple) -> Self {
        Self {
            code: self::code::Value::core__tuple,
            data: self::union_::Value {
                core__tuple: std::mem::ManuallyDrop::new(data),
            },
        }
    }
    fn as_table(&self) -> Option<&::lichen_core::value::Table> {
        if self.code == self::code::Value::core__table {
            Some(unsafe { &self.data.core__table })
        } else {
            None
        }
    }
    fn table(data: ::lichen_core::value::Table) -> Self {
        Self {
            code: self::code::Value::core__table,
            data: self::union_::Value {
                core__table: std::mem::ManuallyDrop::new(data),
            },
        }
    }
    fn as_unit(&self) -> bool {
        self.code == self::code::Value::core__unit
    }
    fn unit() -> Self {
        Self {
            code: self::code::Value::core__unit,
            data: self::union_::Value {
                core__unit: std::mem::ManuallyDrop::new(::lichen_core::value::Unit),
            },
        }
    }
}
impl ::lichen_structure::plugin::Value for self::Value {
    fn as_name_set(&self) -> Option<&::lichen_structure::value::NameSet> {
        if self.code == self::code::Value::structure__name_set {
            Some(unsafe { &self.data.structure__name_set })
        } else {
            None
        }
    }
    fn name_set(data: ::lichen_structure::value::NameSet) -> Self {
        Self {
            code: self::code::Value::structure__name_set,
            data: self::union_::Value {
                structure__name_set: std::mem::ManuallyDrop::new(data),
            },
        }
    }
    fn as_offsets(&self) -> Option<&::lichen_structure::value::Offsets> {
        if self.code == self::code::Value::structure__offsets {
            Some(unsafe { &self.data.structure__offsets })
        } else {
            None
        }
    }
    fn offsets(data: ::lichen_structure::value::Offsets) -> Self {
        Self {
            code: self::code::Value::structure__offsets,
            data: self::union_::Value {
                structure__offsets: std::mem::ManuallyDrop::new(data),
            },
        }
    }
}
#[derive(Clone, Copy)]
pub struct Operator<P: ::lichen_structure::plugin::Project> {
    code: usize,
    data: self::union_::Operator<P>,
}
impl<P: ::lichen_structure::plugin::Project> Eq for self::Operator<P> {}

impl<P: ::lichen_structure::plugin::Project> std::fmt::Debug for self::Operator<P>
where
    P::Value: ::lichen_structure::plugin::Value,
    P::DiagnosticKind: ::lichen_structure::plugin::DiagnosticKind<P>,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.code {
            self::code::Operator::structure__compose => {
                write!(f, "structure::compose")
            }
            self::code::Operator::structure__match => {
                write!(f, "structure::r#match")
            }
            self::code::Operator::structure__transform => {
                write!(f, "structure::transform")
            }
            self::code::Operator::core__sum => {
                write!(f, "core::sum")
            }
            self::code::Operator::core__index => {
                write!(f, "core::index({:?})", unsafe { &*self.data.core__index })
            }
            self::code::Operator::core__find => {
                write!(f, "core::find")
            }
            _ => unreachable!(),
        }
    }
}
impl<P: ::lichen_structure::plugin::Project> PartialEq for self::Operator<P>
where
    P::Value: ::lichen_structure::plugin::Value,
    P::DiagnosticKind: ::lichen_structure::plugin::DiagnosticKind<P>,
{
    fn eq(&self, other: &Self) -> bool {
        if self.code != other.code {
            return false;
        }
        match self.code {
            self::code::Operator::structure__compose => unsafe {
                self.data.structure__compose == other.data.structure__compose
            },
            self::code::Operator::structure__match => unsafe {
                self.data.structure__match == other.data.structure__match
            },
            self::code::Operator::structure__transform => unsafe {
                self.data.structure__transform == other.data.structure__transform
            },
            self::code::Operator::core__sum => unsafe {
                self.data.core__sum == other.data.core__sum
            },
            self::code::Operator::core__index => unsafe {
                self.data.core__index == other.data.core__index
            },
            self::code::Operator::core__find => unsafe {
                self.data.core__find == other.data.core__find
            },
            _ => unreachable!(),
        }
    }
    fn ne(&self, other: &Self) -> bool {
        if self.code != other.code {
            return true;
        }
        match self.code {
            self::code::Operator::structure__compose => unsafe {
                self.data.structure__compose != other.data.structure__compose
            },
            self::code::Operator::structure__match => unsafe {
                self.data.structure__match != other.data.structure__match
            },
            self::code::Operator::structure__transform => unsafe {
                self.data.structure__transform != other.data.structure__transform
            },
            self::code::Operator::core__sum => unsafe {
                self.data.core__sum != other.data.core__sum
            },
            self::code::Operator::core__index => unsafe {
                self.data.core__index != other.data.core__index
            },
            self::code::Operator::core__find => unsafe {
                self.data.core__find != other.data.core__find
            },
            _ => unreachable!(),
        }
    }
}
impl<P: ::lichen_structure::plugin::Project> ::lichen_core::plugin::principal_traits::Operator<P>
    for self::Operator<P>
where
    P::Value: ::lichen_structure::plugin::Value,
    P::DiagnosticKind: ::lichen_structure::plugin::DiagnosticKind<P>,
{
    fn run(
        &self,
        solver: &mut ::lichen_core::runtime::solve::Solver<P>,
        operand: &<P as ::lichen_core::plugin::Project>::Value,
        node: &::lichen_core::runtime::solve::LocalNodeId,
    ) -> ::lichen_core::runtime::operation::Option<P> {
        match self.code{
self::code::Operator::structure__compose=>{
<::lichen_structure::operator::Compose::<> as ::lichen_core::plugin::principal_traits::Operator<P,>>::run(unsafe{& self.data.structure__compose},solver,operand,node,)
}
self::code::Operator::structure__match=>{
<::lichen_structure::operator::Match::<> as ::lichen_core::plugin::principal_traits::Operator<P,>>::run(unsafe{& self.data.structure__match},solver,operand,node,)
}
self::code::Operator::structure__transform=>{
<::lichen_structure::operator::Transform::<> as ::lichen_core::plugin::principal_traits::Operator<P,>>::run(unsafe{& self.data.structure__transform},solver,operand,node,)
}
self::code::Operator::core__sum=>{
<::lichen_core::operator::Sum::<> as ::lichen_core::plugin::principal_traits::Operator<P,>>::run(unsafe{& self.data.core__sum},solver,operand,node,)
}
self::code::Operator::core__index=>{
<::lichen_core::operator::Index::<> as ::lichen_core::plugin::principal_traits::Operator<P,>>::run(unsafe{& self.data.core__index},solver,operand,node,)
}
self::code::Operator::core__find=>{
<::lichen_core::operator::Find::<> as ::lichen_core::plugin::principal_traits::Operator<P,>>::run(unsafe{& self.data.core__find},solver,operand,node,)
}
_=>unreachable!(),}
    }
}
impl<P: ::lichen_structure::plugin::Project> ::lichen_core::plugin::Operator<P>
    for self::Operator<P>
{
    fn as_sum(&self) -> bool {
        self.code == self::code::Operator::core__sum
    }
    fn sum() -> Self {
        Self {
            code: self::code::Operator::core__sum,
            data: self::union_::Operator {
                core__sum: std::mem::ManuallyDrop::new(::lichen_core::operator::Sum),
            },
        }
    }
    fn as_index(&self) -> Option<&::lichen_core::operator::Index> {
        if self.code == self::code::Operator::core__index {
            Some(unsafe { &self.data.core__index })
        } else {
            None
        }
    }
    fn index(data: ::lichen_core::operator::Index) -> Self {
        Self {
            code: self::code::Operator::core__index,
            data: self::union_::Operator {
                core__index: std::mem::ManuallyDrop::new(data),
            },
        }
    }
    fn as_find(&self) -> bool {
        self.code == self::code::Operator::core__find
    }
    fn find() -> Self {
        Self {
            code: self::code::Operator::core__find,
            data: self::union_::Operator {
                core__find: std::mem::ManuallyDrop::new(::lichen_core::operator::Find),
            },
        }
    }
}
impl<P: ::lichen_structure::plugin::Project> ::lichen_structure::plugin::Operator<P>
    for self::Operator<P>
{
    fn as_compose(&self) -> bool {
        self.code == self::code::Operator::structure__compose
    }
    fn compose() -> Self {
        Self {
            code: self::code::Operator::structure__compose,
            data: self::union_::Operator {
                structure__compose: std::mem::ManuallyDrop::new(
                    ::lichen_structure::operator::Compose,
                ),
            },
        }
    }
    fn as_match(&self) -> bool {
        self.code == self::code::Operator::structure__match
    }
    fn r#match() -> Self {
        Self {
            code: self::code::Operator::structure__match,
            data: self::union_::Operator {
                structure__match: std::mem::ManuallyDrop::new(::lichen_structure::operator::Match),
            },
        }
    }
    fn as_transform(&self) -> bool {
        self.code == self::code::Operator::structure__transform
    }
    fn transform() -> Self {
        Self {
            code: self::code::Operator::structure__transform,
            data: self::union_::Operator {
                structure__transform: std::mem::ManuallyDrop::new(
                    ::lichen_structure::operator::Transform,
                ),
            },
        }
    }
}

pub struct DiagnosticKind<P: ::lichen_structure::plugin::Project> {
    code: usize,
    data: self::union_::DiagnosticKind<P>,
}
impl<P: ::lichen_structure::plugin::Project> Eq for self::DiagnosticKind<P> {}

impl<P: ::lichen_structure::plugin::Project> std::fmt::Debug for self::DiagnosticKind<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.code {
            self::code::DiagnosticKind::structure__member_name_repetition => {
                write!(f, "structure::member_name_repetition({:?})", unsafe {
                    &*self.data.structure__member_name_repetition
                })
            }
            self::code::DiagnosticKind::structure__member_name_missing => {
                write!(f, "structure::member_name_missing({:?})", unsafe {
                    &*self.data.structure__member_name_missing
                })
            }
            self::code::DiagnosticKind::core__unequality => {
                write!(f, "core::unequality({:?})", unsafe {
                    &*self.data.core__unequality
                })
            }
            self::code::DiagnosticKind::core__index_out_of_bounds => {
                write!(f, "core::index_out_of_bounds({:?})", unsafe {
                    &*self.data.core__index_out_of_bounds
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
            self::code::DiagnosticKind::structure__member_name_repetition => unsafe {
                self.data.structure__member_name_repetition
                    == other.data.structure__member_name_repetition
            },
            self::code::DiagnosticKind::structure__member_name_missing => unsafe {
                self.data.structure__member_name_missing
                    == other.data.structure__member_name_missing
            },
            self::code::DiagnosticKind::core__unequality => unsafe {
                self.data.core__unequality == other.data.core__unequality
            },
            self::code::DiagnosticKind::core__index_out_of_bounds => unsafe {
                self.data.core__index_out_of_bounds == other.data.core__index_out_of_bounds
            },
            _ => unreachable!(),
        }
    }
    fn ne(&self, other: &Self) -> bool {
        if self.code != other.code {
            return true;
        }
        match self.code {
            self::code::DiagnosticKind::structure__member_name_repetition => unsafe {
                self.data.structure__member_name_repetition
                    != other.data.structure__member_name_repetition
            },
            self::code::DiagnosticKind::structure__member_name_missing => unsafe {
                self.data.structure__member_name_missing
                    != other.data.structure__member_name_missing
            },
            self::code::DiagnosticKind::core__unequality => unsafe {
                self.data.core__unequality != other.data.core__unequality
            },
            self::code::DiagnosticKind::core__index_out_of_bounds => unsafe {
                self.data.core__index_out_of_bounds != other.data.core__index_out_of_bounds
            },
            _ => unreachable!(),
        }
    }
}
impl<P: ::lichen_structure::plugin::Project> std::hash::Hash for self::DiagnosticKind<P> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.code.hash(state);
        match self.code {
            self::code::DiagnosticKind::structure__member_name_repetition => {
                unsafe { &self.data.structure__member_name_repetition }.hash(state);
            }
            self::code::DiagnosticKind::structure__member_name_missing => {
                unsafe { &self.data.structure__member_name_missing }.hash(state);
            }
            self::code::DiagnosticKind::core__unequality => {
                unsafe { &self.data.core__unequality }.hash(state);
            }
            self::code::DiagnosticKind::core__index_out_of_bounds => {
                unsafe { &self.data.core__index_out_of_bounds }.hash(state);
            }
            _ => unreachable!(),
        }
    }
}
impl<P: ::lichen_structure::plugin::Project> Clone for self::DiagnosticKind<P> {
    fn clone(&self) -> Self {
        match self.code {
            self::code::DiagnosticKind::structure__member_name_repetition => Self {
                code: self.code,
                data: self::union_::DiagnosticKind {
                    structure__member_name_repetition: unsafe {
                        &self.data.structure__member_name_repetition
                    }
                    .clone(),
                },
            },
            self::code::DiagnosticKind::structure__member_name_missing => Self {
                code: self.code,
                data: self::union_::DiagnosticKind {
                    structure__member_name_missing: unsafe {
                        &self.data.structure__member_name_missing
                    }
                    .clone(),
                },
            },
            self::code::DiagnosticKind::core__unequality => Self {
                code: self.code,
                data: self::union_::DiagnosticKind {
                    core__unequality: unsafe { &self.data.core__unequality }.clone(),
                },
            },
            self::code::DiagnosticKind::core__index_out_of_bounds => Self {
                code: self.code,
                data: self::union_::DiagnosticKind {
                    core__index_out_of_bounds: unsafe { &self.data.core__index_out_of_bounds }
                        .clone(),
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
self::code::DiagnosticKind::structure__member_name_repetition=>{
<::lichen_structure::diagnostic_kind::MemberNameRepetition::<> as ::lichen_core::plugin::principal_traits::DiagnosticKind<P,>>::message(unsafe{& self.data.structure__member_name_repetition},f,)
}
self::code::DiagnosticKind::structure__member_name_missing=>{
<::lichen_structure::diagnostic_kind::MemberNameMissing::<> as ::lichen_core::plugin::principal_traits::DiagnosticKind<P,>>::message(unsafe{& self.data.structure__member_name_missing},f,)
}
self::code::DiagnosticKind::core__unequality=>{
<::lichen_core::diagnostic_kind::Unequality::<P,> as ::lichen_core::plugin::principal_traits::DiagnosticKind<P,>>::message(unsafe{& self.data.core__unequality},f,)
}
self::code::DiagnosticKind::core__index_out_of_bounds=>{
<::lichen_core::diagnostic_kind::IndexOutOfBounds::<> as ::lichen_core::plugin::principal_traits::DiagnosticKind<P,>>::message(unsafe{& self.data.core__index_out_of_bounds},f,)
}
_=>unreachable!(),}
    }
}
impl<P: ::lichen_structure::plugin::Project> ::lichen_core::plugin::DiagnosticKind<P>
    for self::DiagnosticKind<P>
{
    fn as_unequality(&self) -> Option<&::lichen_core::diagnostic_kind::Unequality<P>> {
        if self.code == self::code::DiagnosticKind::core__unequality {
            Some(unsafe { &self.data.core__unequality })
        } else {
            None
        }
    }
    fn unequality(data: ::lichen_core::diagnostic_kind::Unequality<P>) -> Self {
        Self {
            code: self::code::DiagnosticKind::core__unequality,
            data: self::union_::DiagnosticKind {
                core__unequality: std::mem::ManuallyDrop::new(data),
            },
        }
    }
    fn as_index_out_of_bounds(&self) -> Option<&::lichen_core::diagnostic_kind::IndexOutOfBounds> {
        if self.code == self::code::DiagnosticKind::core__index_out_of_bounds {
            Some(unsafe { &self.data.core__index_out_of_bounds })
        } else {
            None
        }
    }
    fn index_out_of_bounds(data: ::lichen_core::diagnostic_kind::IndexOutOfBounds) -> Self {
        Self {
            code: self::code::DiagnosticKind::core__index_out_of_bounds,
            data: self::union_::DiagnosticKind {
                core__index_out_of_bounds: std::mem::ManuallyDrop::new(data),
            },
        }
    }
}
impl<P: ::lichen_structure::plugin::Project> ::lichen_structure::plugin::DiagnosticKind<P>
    for self::DiagnosticKind<P>
{
    fn as_member_name_repetition(
        &self,
    ) -> Option<&::lichen_structure::diagnostic_kind::MemberNameRepetition> {
        if self.code == self::code::DiagnosticKind::structure__member_name_repetition {
            Some(unsafe { &self.data.structure__member_name_repetition })
        } else {
            None
        }
    }
    fn member_name_repetition(
        data: ::lichen_structure::diagnostic_kind::MemberNameRepetition,
    ) -> Self {
        Self {
            code: self::code::DiagnosticKind::structure__member_name_repetition,
            data: self::union_::DiagnosticKind {
                structure__member_name_repetition: std::mem::ManuallyDrop::new(data),
            },
        }
    }
    fn as_member_name_missing(
        &self,
    ) -> Option<&::lichen_structure::diagnostic_kind::MemberNameMissing> {
        if self.code == self::code::DiagnosticKind::structure__member_name_missing {
            Some(unsafe { &self.data.structure__member_name_missing })
        } else {
            None
        }
    }
    fn member_name_missing(data: ::lichen_structure::diagnostic_kind::MemberNameMissing) -> Self {
        Self {
            code: self::code::DiagnosticKind::structure__member_name_missing,
            data: self::union_::DiagnosticKind {
                structure__member_name_missing: std::mem::ManuallyDrop::new(data),
            },
        }
    }
}
impl<P: ::lichen_structure::plugin::Project<Ast = self::Ast<P>>> ::lichen_core::plugin::Ast<P>
    for self::Ast<P>
where
    P::Operator: ::lichen_structure::plugin::Operator<P>,
{
    fn add_primitive_core(&mut self, value: Option<P::Value>) -> ::lichen_core::ast::ExprId {
        let expr = <Self as ::lichen_core::ast::Ast<P>>::add_auto_primitive(self);
        if let Some(value) = value {
            let node = self.impl_.value(&expr);
            *self.impl_.module.evaluation_mut(&node) =
                ::lichen_core::runtime::evaluation::Evaluation::Value(value)
        }
        expr
    }
    fn add_sum(&mut self, addends: &::lichen_core::ast::ExprId) -> ::lichen_core::ast::ExprId {
        let output = <Self as ::lichen_core::ast::Ast<P>>::add_auto(self);
        <::lichen_structure::expr_impl::Sum as ::lichen_core::plugin::expr::sum<P>>::build(
            self, &output, addends,
        );
        <::lichen_core::expr_impl::Sum as ::lichen_core::plugin::expr::sum<P>>::build(
            self, &output, addends,
        );
        output
    }
    fn add_index(
        &mut self,
        array: &::lichen_core::ast::ExprId,
        index: &::lichen_core::ast::ExprId,
    ) -> ::lichen_core::ast::ExprId {
        let output = <Self as ::lichen_core::ast::Ast<P>>::add_auto(self);
        <::lichen_structure::expr_impl::Index as ::lichen_core::plugin::expr::index<P>>::build(
            self, &output, array, index,
        );
        <::lichen_core::expr_impl::Index as ::lichen_core::plugin::expr::index<P>>::build(
            self, &output, array, index,
        );
        output
    }
    fn add_find(
        &mut self,
        table: &::lichen_core::ast::ExprId,
        name: &::lichen_core::ast::ExprId,
    ) -> ::lichen_core::ast::ExprId {
        let output = <Self as ::lichen_core::ast::Ast<P>>::add_auto(self);
        <::lichen_structure::expr_impl::Find as ::lichen_core::plugin::expr::find<P>>::build(
            self, &output, table, name,
        );
        <::lichen_core::expr_impl::Find as ::lichen_core::plugin::expr::find<P>>::build(
            self, &output, table, name,
        );
        output
    }
    fn add_array<'a>(
        &mut self,
        items: impl IntoIterator<Item = &'a ::lichen_core::ast::ExprId> + Copy,
    ) -> ::lichen_core::ast::ExprId {
        let output = <Self as ::lichen_core::ast::Ast<P>>::add_auto(self);
        <::lichen_structure::expr_impl::Array as ::lichen_core::plugin::expr::array<P>>::build(
            self, &output, items,
        );
        <::lichen_core::expr_impl::Tuple as ::lichen_core::plugin::expr::array<P>>::build(
            self, &output, items,
        );
        output
    }
}
impl<P: ::lichen_structure::plugin::Project<Ast = self::Ast<P>>> ::lichen_structure::plugin::Ast<P>
    for self::Ast<P>
where
    P::Operator: ::lichen_structure::plugin::Operator<P>,
{
    fn structure(&self, expr: &::lichen_core::ast::ExprId) -> ::lichen_core::runtime::NodeIdLocal {
        self.impl_.get_property(expr, 1, 0)
    }
    fn structure_components(
        &self,
        expr: &::lichen_core::ast::ExprId,
    ) -> ::lichen_core::runtime::NodeIdLocal {
        self.impl_.get_property(expr, 1, 1)
    }
    fn add_primitive_structure(
        &mut self,
        value: Option<P::Value>,
        structure: Option<P::Value>,
    ) -> ::lichen_core::ast::ExprId {
        let expr = <Self as ::lichen_core::ast::Ast<P>>::add_auto_primitive(self);
        if let Some(value) = value {
            let node = self.impl_.value(&expr);
            *self.impl_.module.evaluation_mut(&node) =
                ::lichen_core::runtime::evaluation::Evaluation::Value(value)
        }
        if let Some(structure) = structure {
            let node = <Self as ::lichen_structure::plugin::Ast<P>>::structure(self, &expr);
            *self.impl_.module.evaluation_mut(&node) =
                ::lichen_core::runtime::evaluation::Evaluation::Value(structure)
        }
        expr
    }
    fn add_member(
        &mut self,
        instance: &::lichen_core::ast::ExprId,
        name: &::lichen_core::ast::ExprId,
    ) -> ::lichen_core::ast::ExprId {
        let output = <Self as ::lichen_core::ast::Ast<P>>::add_auto(self);
        <::lichen_structure::expr_impl::Member as ::lichen_structure::plugin::expr::member<P,>>::build(self,&output,instance,name,);
        output
    }
    fn add_compose(
        &mut self,
        name_set: &::lichen_core::ast::ExprId,
        structures: &::lichen_core::ast::ExprId,
    ) -> ::lichen_core::ast::ExprId {
        let output = <Self as ::lichen_core::ast::Ast<P>>::add_auto(self);
        <::lichen_structure::expr_impl::Compose as ::lichen_structure::plugin::expr::compose<P,>>::build(self,&output,name_set,structures,);
        output
    }
    fn add_construct(
        &mut self,
        structure: &::lichen_core::ast::ExprId,
        name_set: &::lichen_core::ast::ExprId,
        members: &::lichen_core::ast::ExprId,
    ) -> ::lichen_core::ast::ExprId {
        let output = <Self as ::lichen_core::ast::Ast<P>>::add_auto(self);
        <::lichen_structure::expr_impl::Construct as ::lichen_structure::plugin::expr::construct<
            P,
        >>::build(self, &output, structure, name_set, members);
        output
    }
}
pub struct Ast<P: ::lichen_structure::plugin::Project> {
    pub impl_: ::lichen_core::ast::AstImpl<P>,
}
impl<P: ::lichen_structure::plugin::Project> ::lichen_core::plugin::principal_traits::Ast<P>
    for self::Ast<P>
{
    const PROPERTIES_COUNT: usize = 2;
    fn impl_(&self) -> &::lichen_core::ast::AstImpl<P> {
        &self.impl_
    }
    fn impl_mut(&mut self) -> &mut ::lichen_core::ast::AstImpl<P> {
        &mut self.impl_
    }
}
