use std::{any::Any, ops::Deref, rc::Rc};

/// The implementation of a raw foreign function.
pub use crate::ll::bytecode::ForeignFunction as RawForeignFunction;
/// The kind of a raw function.
pub use crate::ll::bytecode::FunctionKind as RawFunctionKind;
use crate::{
    corelib, create_trait_value, ffvariants,
    ll::{
        ast::DumpAst,
        bytecode,
        bytecode::{
            BuiltinDispatchTables, BuiltinTraits, Chunk, DispatchTable, Environment, Function,
            FunctionKind, Opcode, Opr24,
        },
        codegen::{self, CodeGenerator},
        gc::{Gc, Memory},
        lexer::Lexer,
        parser::Parser,
        value::{Closure, RawValue},
        vm::{self, Globals},
    },
    BuiltType, CoreLibrary, Error, Fiber, ForeignFunction, GlobalIndex, MethodIndex, MicaResultExt,
    TraitBuilder, TryFromValue, TypeBuilder, UserData, Value,
};

/// Options for debugging the language implementation.
#[derive(Debug, Clone, Copy, Default)]
pub struct DebugOptions {
    /// Set to `true` to print the AST to stdout after parsing.
    pub dump_ast: bool,
    /// Set to `true` to print the bytecode to stdout after successful compilation.
    pub dump_bytecode: bool,
}

/// **Start here!** An execution engine. Contains information about things like globals, registered
/// types, etc.
pub struct Engine {
    pub(crate) env: Environment,
    pub(crate) builtin_traits: BuiltinTraits,
    pub(crate) globals: Globals,
    // This field is needed to keep all builtin dispatch tables alive for longer than `gc`.
    pub(crate) gc: Memory,
    debug_options: DebugOptions,
}

impl Engine {
    /// Creates a new engine using the [default core library][corelib].
    pub fn new() -> Self {
        Self::with_corelib(corelib::Lib)
    }

    /// Creates a new engine with an alternative core library.
    pub fn with_corelib(corelib: impl CoreLibrary) -> Self {
        Self::with_debug_options(corelib, Default::default())
    }

    /// Creates a new engine with specific debug options.
    ///
    /// [`Engine::new`] creates an engine with [`Default`] debug options, and should generally be
    /// preferred unless you're debugging the language's internals.
    ///
    /// Constructing the engine can fail if the standard library defines way too many methods.
    pub fn with_debug_options(mut corelib: impl CoreLibrary, debug_options: DebugOptions) -> Self {
        let mut gc = Memory::new();
        // This is a little bad because it allocates a bunch of empty dtables only to discard them.
        let mut env = Environment::new(BuiltinDispatchTables::empty());

        let builtin_traits = BuiltinTraits::register_in(&mut env);
        let iterator = create_trait_value(&mut env, &mut gc, builtin_traits.iterator);

        macro_rules! get_dtables {
            ($type_name:tt, $define:tt) => {{
                let tb = TypeBuilder::new($type_name);
                let tb = corelib.$define(tb);
                tb.build(&mut env, &mut gc, &builtin_traits)
                    .expect("corelib declares too many methods")
            }};
        }
        let nil = get_dtables!("Nil", define_nil);
        let boolean = get_dtables!("Boolean", define_boolean);
        let number = get_dtables!("Number", define_number);
        let string = get_dtables!("String", define_string);
        let list = get_dtables!("List", define_list);
        let dict = get_dtables!("Dict", define_dict);
        env.builtin_dtables = BuiltinDispatchTables {
            nil: Gc::clone(&nil.instance_dtable),
            boolean: Gc::clone(&boolean.instance_dtable),
            number: Gc::clone(&number.instance_dtable),
            string: Gc::clone(&string.instance_dtable),
            function: Gc::new(DispatchTable::new_for_instance("Function")),
            list: Gc::clone(&list.instance_dtable),
            dict: Gc::clone(&dict.instance_dtable),
        };

        let mut engine = Self { env, builtin_traits, globals: Globals::new(), gc, debug_options };
        // Unwrapping here is fine because at this point we haven't got quite that many globals
        // registered to overflow an Opr24.
        engine.set_built_type(&nil).unwrap();
        engine.set_built_type(&boolean).unwrap();
        engine.set_built_type(&number).unwrap();
        engine.set_built_type(&string).unwrap();
        engine.set_built_type(&list).unwrap();
        engine.set("Iterator", iterator).unwrap();

        corelib.load(&mut engine).expect("corelib failed to load (in CoreLibrary::load)");

        engine
    }

    /// Compiles a script.
    ///
    /// # Errors
    ///  - [`Error::Compile`] - Syntax or semantic error
    pub fn compile(
        &mut self,
        filename: impl AsRef<str>,
        source: impl Into<String>,
    ) -> Result<Script, Error> {
        let module_name = Rc::from(filename.as_ref());
        let lexer = Lexer::new(Rc::clone(&module_name), source.into());
        let (ast, root_node) = Parser::new(lexer).parse()?;
        if self.debug_options.dump_ast {
            eprintln!("Mica - AST dump:");
            eprintln!("{:?}", DumpAst(&ast, root_node));
        }

        let main_chunk = CodeGenerator::new(module_name, &mut self.env, &self.builtin_traits)
            .generate(&ast, root_node)?;
        if self.debug_options.dump_bytecode {
            eprintln!("Mica - global environment:");
            eprintln!("{:#?}", self.env);
            eprintln!("Mica - main chunk disassembly:");
            eprintln!("{:#?}", main_chunk);
        }

        Ok(Script { engine: self, main_chunk })
    }

    /// Compiles and starts running a script.
    ///
    /// This can be used as a shorthand if you don't intend to reuse the compiled bytecode.
    ///
    /// # Errors
    /// See [`compile`][`Self::compile`].
    pub fn start(
        &mut self,
        filename: impl AsRef<str>,
        source: impl Into<String>,
    ) -> Result<Fiber, Error> {
        let script = self.compile(filename, source)?;
        Ok(script.into_fiber())
    }

    /// Calls the provided function with the given arguments.
    ///
    /// # Errors
    ///
    /// - [`Error::Runtime`] - if a runtime error occurs - `function` isn't callable or an error is
    ///   raised during execution
    /// - [`Error::TooManyArguments`] - if more arguments than the implementation can support is
    ///   passed to the function
    pub fn call<T>(
        &mut self,
        function: Value,
        arguments: impl IntoIterator<Item = Value>,
    ) -> Result<T, Error>
    where
        T: TryFromValue,
    {
        let stack: Vec<_> =
            Some(function).into_iter().chain(arguments).map(|x| x.to_raw(&mut self.gc)).collect();
        // Having to construct a chunk here isn't the most clean, but it's the simplest way of
        // making the VM perform a function call. It reuses sanity checks such as ensuring
        // `function` can actually be called.
        let mut chunk = Chunk::new(Rc::from("(call)"));
        chunk.emit((
            Opcode::Call,
            // 1 has to be subtracted from the stack length there because the VM itself adds 1 to
            // count in the function argument.
            Opr24::try_from(stack.len() - 1).map_err(|_| Error::TooManyArguments)?,
        ));
        chunk.emit(Opcode::Halt);
        let chunk = Rc::new(chunk);
        let fiber = Fiber { engine: self, inner: vm::Fiber::new(chunk, stack) };
        fiber.trampoline()
    }

    /// Returns the unique ID of a method with a given name and arity.
    ///
    /// Note that there can only exist about 65 thousand unique method signatures. This is usually
    /// not a problem as method names often repeat. Also note that unlike functions, a method can
    /// only accept up to 256 arguments. Which, to be quite frankly honest, should be enough for
    /// anyone.
    ///
    /// # Errors
    ///
    /// - [`Error::TooManyMethods`] - raised when too many unique method signatures exist at once
    pub fn method_id(&mut self, signature: impl MethodSignature) -> Result<MethodIndex, Error> {
        signature.to_method_id(&mut self.env)
    }

    /// Calls a method on a receiver with the given arguments.
    ///
    /// Note that if you're calling a method often, it's cheaper to precompute the method signature
    /// into a [`MethodIndex`] by using the [`method_id`][`Self::method_id`] function, compared to
    /// passing a name+arity pair every single time.
    ///
    /// # Errors
    ///
    /// - [`Error::Runtime`] - if a runtime error occurs - `function` isn't callable or an error is
    ///   raised during execution
    /// - [`Error::TooManyArguments`] - if more arguments than the implementation can support is
    ///   passed to the function
    /// - [`Error::TooManyMethods`] - if too many methods with different signatures exist at the
    ///   same time
    /// - [`Error::ArgumentCount`] - if `arguments.count()` does not match the argument count of the
    ///   signature
    pub fn call_method<T>(
        &mut self,
        receiver: Value,
        signature: impl MethodSignature,
        arguments: impl IntoIterator<Item = Value>,
    ) -> Result<T, Error>
    where
        T: TryFromValue,
    {
        let method_id = signature.to_method_id(&mut self.env)?;
        // Unwrapping here is fine because `to_method_id` ensures that a method with a given ID
        // exists.
        let signature = self.env.get_method_signature(method_id).unwrap();
        let stack: Vec<_> =
            Some(receiver).into_iter().chain(arguments).map(|x| x.to_raw(&mut self.gc)).collect();
        let argument_count = u8::try_from(stack.len()).map_err(|_| Error::TooManyArguments)?;
        if Some(argument_count as u16) != signature.arity {
            return Err(Error::ArgumentCount {
                // Unwrapping here is fine because signatures of methods created by `to_method_id`
                // always have a static arity.
                expected: signature.arity.unwrap() as usize - 1,
                got: argument_count as usize - 1,
            });
        }
        let mut chunk = Chunk::new(Rc::from("(call)"));
        chunk.emit((Opcode::CallMethod, Opr24::pack((method_id.to_u16(), argument_count))));
        chunk.emit(Opcode::Halt);
        let chunk = Rc::new(chunk);
        let fiber = Fiber { engine: self, inner: vm::Fiber::new(chunk, stack) };
        fiber.trampoline()
    }

    /// Returns the unique global ID for the global with the given name, or an error if there
    /// are too many globals in scope.
    ///
    /// The maximum amount of globals is about 16 million, so you shouldn't worry too much about
    /// hitting that limit unless you're stress-testing the VM or accepting untrusted input as
    /// globals.
    ///
    /// # Errors
    ///  - [`Error::TooManyGlobals`] - Too many globals with unique names were created
    pub fn global_id(&mut self, name: impl GlobalName) -> Result<GlobalIndex, Error> {
        name.to_global_id(&mut self.env)
    }

    /// Sets a global variable that'll be available to scripts executed by the engine.
    ///
    /// The `id` parameter can be either an `&str` or a prefetched [`global_id`][`Self::global_id`].
    ///
    /// # Errors
    ///  - [`Error::TooManyGlobals`] - Too many globals with unique names were created
    pub fn set<G, T>(&mut self, id: G, value: T) -> Result<(), Error>
    where
        G: GlobalName,
        T: Into<Value>,
    {
        let id = id.to_global_id(&mut self.env)?;
        self.globals.set(id, value.into().to_raw(&mut self.gc));
        Ok(())
    }

    /// Returns the value of a global variable, or `nil` if it's not set.
    ///
    /// The `id` parameter can be either an `&str` or a prefetched [`global_id`][`Self::global_id`].
    ///
    /// # Errors
    ///  - [`Error::TooManyGlobals`] - Too many globals with unique names were created
    ///  - [`Error::TypeMismatch`] - The type of the value is not convertible to `T`
    pub fn get<G, T>(&self, id: G) -> Result<T, Error>
    where
        G: OptionalGlobalName,
        T: TryFromValue,
    {
        if let Some(id) = id.try_to_global_id(&self.env) {
            T::try_from_value(&Value::from_raw(self.globals.get(id)))
        } else {
            T::try_from_value(&Value::from_raw(RawValue::from(())))
        }
    }

    /// Declares a "raw" function in the global scope. Raw functions do not perform any type checks
    /// by default and accept a variable number of arguments.
    ///
    /// You should generally prefer [`add_function`][`Self::add_function`] instead of this.
    ///
    /// Note that this cannot accept [`GlobalId`]s, because a name is required to create the
    /// function and global IDs have their name erased.
    ///
    /// `parameter_count` should reflect the parameter count of the function. Pass `None` if the
    /// function accepts a variable number of arguments. Note that because this function omits type
    /// checks you may receive a different amount of arguments than specified.
    ///
    /// # Errors
    ///  - [`Error::TooManyGlobals`] - Too many globals with unique names were created
    ///  - [`Error::TooManyFunctions`] - Too many functions were registered into the engine
    pub fn add_raw_function(
        &mut self,
        name: &str,
        parameter_count: impl Into<Option<u16>>,
        f: FunctionKind,
    ) -> Result<(), Error> {
        let global_id = name.to_global_id(&mut self.env)?;
        let name = Rc::from(name);
        let function_id = self
            .env
            .create_function(Function {
                name: Rc::clone(&name),
                parameter_count: parameter_count.into(), // doesn't matter for non-methods
                kind: f,
                hidden_in_stack_traces: false,
            })
            .map_err(|_| Error::TooManyFunctions)?;
        let function =
            RawValue::from(self.gc.allocate(Closure { name, function_id, captures: Vec::new() }));
        self.globals.set(global_id, function);
        Ok(())
    }

    /// Declares a function in the global scope.
    ///
    /// # Errors
    /// See [`add_raw_function`][`Self::add_raw_function`].
    pub fn add_function<F, V>(&mut self, name: &str, f: F) -> Result<(), Error>
    where
        V: ffvariants::Bare,
        F: ForeignFunction<V>,
    {
        self.add_raw_function(
            name,
            F::parameter_count(),
            FunctionKind::Foreign(f.into_raw_foreign_function()),
        )
    }

    /// Declares a type in the global scope.
    ///
    /// # Errors
    ///  - [`Error::TooManyGlobals`] - Too many globals with unique names were created
    ///  - [`Error::TooManyFunctions`] - Too many functions were registered into the engine
    ///  - [`Error::TooManyMethods`] - Too many unique method signatures were created
    pub fn add_type<T>(&mut self, builder: TypeBuilder<T>) -> Result<(), Error>
    where
        T: Any + UserData,
    {
        let built = builder.build(&mut self.env, &mut self.gc, &self.builtin_traits)?;
        self.set_built_type(&built)?;
        Ok(())
    }

    pub(crate) fn set_built_type<T>(&mut self, typ: &BuiltType<T>) -> Result<(), Error>
    where
        T: Any,
    {
        let value = typ.make_type(&mut self.gc);
        self.set(typ.type_name.deref(), value)
    }

    /// Starts building a new trait.
    pub fn build_trait(&mut self, name: &str) -> Result<TraitBuilder<'_>, Error> {
        Ok(TraitBuilder {
            inner: codegen::TraitBuilder::new(&mut self.env, None, Rc::from(name)).mica()?,
            gc: &mut self.gc,
        })
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

/// A script pre-compiled into bytecode.
pub struct Script<'e> {
    engine: &'e mut Engine,
    main_chunk: Rc<Chunk>,
}

impl<'e> Script<'e> {
    /// Starts running a script in a new fiber.
    pub fn start(&mut self) -> Fiber {
        Fiber {
            engine: self.engine,
            inner: vm::Fiber::new(Rc::clone(&self.main_chunk), Vec::new()),
        }
    }

    /// Starts running a script in a new fiber, consuming the script.
    pub fn into_fiber(self) -> Fiber<'e> {
        Fiber {
            engine: self.engine,
            inner: vm::Fiber::new(Rc::clone(&self.main_chunk), Vec::new()),
        }
    }
}

mod global_id {
    use crate::GlobalIndex;

    pub trait Sealed {}
    impl Sealed for GlobalIndex {}
    impl Sealed for &str {}
}

/// A trait for names convertible to global IDs.
pub trait GlobalName: global_id::Sealed {
    #[doc(hidden)]
    fn to_global_id(&self, env: &mut Environment) -> Result<GlobalIndex, Error>;
}

impl GlobalName for GlobalIndex {
    fn to_global_id(&self, _: &mut Environment) -> Result<GlobalIndex, Error> {
        Ok(*self)
    }
}

impl GlobalName for &str {
    fn to_global_id(&self, env: &mut Environment) -> Result<GlobalIndex, Error> {
        Ok(if let Some(slot) = env.get_global(self) {
            slot
        } else {
            env.create_global(self).map_err(|_| Error::TooManyGlobals)?
        })
    }
}

/// A trait for names convertible to global IDs.
pub trait OptionalGlobalName {
    #[doc(hidden)]
    fn try_to_global_id(&self, env: &Environment) -> Option<GlobalIndex>;
}

impl OptionalGlobalName for GlobalIndex {
    fn try_to_global_id(&self, _: &Environment) -> Option<GlobalIndex> {
        Some(*self)
    }
}

impl OptionalGlobalName for &str {
    fn try_to_global_id(&self, env: &Environment) -> Option<GlobalIndex> {
        env.get_global(self)
    }
}

mod method_index {
    use crate::MethodIndex;

    pub trait Sealed {}

    impl Sealed for MethodIndex {}
    impl Sealed for (&str, u8) {}
}

/// Implemented by every type that can be used as a method signature.
///
/// See [`Engine::call_method`].
pub trait MethodSignature: method_index::Sealed {
    #[doc(hidden)]
    fn to_method_id(&self, env: &mut Environment) -> Result<MethodIndex, Error>;
}

impl MethodSignature for MethodIndex {
    fn to_method_id(&self, _: &mut Environment) -> Result<MethodIndex, Error> {
        Ok(*self)
    }
}

/// Tuples of string slices and `u8`s are a user-friendly representation of method signatures.
/// For instance, `("cat", 1)` represents the method `cat/1`.
impl MethodSignature for (&str, u8) {
    fn to_method_id(&self, env: &mut Environment) -> Result<MethodIndex, Error> {
        env.get_or_create_method_index(&bytecode::MethodSignature {
            name: Rc::from(self.0),
            arity: Some(self.1 as u16 + 1),
            trait_id: None,
        })
        .map_err(|_| Error::TooManyMethods)
    }
}
