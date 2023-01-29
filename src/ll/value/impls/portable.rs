//! Portable implementation of values. Uses a regular `enum`, which isn't very cache efficient,
//! but is supported on most platforms.

use std::{hint::unreachable_unchecked, mem, ops::Deref};

use crate::ll::{
    gc::GcRaw,
    value::{Closure, Struct, Trait, UserData, ValueCommon, ValueKind},
};

/// A portable implementation of values.
#[derive(Clone, Copy)]
pub(crate) enum ValueImpl {
    /// Nil denotes the lack of a value.
    Nil,
    /// The false boolean.
    False,
    /// The true boolean.
    True,
    /// A double-precision floating point number.
    Number(f64),
    /// A string.
    String(GcRaw<String>),
    /// A function.
    Function(GcRaw<Closure>),
    /// A struct.
    Struct(GcRaw<Struct>),
    /// A trait.
    Trait(GcRaw<Trait>),
    /// Dynamically-typed user data.
    UserData(GcRaw<Box<dyn UserData>>),
}

impl ValueCommon for ValueImpl {
    fn new_nil() -> Self {
        Self::Nil
    }

    fn new_boolean(b: bool) -> Self {
        match b {
            true => Self::True,
            false => Self::False,
        }
    }

    fn new_number(n: f64) -> Self {
        Self::Number(n)
    }

    fn new_string(s: GcRaw<String>) -> Self {
        Self::String(s)
    }

    fn new_function(f: GcRaw<Closure>) -> Self {
        Self::Function(f)
    }

    fn new_struct(s: GcRaw<Struct>) -> Self {
        Self::Struct(s)
    }

    fn new_trait(t: GcRaw<Trait>) -> Self {
        Self::Trait(t)
    }

    fn new_user_data(u: GcRaw<Box<dyn UserData>>) -> Self {
        Self::UserData(u)
    }

    fn kind(&self) -> ValueKind {
        match self {
            ValueImpl::Nil => ValueKind::Nil,
            ValueImpl::False | ValueImpl::True => ValueKind::Boolean,
            ValueImpl::Number(_) => ValueKind::Number,
            ValueImpl::String(_) => ValueKind::String,
            ValueImpl::Function(_) => ValueKind::Function,
            ValueImpl::Struct(_) => ValueKind::Struct,
            ValueImpl::Trait(_) => ValueKind::Trait,
            ValueImpl::UserData(_) => ValueKind::UserData,
        }
    }

    unsafe fn get_boolean_unchecked(&self) -> bool {
        match self {
            Self::True => true,
            Self::False => false,
            _ => unreachable_unchecked(),
        }
    }

    unsafe fn get_number_unchecked(&self) -> &f64 {
        if let Self::Number(x) = self {
            x
        } else {
            unreachable_unchecked()
        }
    }

    unsafe fn get_raw_string_unchecked(&self) -> GcRaw<String> {
        if let Self::String(s) = self {
            *s
        } else {
            unreachable_unchecked()
        }
    }

    unsafe fn get_raw_function_unchecked(&self) -> GcRaw<Closure> {
        if let Self::Function(f) = self {
            *f
        } else {
            unreachable_unchecked()
        }
    }

    unsafe fn get_raw_struct_unchecked(&self) -> GcRaw<Struct> {
        if let Self::Struct(s) = self {
            *s
        } else {
            unreachable_unchecked()
        }
    }

    unsafe fn get_raw_trait_unchecked(&self) -> GcRaw<Trait> {
        if let Self::Trait(t) = self {
            *t
        } else {
            unreachable_unchecked()
        }
    }

    unsafe fn get_raw_user_data_unchecked(&self) -> GcRaw<Box<dyn UserData>> {
        if let Self::UserData(u) = self {
            *u
        } else {
            unreachable_unchecked()
        }
    }
}

impl PartialEq for ValueImpl {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Number(l), Self::Number(r)) => l == r,
            (Self::String(l), Self::String(r)) => unsafe { l.get() == r.get() },
            (Self::Function(l), Self::Function(r)) => l == r,
            (Self::Struct(l), Self::Struct(r)) => l == r,
            (Self::Trait(l), Self::Trait(r)) => l == r,
            (Self::UserData(l), Self::UserData(r)) => unsafe {
                l.get().partial_eq(r.get().deref())
            },
            _ => mem::discriminant(self) == mem::discriminant(other),
        }
    }
}
