//! Nodes contain all logic and state, but no information on order of execution.
use facet::Field;
use std::{any::Any, collections::HashMap, sync::Arc};

use crate::{
    InjectionError, StageShape,
    stage::{EvalStrategy, ReevaluationRule, Stage},
};

/// Every node wraps a Stage, which is a decorated function that has some
/// number of inputs and some number of outputs.
///
/// TODO: Explain the caching functionality in detail
#[derive(Debug)]
pub struct Node<S: Stage> {
    pub(super) stage: S,
    // Arbitrary state, by default will be ()
    pub(super) state: S::State,
    pub(super) inputs: S::Input,
    pub(super) outputs: Arc<S::Output>,
    pub(super) cache: HashMap<u64, Vec<Cached<S>>>,
    pub(super) input_changed: bool,
}

/// This represents a cached input/output pair
/// TODO: handle state as well
#[derive(Debug, Clone)]
pub struct Cached<S: Stage> {
    pub inputs: Arc<S::Input>,
    pub outputs: Arc<S::Output>,
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
            inputs: S::Input::default(),
            outputs: Arc::new(S::Output::default()),
            cache: HashMap::new(),
            input_changed: true,
        }
    }
}

/// This is used to type-erase a node. It's public because the macro needs to
/// use this, but there should be no reason anyone should manually implement
/// this.
#[cfg_attr(feature = "tokio", async_trait::async_trait)]
pub trait AnyNode: Any + Send + 'static {
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
    async fn eval_async(&mut self) -> Result<Arc<dyn Any + Send + Sync>, InjectionError>;
    fn eval_strategy(&self) -> EvalStrategy;
    fn reeval_rule(&self) -> ReevaluationRule;
    /// This a a core part of the plumbing of this crate - take the outputs of
    /// a parent node and use them to set the inputs of a child node.
    fn flow_data(
        &mut self,
        child: &mut Box<dyn AnyNode>,
        output: Option<&'static Field>,
        input: Option<&'static Field>,
    ) -> Result<(), InjectionError>;
    /// Used to support `[Self::flow_data]`
    fn inputs_mut(&mut self) -> &mut dyn DynFields;
    /// Used to support `[Self::flow_data]`
    fn outputs_mut(&mut self) -> Option<&mut dyn DynFields>;
    /// Used to indicate that an input has been modified from a previous run.
    fn set_input_changed(&mut self, val: bool);
    /// See `[Self::set_input_changed]`
    fn input_changed(&self) -> bool;
    fn cache_mut(&mut self) -> &mut dyn Any;
}

#[cfg_attr(feature = "tokio", async_trait::async_trait)]
impl<S: Stage + Send + 'static> AnyNode for Node<S>
where
    S::State: Send,
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
        if let Some(out_mut) = self.outputs_mut() {
            Ok(out_mut.replace(Box::new(outputs)))
        } else {
            panic!("TODO: Handle this");
        }
    }

    #[cfg(feature = "tokio")]
    async fn eval_async(
        &mut self,
    ) -> Result<HashMap<Field, Arc<dyn Any + Send + Sync>>, InjectionError> {
        let mut old_outputs = HashMap::new();
        let outputs = self
            .stage
            .evaluate_async(&mut self.state, &mut self.inputs, &mut self.cache)
            .await?;

        match outputs {
            NodeOutput::Standard(val) => {
                if let Some(old_output) = self.replace_output(&UNNAMED_OUTPUT_FIELD, val)? {
                    old_outputs.insert(UNNAMED_OUTPUT_FIELD.clone(), old_output);
                }
            }
            NodeOutput::Named(hash_map) => {
                for (key, val) in hash_map.into_iter() {
                    if let Some(old_output) = self.replace_output(&key, val)? {
                        old_outputs.insert(key, old_output);
                    }
                }
            }
        }
        Ok(old_outputs)
    }

    fn flow_data(
        &mut self,
        child: &mut Box<dyn AnyNode>,
        output: Option<&'static Field>,
        input: Option<&'static Field>,
    ) -> Result<(), InjectionError> {
        self.stage.clone().inject_input(self, child, output, input)
    }

    fn inputs_mut(&mut self) -> &mut dyn DynFields {
        &mut self.inputs as &mut dyn DynFields
    }

    fn outputs_mut(&mut self) -> Option<&mut dyn DynFields> {
        Arc::get_mut(&mut self.outputs).map(|inner| inner as &mut dyn DynFields)
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
pub trait DynFields: Any {
    fn field<'a>(&'a self, field: Option<&'static Field>) -> Option<&'a (dyn Any + 'static)>;
    fn field_mut<'a>(
        &'a mut self,
        field: Option<&'static Field>,
    ) -> Option<&'a mut (dyn Any + 'static)>;
    fn take_field(&mut self, field: Option<&'static Field>) -> Option<Box<dyn Any>>;
    fn replace(&mut self, other: Box<dyn Any>) -> Box<dyn DynFields>;
}
