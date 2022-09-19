use std::rc::Rc;

use mica_language::bytecode::{Environment, Opr24};
use mica_language::codegen;
use mica_language::gc::{Gc, Memory};
use mica_language::value::create_trait;

use crate::{Error, Hidden, LanguageErrorKind, MethodId, Value};

/// Allows you to build traits programatically from Rust code.
pub struct TraitBuilder<'e> {
   pub(crate) inner: codegen::TraitBuilder<'e>,
   pub(crate) gc: &'e mut Memory,
}

impl<'e> TraitBuilder<'e> {
   /// Adds a new function requirement into the trait and returns its method ID, which can be used
   /// to call the function on values implementing the trait.
   pub fn add_function(&mut self, name: &str, arity: u16) -> Result<MethodId, Error> {
      self.inner.add_method(Rc::from(name), arity).map(MethodId).map_err(|e| match e {
         LanguageErrorKind::TooManyTraits => Error::TooManyTraits,
         LanguageErrorKind::TooManyFunctions => Error::TooManyFunctions,
         LanguageErrorKind::TooManyMethods => Error::TooManyMethods,
         LanguageErrorKind::TooManyParameters => Error::TooManyParametersInTraitMethod,
         _ => unreachable!(),
      })
   }

   /// Finishes building the trait and wraps it into a value.
   pub fn build(self) -> Value {
      let (trait_id, env) = self.inner.build();
      create_trait_value(env, self.gc, trait_id)
   }
}

pub(crate) fn create_trait_value(env: &mut Environment, gc: &mut Memory, trait_id: Opr24) -> Value {
   let instance = create_trait(env, gc, trait_id);
   let instance = unsafe { Gc::from_raw(instance) };
   Value::Trait(Hidden(instance))
}
