//! Nodes contain all logic and state, but no information on order of execution.
use std::{
    any::{Any, TypeId},
    collections::HashMap,
    sync::Arc,
};

use crate::{
    InjectionError, RefType,
    stage::{EvalStrategy, ReevaluationRule, Stage},
    types::DataLabel,
};

/// Used for single-output functions
pub const UNNAMED_OUTPUT_NAME: DataLabel = DataLabel::new_const("_");

/// Every node wraps a Stage, which is a decorated function that has some
/// number of inputs and some number of outputs.
///
/// TODO: Explain the caching functionality in detail
#[derive(Debug)]
pub struct Node<S: Stage> {
    pub(super) stage: S,
    // Arbitrary state, by default will be ()
    pub(super) state: S::State,
    pub(super) inputs: Option<S::Input>,
    pub(super) outputs: Option<S::Output>,
    /// TODO: Simplify this!
    pub(super) cache: HashMap<u64, Vec<Cached<S::Input, S::Output>>>,
    pub(super) input_changed: bool,
}

/// This represents a cached input/output pair
/// TODO: handle state as well
#[derive(Debug, Clone)]
pub struct Cached<I, O> {
    pub inputs: I,
    pub outputs: O,
}

/// Trait used to downcast and compare equality
pub trait DowncastEq {
    fn downcast_eq(&self, other: &dyn Any) -> bool;
}

impl<T: Any + PartialEq> DowncastEq for T {
    fn downcast_eq(&self, other: &dyn Any) -> bool {
        if let Some(other) = other.downcast_ref::<T>() {
            self == other
        } else {
            false
        }
    }
}

impl<S: Stage> Node<S> {
    pub fn new(stage: S, initial_state: S::State) -> Self {
        Self {
            stage,
            state: initial_state,
            inputs: None,
            outputs: None,
            cache: HashMap::new(),
            input_changed: true,
        }
    }

    /// Evaluates the node, returning any prior outputs
    fn eval(&mut self) -> Result<Option<S::Output>, InjectionError> {
        if let Some(inputs) = &mut self.inputs {
            Ok(self.outputs.replace(self
                .stage
                .evaluate(&mut self.state, inputs, &mut self.cache)?))
        } else {
            Err(InjectionError::InputNotFound(DataLabel::new_blank()))
        }
    }

    /// Evaluates the node asynchronously, returning any prior outputs
    #[cfg(feature = "tokio")]
    async fn eval_async(&mut self) -> Result<Option<S::Output>, InjectionError> {
        if let Some(inputs) = &mut self.inputs {
            Ok(self.outputs.replace(self
                .stage
                .evaluate_async(&mut self.state, inputs, &mut self.cache).await?))
        } else {
            Err(InjectionError::InputNotFound(DataLabel::new_blank()))
        }
    }
}
