use std::borrow::Cow;
use std::cell::UnsafeCell;
use std::cmp::Ordering;
use std::marker::PhantomPinned;
use std::mem::{self, MaybeUninit};
use std::pin::Pin;
use std::ptr;
use std::rc::Rc;

use crate::bytecode::Opr24;
use crate::common::ErrorKind;

/// The type of a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Type {
   Nil,
   Boolean,
   Number,
   String,
   Function,
}

impl std::fmt::Display for Type {
   fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
      write!(f, "{self:?}")
   }
}

/// A dynamically-typed value.
#[derive(Clone)]
pub enum Value {
   /// Nil denotes the lack of a value.
   Nil,
   /// The false boolean.
   False,
   /// The true boolean.
   True,
   /// A double-precision floating point number.
   Number(f64),
   /// A string.
   String(Rc<str>),
   /// A function.
   Function(Rc<Closure>),
}

impl Value {
   /// Returns the type of the value.
   pub fn typ(&self) -> Type {
      match self {
         Value::Nil => Type::Nil,
         Value::False | Value::True => Type::Boolean,
         Value::Number(_) => Type::Number,
         Value::String(_) => Type::String,
         Value::Function(_) => Type::Function,
      }
   }

   fn type_error(&self, expected: &'static str) -> ErrorKind {
      ErrorKind::TypeError {
         expected: Cow::from(expected),
         got: Cow::from(self.typ().to_string()),
      }
   }

   /// Ensures the value is a `Nil`, returning a type mismatch error if that's not the case.
   pub fn nil(&self) -> Result<(), ErrorKind> {
      if let Value::Nil = self {
         Ok(())
      } else {
         Err(self.type_error("Nil"))
      }
   }

   /// Ensures the value is a `Boolean`, returning a type mismatch error if that's not the case.
   pub fn boolean(&self) -> Result<bool, ErrorKind> {
      match self {
         Value::False => Ok(false),
         Value::True => Ok(true),
         _ => Err(self.type_error("Boolean")),
      }
   }

   /// Ensures the value is a `Number`, returning a type mismatch error if that's not the case.
   pub fn number(&self) -> Result<f64, ErrorKind> {
      if let &Value::Number(x) = self {
         Ok(x)
      } else {
         Err(self.type_error("Number"))
      }
   }

   /// Ensures the value is a `String`, returning a type mismatch error if that's not the case.
   pub fn string(&self) -> Result<&str, ErrorKind> {
      if let Value::String(s) = self {
         Ok(s)
      } else {
         Err(self.type_error("String"))
      }
   }

   /// Ensures the value is a `Function`, returning a type mismatch error if that's not the case.
   pub fn function(&self) -> Result<&Rc<Closure>, ErrorKind> {
      if let Value::Function(c) = self {
         Ok(c)
      } else {
         Err(self.type_error("Function"))
      }
   }

   /// Returns whether the value is truthy. All values except `Nil` and `False` are truthy.
   pub fn is_truthy(&self) -> bool {
      !matches!(self, Value::Nil | Value::False)
   }

   /// Returns whether the values is falsy. The only falsy values are `Nil` and `False`.
   pub fn is_falsy(&self) -> bool {
      !self.is_truthy()
   }

   /// Attempts to partially compare this value with another one.
   ///
   /// Returns an error if the types of the two values are not the same.
   pub fn try_partial_cmp(&self, other: &Self) -> Result<Option<Ordering>, ErrorKind> {
      if self.typ() != other.typ() {
         Err(ErrorKind::TypeError {
            expected: self.typ().to_string().into(),
            got: other.typ().to_string().into(),
         })
      } else {
         match self {
            Self::Nil => Ok(Some(Ordering::Equal)),
            Self::False | Self::True => {
               Ok(Some(self.boolean().unwrap().cmp(&other.boolean().unwrap())))
            }
            Self::Number(x) => Ok(x.partial_cmp(&other.number().unwrap())),
            Self::String(s) => {
               if let Value::String(t) = &other {
                  Ok(s.partial_cmp(t))
               } else {
                  unreachable!()
               }
            }
            Self::Function(_) => Ok(None),
         }
      }
   }
}

impl Default for Value {
   fn default() -> Self {
      Self::Nil
   }
}

impl From<bool> for Value {
   fn from(b: bool) -> Self {
      match b {
         false => Self::False,
         true => Self::True,
      }
   }
}

impl std::fmt::Debug for Value {
   fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
      match self {
         Value::Nil => f.write_str("nil"),
         Value::False => f.write_str("false"),
         Value::True => f.write_str("true"),
         Value::Number(x) => write!(f, "{x}"),
         Value::String(s) => write!(f, "{s:?}"),
         Value::Function(_) => write!(f, "<func>"),
      }
   }
}

impl std::fmt::Display for Value {
   fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
      match self {
         Value::Nil => f.write_str("nil"),
         Value::False => f.write_str("false"),
         Value::True => f.write_str("true"),
         Value::Number(x) => write!(f, "{x}"),
         Value::String(s) => write!(f, "{s}"),
         Value::Function(_) => write!(f, "<func>"),
      }
   }
}

impl PartialEq for Value {
   fn eq(&self, other: &Self) -> bool {
      match (self, other) {
         (Self::Number(l), Self::Number(r)) => l == r,
         (Self::String(l), Self::String(r)) => l == r,
         (Self::Function(l), Self::Function(r)) => Rc::ptr_eq(l, r),
         _ => core::mem::discriminant(self) == core::mem::discriminant(other),
      }
   }
}

/// An upvalue captured by a closure.
#[derive(Debug)]
pub struct Upvalue {
   /// A writable pointer to the variable captured by this upvalue.
   pub(crate) ptr: UnsafeCell<ptr::NonNull<Value>>,
   /// Storage for a closed upvalue.
   closed: UnsafeCell<MaybeUninit<Value>>,

   _pinned: PhantomPinned,
}

impl Upvalue {
   /// Creates a new upvalue pointing to a live variable.
   pub(crate) fn new(var: ptr::NonNull<Value>) -> Pin<Rc<Upvalue>> {
      Rc::pin(Upvalue {
         ptr: UnsafeCell::new(var),
         closed: UnsafeCell::new(MaybeUninit::uninit()),
         _pinned: PhantomPinned,
      })
   }

   /// Closes an upvalue by `mem::take`ing the value behind the `ptr` into the `closed` field, and
   /// updating the `ptr` field to point to the `closed` field's contents.
   ///
   /// # Safety
   /// The caller must ensure there are no mutable references to the variable at the time of
   /// calling this.
   pub(crate) unsafe fn close(&self) {
      let ptr = &mut *self.ptr.get();
      let closed = &mut *self.closed.get();
      let value = mem::take(ptr.as_mut());
      *ptr = ptr::NonNull::new(closed.write(value) as *mut _).unwrap();
   }

   /// Returns the value pointed to by this upvalue.
   ///
   /// # Safety
   /// The caller must ensure there are no mutable references to the source variable at the time
   /// of calling this.
   pub(crate) unsafe fn get(&self) -> &Value {
      (*self.ptr.get()).as_ref()
   }

   /// Writes to the variable pointed to by this upvalue.
   ///
   /// # Safety
   /// The caller must ensure there are no mutable references to the source variable at the time
   /// of calling this.
   pub(crate) unsafe fn set(&self, value: Value) {
      *(*self.ptr.get()).as_ptr() = value;
   }
}

/// The runtime representation of a function.
#[derive(Debug)]
pub struct Closure {
   pub function_id: Opr24,
   pub captures: Vec<Pin<Rc<Upvalue>>>,
}
