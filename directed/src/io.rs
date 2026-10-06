//! The port buffer passed to a stage. This is the entire surface a generated
//! stage touches: read inputs by index, write outputs by index.

use std::sync::Arc;

use crate::error::CallError;
use crate::signature::Signature;
use crate::value::Value;

/// Owns the input values for one evaluation and collects the outputs.
pub struct Io {
    signature: &'static Signature,
    inputs: Vec<Option<Value>>,
    outputs: Vec<Option<Value>>,
}

impl Io {
    pub fn new(signature: &'static Signature) -> Self {
        Self {
            inputs: vec![None; signature.inputs.len()],
            outputs: vec![None; signature.outputs.len()],
            signature,
        }
    }

    fn input_name(&self, index: usize) -> &'static str {
        self.signature
            .inputs
            .get(index)
            .map(|p| p.name)
            .unwrap_or("<unknown>")
    }

    /// Borrow an input as `&T`. Used for `&T` parameters.
    pub fn get<T: Send + Sync + 'static>(&self, index: usize) -> Result<&T, CallError> {
        let name = self.input_name(index);
        let value = self
            .inputs
            .get(index)
            .and_then(|slot| slot.as_ref())
            .ok_or(CallError::MissingInput { index, name })?;
        value
            .downcast_ref::<T>()
            .ok_or(CallError::InputTypeMismatch {
                index,
                name,
                expected: std::any::type_name::<T>(),
            })
    }

    /// Move an input out as `T`. Fails if the value is shared.
    pub fn take<T: Send + Sync + 'static>(&mut self, index: usize) -> Result<T, CallError> {
        let name = self.input_name(index);
        let value = self
            .inputs
            .get_mut(index)
            .and_then(|slot| slot.take())
            .ok_or(CallError::MissingInput { index, name })?;
        let arc = Arc::downcast::<T>(value).map_err(|_| CallError::InputTypeMismatch {
            index,
            name,
            expected: std::any::type_name::<T>(),
        })?;
        Arc::try_unwrap(arc).map_err(|_| CallError::SharedInput {
            index,
            name,
            expected: std::any::type_name::<T>(),
        })
    }

    /// Take an input as `T`, cloning the inner value if it is shared.
    pub fn take_cloned<T: Clone + Send + Sync + 'static>(
        &mut self,
        index: usize,
    ) -> Result<T, CallError> {
        let name = self.input_name(index);
        let value = self
            .inputs
            .get_mut(index)
            .and_then(|slot| slot.take())
            .ok_or(CallError::MissingInput { index, name })?;
        let arc = Arc::downcast::<T>(value).map_err(|_| CallError::InputTypeMismatch {
            index,
            name,
            expected: std::any::type_name::<T>(),
        })?;
        Ok(match Arc::try_unwrap(arc) {
            Ok(value) => value,
            Err(shared) => (*shared).clone(),
        })
    }

    /// Write a single output by index.
    pub fn set<T: Send + Sync + 'static>(&mut self, index: usize, value: T) {
        self.outputs[index] = Some(Arc::new(value));
    }

    pub(crate) fn set_input(&mut self, index: usize, value: Value) {
        self.inputs[index] = Some(value);
    }

    pub(crate) fn into_outputs(self) -> Vec<Option<Value>> {
        self.outputs
    }
}
