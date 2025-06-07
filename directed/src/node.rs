//! Nodes contain all logic and state, but no information on order of execution.
use std::{
    any::{Any, TypeId},
    collections::HashMap,
    sync::Arc,
};
use facet::{Facet, Field, Shape};

use crate::{
    InjectionError, RefType,
    stage::{EvalStrategy, ReevaluationRule, Stage},
};

/// Type-erasure for nodes
pub trait AnyNode: Any {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn into_any(self: Box<Self>) -> Box<dyn Any>;
    fn eval_strategy(&self) -> EvalStrategy;
    fn reeval_rule(&self) -> ReevaluationRule;
    fn stage_name(&self) -> &str;
    fn input_changed(&self) -> bool;
    fn set_input_changed(&mut self, value: bool);
    fn eval(&mut self) -> Result<Option<Box<dyn Any>>, InjectionError>;
    fn input_shape(&self) -> &Shape;
    fn output_shape(&self) -> &Shape;
    fn state_shape(&self) -> &Shape;
    /// This a a core part of the plumbing of this crate - take the outputs of
    /// a parent node and use them to set the inputs of a child node.
    fn flow_data(
        &mut self,
        child: &mut Box<dyn AnyNode>,
        output: Field,
        input: Field,
    ) -> Result<(), InjectionError>;
}

impl<S: Stage + 'static> AnyNode for Node<S> {
    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    
    fn eval_strategy(&self) -> EvalStrategy {
        self.stage.eval_strategy()
    }
    
    fn reeval_rule(&self) -> ReevaluationRule {
        self.stage.reeval_rule()
    }
    
    fn stage_name(&self) -> &str {
        self.stage.name()
    }

    fn input_changed(&self) -> bool {
        self.input_changed
    }

    fn set_input_changed(&mut self, value: bool) {
        self.input_changed = value;
    }

    fn eval(&mut self) -> Result<Option<Box<dyn Any>>, InjectionError> {
        self.eval().map(|out| out.map(|out| Box::new(out) as Box<dyn Any>)) 
    }

    fn input_shape(&self) -> &Shape {
        S::Input::SHAPE
    }

    fn output_shape(&self) -> &Shape {
        S::Output::SHAPE
    }

    fn state_shape(&self) -> &Shape {
        S::State::SHAPE
    }

    fn flow_data(
        &mut self,
        child: &mut Box<dyn AnyNode>,
        output: Field,
        input: Field,
    ) -> Result<(), InjectionError> {
        self.stage.clone().inject_input(self, child, output, input)
    }
}

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
            Err(InjectionError::InputNotFound(String::new()))
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
