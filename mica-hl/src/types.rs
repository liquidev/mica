use std::marker::PhantomData;
use std::rc::Rc;

use mica_language::bytecode::{
   DispatchTable, Environment, Function, FunctionKind, FunctionSignature,
};
use mica_language::value::{Closure, Struct, Value};

use crate::{ffvariants, Error, ForeignFunction, RawForeignFunction};

/// A descriptor for a dispatch table. Defines which methods are available on the table, as well
/// as their implementations.
#[derive(Default)]
pub(crate) struct DispatchTableDescriptor {
   methods: Vec<(FunctionSignature, RawForeignFunction)>,
}

impl DispatchTableDescriptor {
   /// Builds a dispatch table from this descriptor.
   pub(crate) fn build_dtable(
      self,
      // The rc is passed by reference to prevent an unnecessary clone.
      mut dtable: DispatchTable,
      env: &mut Environment,
   ) -> Result<DispatchTable, Error> {
      for (signature, f) in self.methods {
         let function_id = env
            .create_function(Function {
               name: Rc::from(format!("{}.{}", &dtable.pretty_name, signature.name)),
               parameter_count: signature.arity,
               kind: FunctionKind::Foreign(f),
            })
            .map_err(|_| Error::TooManyFunctions)?;
         let index = env.get_method_index(&signature).map_err(|_| Error::TooManyMethods)?;
         dtable.set_method(
            index,
            Rc::new(Closure {
               function_id,
               captures: Vec::new(),
            }),
         );
      }
      Ok(dtable)
   }
}

/// A builder that allows for binding APIs with user-defined types.
pub struct TypeBuilder<T>
where
   T: ?Sized,
{
   type_name: Rc<str>,
   type_dtable: DispatchTableDescriptor,
   instance_dtable: DispatchTableDescriptor,
   _data: PhantomData<T>,
}

impl<T> TypeBuilder<T>
where
   T: ?Sized,
{
   /// Creates a new `TypeBuilder`.
   pub fn new(type_name: impl Into<Rc<str>>) -> Self {
      let type_name = type_name.into();
      Self {
         type_dtable: Default::default(),
         instance_dtable: Default::default(),
         type_name,
         _data: PhantomData,
      }
   }

   /// Adds a _raw_ instance function to the type.
   ///
   /// You should generally prefer [`add_function`][`Self::add_function`] instead of this.
   ///
   /// `parameter_count` should reflect the parameter count of the function. Pass `None` if the
   /// function accepts a variable number of arguments. Note that _unlike with bare raw functions_
   /// there can be two functions with the same name defined on a type, as long as they have
   /// different arities. Functions with specific arities take priority over varargs.
   ///
   /// Note that this function _consumes_ the builder; this is because calls to functions that add
   /// into the type are meant to be chained together in one expression.
   pub fn add_raw_function(
      mut self,
      name: &str,
      parameter_count: Option<u16>,
      f: RawForeignFunction,
   ) -> Self {
      self.instance_dtable.methods.push((
         FunctionSignature {
            name: Rc::from(name),
            arity: parameter_count,
         },
         f,
      ));
      self
   }

   /// Adds a _raw_ static function to the type.
   ///
   /// You should generally prefer [`add_static`][`Self::add_static`] instead of this.
   ///
   /// `parameter_count` should reflect the parameter count of the function. Pass `None` if the
   /// function accepts a variable number of arguments. Note that _unlike with bare raw functions_
   /// there can be two functions with the same name defined on a type, as long as they have
   /// different arities. Functions with specific arities take priority over varargs.
   ///
   /// Note that this function _consumes_ the builder; this is because calls to functions that add
   /// into the type are meant to be chained together in one expression.
   pub fn add_raw_static(
      mut self,
      name: &str,
      parameter_count: Option<u16>,
      f: RawForeignFunction,
   ) -> Self {
      self.type_dtable.methods.push((
         FunctionSignature {
            name: Rc::from(name),
            arity: parameter_count,
         },
         f,
      ));
      self
   }

   /// Adds an instance function to the struct.
   ///
   /// The function must follow the "method" calling convention, in that it accepts `&`[`T`] or
   /// `&mut `[`T`] as its first parameter.
   pub fn add_function<F, V>(self, name: &str, f: F) -> Self
   where
      V: ffvariants::Method<T>,
      F: ForeignFunction<V>,
   {
      self.add_raw_function(name, f.parameter_count(), f.into_raw_foreign_function())
   }

   /// Adds a static function to the struct.
   ///
   /// The function must follow the "bare" calling convention, in that it doesn't accept a reference
   /// to `T` as its first parameter.
   pub fn add_static<F, V>(self, name: &str, f: F) -> Self
   where
      V: ffvariants::Bare,
      F: ForeignFunction<V>,
   {
      self.add_raw_static(
         name,
         f.parameter_count().map(|x| {
            // Add 1 for the static receiver, which isn't counted into the bare function's
            // signature.
            x + 1
         }),
         f.into_raw_foreign_function(),
      )
   }

   /// Builds the struct builder into its type dtable and instance dtable, respectively.
   pub(crate) fn build(self, env: &mut Environment) -> Result<BuiltType, Error> {
      let mut type_dtable = Rc::new(
         self
            .type_dtable
            .build_dtable(DispatchTable::new_for_type(Rc::clone(&self.type_name)), env)?,
      );
      let instance_dtable = Rc::new(self.instance_dtable.build_dtable(
         DispatchTable::new_for_instance(Rc::clone(&self.type_name)),
         env,
      )?);
      Rc::get_mut(&mut type_dtable).unwrap().instance = Some(Rc::clone(&instance_dtable));
      Ok(BuiltType {
         type_dtable,
         instance_dtable,
         type_name: self.type_name,
      })
   }
}

/// Dispatch tables for an finished type.
pub(crate) struct BuiltType {
   pub(crate) type_name: Rc<str>,
   pub(crate) type_dtable: Rc<DispatchTable>,
   pub(crate) instance_dtable: Rc<DispatchTable>,
}

impl BuiltType {
   /// Makes a struct value from the built type.
   pub(crate) fn make_type_struct(&self) -> Value {
      Value::Struct(Rc::new(Struct::new_type(Rc::clone(&self.type_dtable))))
   }
}
