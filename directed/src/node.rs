//! Nodes contain all logic and state, but no information on order of execution.
use std::{any::Any, collections::HashMap};

use crate::{
    stage::{EvalStrategy, ReevaluationRule, Stage}, InjectionError, StageShape, TypeReflection
};

/// Every node wraps a Stage, which is a decorated function that has some
/// number of inputs and some number of outputs.
///
/// TODO: Explain the caching functionality in detail
#[derive(Debug)]
pub struct Node<S: Stage> {
    pub stage: S,
    // Arbitrary state, by default will be ()
    pub state: S::State,
    pub inputs: S::Input,
    pub outputs: S::Output,
    pub cache: HashMap<u64, Vec<Cached<S>>>,
    pub input_changed: bool,
}

/// This represents a cached input/output pair
/// TODO: handle state as well
#[derive(Debug, Clone)]
pub struct Cached<S: Stage> {
    pub inputs: S::Input,
    pub outputs: S::Output,
}

impl<S: Stage> Node<S> {
    pub fn new(stage: S, initial_state: S::State) -> Self {
        Self {
            stage,
            state: initial_state,
            inputs: S::Input::default(),
            outputs: S::Output::default(),
            cache: HashMap::new(),
            input_changed: true,
        }
    }
}

/// This is used to type-erase a node. It's public because the macro needs to
/// use this, but there should be no reason anyone should manually implement
/// this.
#[cfg_attr(feature = "tokio", async_trait::async_trait)]
pub trait AnyNode: Any + Send + Sync + 'static {
    /// Gets the shape of the stage
    fn stage_shape(&self) -> &'static StageShape;
    /// Upcast to `dyn Any` to get its more-specific downcast capabilities
    fn into_any(self: Box<Self>) -> Box<dyn Any>;
    /// Primarily used for internal checks
    fn as_any(&self) -> &dyn Any;
    /// USed to get mutable access to state
    fn as_any_mut(&mut self) -> &mut dyn Any;
    /// Evaluates the node. Returns a map of prior outputs
    fn eval(&mut self) -> Result<Box<dyn DynFields>, InjectionError>;
    #[cfg(feature = "tokio")]
    async fn eval_async(&mut self) -> Result<Box<dyn DynFields + Send + Sync>, InjectionError>;
    fn eval_strategy(&self) -> EvalStrategy;
    fn reeval_rule(&self) -> ReevaluationRule;
    /// This a a core part of the plumbing of this crate - take the outputs of
    /// a parent node and use them to set the inputs of a child node.
    fn flow_data(
        &mut self,
        child: &mut Box<dyn AnyNode>,
        output: Option<&'static TypeReflection>,
        input: Option<&'static TypeReflection>,
    ) -> Result<(), InjectionError>;
    /// Used to support `[Self::flow_data]`
    fn inputs_mut(&mut self) -> &mut dyn DynFields;
    /// Used to support `[Self::flow_data]`
    fn outputs_mut(&mut self) -> &mut dyn DynFields;
    /// Used to indicate that an input has been modified from a previous run.
    fn set_input_changed(&mut self, val: bool);
    /// See `[Self::set_input_changed]`
    fn input_changed(&self) -> bool;
    fn cache_mut(&mut self) -> &mut dyn Any;
}

#[cfg_attr(feature = "tokio", async_trait::async_trait)]
impl<S: Stage + Send + Sync + 'static> AnyNode for Node<S>
{
    fn stage_shape(&self) -> &'static StageShape {
        &S::SHAPE
    }

    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        Box::new(*self)
    }

    fn as_any(&self) -> &dyn Any {
        self as &dyn Any
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self as &mut dyn Any
    }

    fn eval_strategy(&self) -> EvalStrategy {
        self.stage.eval_strategy()
    }

    fn reeval_rule(&self) -> ReevaluationRule {
        self.stage.reeval_rule()
    }

    fn eval(&mut self) -> Result<Box<dyn DynFields>, InjectionError> {
        let outputs = self
            .stage
            .evaluate(&mut self.state, &mut self.inputs, &mut self.cache)?;
        Ok(self.outputs_mut().replace(Box::new(outputs)))
    }

    #[cfg(feature = "tokio")]
    async fn eval_async(&mut self) -> Result<Box<dyn DynFields + Send + Sync>, InjectionError> {
        let outputs = self
            .stage
            .evaluate_async(&mut self.state, &mut self.inputs, &mut self.cache)
            .await?;
        Ok(self.outputs_mut().replace(Box::new(outputs)))
    }

    fn flow_data(
        &mut self,
        child: &mut Box<dyn AnyNode>,
        output: Option<&'static TypeReflection>,
        input: Option<&'static TypeReflection>,
    ) -> Result<(), InjectionError> {
        self.stage.clone().inject_input(self, child, output, input)
    }

    fn inputs_mut(&mut self) -> &mut dyn DynFields {
        &mut self.inputs as &mut dyn DynFields
    }

    fn outputs_mut(&mut self) -> &mut dyn DynFields {
        &mut self.outputs as &mut dyn DynFields
    }

    fn set_input_changed(&mut self, val: bool) {
        self.input_changed = val;
    }

    fn input_changed(&self) -> bool {
        self.input_changed
    }

    fn cache_mut(&mut self) -> &mut dyn Any {
        &mut self.cache
    }
}

/// Trait to abstract over accessing and taking outputs from nodes
#[cfg(not(feature = "tokio"))]
pub trait DynFields: Any {
    fn field<'a>(&'a self, field: Option<&'static TypeReflection>) -> Option<&'a (dyn Any + 'static)>;
    fn field_mut<'a>(
        &'a mut self,
        field: Option<&'static TypeReflection>,
    ) -> Option<&'a mut (dyn Any + 'static)>;
    fn take_field(&mut self, field: Option<&'static TypeReflection>) -> Option<Box<dyn Any>>;
    fn replace(&mut self, other: Box<dyn Any>) -> Box<dyn DynFields>;
}

#[cfg(feature = "tokio")]
pub trait DynFields: Any + Send + Sync {
    fn field<'a>(&'a self, field: Option<&'static TypeReflection>) -> Option<&'a (dyn Any + 'static)>;
    fn field_mut<'a>(
        &'a mut self,
        field: Option<&'static TypeReflection>,
    ) -> Option<&'a mut (dyn Any + 'static)>;
    fn take_field(&mut self, field: Option<&'static TypeReflection>) -> Option<Box<dyn Any>>;
    fn replace(&mut self, other: Box<dyn Any>) -> Box<dyn DynFields>;
}