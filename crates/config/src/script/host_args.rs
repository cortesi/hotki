//! Argument decoding for Luau host functions.

use std::vec::IntoIter;

use ruau::vm::{
    FromLua, Function, MultiValue, RuntimeError, Scope, ScopedValue, Table,
    serde::from_scoped_value,
};
use serde::Deserialize;

/// Decode one host call's Luau argument list.
pub(super) struct HostArgs<'s> {
    /// Remaining argument values.
    values: IntoIter<ScopedValue<'s>>,
}

impl<'s> HostArgs<'s> {
    /// Start decoding a plain function call.
    pub(super) fn new(args: MultiValue<'s>) -> Self {
        Self {
            values: args.into_vec().into_iter(),
        }
    }

    /// Start decoding a method call, dropping Luau's explicit receiver value.
    pub(super) fn method(args: MultiValue<'s>) -> Self {
        let mut args = Self::new(args);
        let _receiver = args.values.next();
        args
    }

    /// Decode a required value with the standard "`context` is required" error.
    pub(super) fn required(&mut self, context: &str) -> Result<ScopedValue<'s>, RuntimeError> {
        self.values
            .next()
            .ok_or_else(|| RuntimeError::runtime(format!("{context} is required")))
    }

    /// Decode a required value with a custom missing-argument error.
    pub(super) fn required_with_message(
        &mut self,
        message: &str,
    ) -> Result<ScopedValue<'s>, RuntimeError> {
        self.values
            .next()
            .ok_or_else(|| RuntimeError::runtime(message.to_owned()))
    }

    /// Decode an optional value, treating absence as nil.
    pub(super) fn optional(&mut self) -> ScopedValue<'s> {
        self.values.next().unwrap_or(ScopedValue::Nil)
    }

    /// Reject unexpected trailing arguments.
    pub(super) fn finish(&mut self, context: &str) -> Result<(), RuntimeError> {
        if self.values.next().is_some() {
            return Err(RuntimeError::runtime(format!(
                "{context} got too many arguments"
            )));
        }
        Ok(())
    }

    /// Decode a required string argument.
    pub(super) fn string(
        &mut self,
        scope: &Scope<'s>,
        context: &str,
    ) -> Result<String, RuntimeError> {
        String::from_lua(self.required(context)?, scope)
    }

    /// Decode a required argument through ruau's `FromLua` bridge.
    pub(super) fn lua<T>(&mut self, scope: &Scope<'s>, context: &str) -> Result<T, RuntimeError>
    where
        T: FromLua<'s>,
    {
        T::from_lua(self.required(context)?, scope)
    }

    /// Decode a required argument through the serde bridge.
    pub(super) fn serde<T>(&mut self, scope: &Scope<'s>, context: &str) -> Result<T, RuntimeError>
    where
        T: for<'de> Deserialize<'de>,
    {
        from_scoped_value(scope, self.required(context)?)
            .map_err(|err| RuntimeError::runtime(err.message()))
    }

    /// Decode a required function argument.
    pub(super) fn function(&mut self, context: &str) -> Result<Function<'s>, RuntimeError> {
        expect_function_value(self.required(context)?, context)
    }

    /// Decode a required table argument.
    pub(super) fn table(&mut self, context: &str) -> Result<Table<'s>, RuntimeError> {
        match self.required(context)? {
            ScopedValue::Table(table) => Ok(table),
            other => Err(RuntimeError::runtime(format!(
                "{context} must be a table, got {}",
                other.type_name()
            ))),
        }
    }
}

/// Decode a scoped value that must be a function.
pub(super) fn expect_function_value<'s>(
    value: ScopedValue<'s>,
    context: &str,
) -> Result<Function<'s>, RuntimeError> {
    match value {
        ScopedValue::Function(func) => Ok(func),
        other => Err(RuntimeError::runtime(format!(
            "{context} must be a function, got {}",
            other.type_name()
        ))),
    }
}

/// Require exactly one returned Luau value.
pub(super) fn single_return<'s>(
    values: MultiValue<'s>,
    context: &str,
) -> Result<ScopedValue<'s>, RuntimeError> {
    let mut values = HostArgs::new(values);
    let value = values.optional();
    values.finish(context)?;
    Ok(value)
}
