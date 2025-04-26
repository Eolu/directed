//! Nodes contain all logic and state, but no information on order of execution.
use std::{
    any::{Any, TypeId},
    collections::HashMap,
    sync::Arc,
};

use crate::{
    stage::{EvalStrategy, ReevaluationRule, Stage}, types::{DataLabel, NodeOutput}, InjectionError, RefType
};

/// Used for single-output functions
pub(crate) const UNNAMED_OUTPUT_NAME: DataLabel = DataLabel::new_const("_");

/// Every node wraps a Stage, which is a decorated function that has some
/// number of inputs and some number of outputs.
///
/// TODO: Explain the caching functionality in detail
#[derive(Debug)]
pub struct Node<S: Stage> {
    pub(super) stage: S,
    // Arbitrary state, by default will be (). Can be used to make nodes even
    // MORE stateful.
    pub(super) state: S::State,
    pub(super) inputs: HashMap<DataLabel, (Arc<dyn Any + Send + Sync>, ReevaluationRule)>,
    pub(super) outputs: HashMap<DataLabel, Arc<dyn Any + Send + Sync>>,
    pub(super) input_changed: bool,
}

impl<S: Stage> Node<S> {
    pub fn new(stage: S, initial_state: S::State) -> Self {
        Self {
            stage,
            state: initial_state,
            inputs: HashMap::new(),
            outputs: HashMap::new(),
            input_changed: true,
        }
    }
}

/// This is used to type-erase a node. It's public because the macro needs to
/// use this, but there should be no reason anyone should manually implement
/// this.
pub trait AnyNode: Any {
    /// Upcast to `dyn Any` to get its more-specific downcast capabilities
    fn into_any(self: Box<Self>) -> Box<dyn Any>;
    /// Primarily used for internal checks
    fn as_any(&self) -> &dyn Any;
    /// USed to get mutable access to state
    fn as_any_mut(&mut self) -> &mut dyn Any;
    /// Evaluates the node. Returns a map of prior outputs
    fn eval(&mut self) -> Result<HashMap<DataLabel, Arc<dyn Any + Send + Sync>>, InjectionError>;
    fn eval_strategy(&self) -> EvalStrategy;
    fn reeval_rule(&self) -> ReevaluationRule;
    /// Set the outputs and return any existing outputs.
    // TODO: Update this doc when cache-all is implemented.
    fn replace_output(
        &mut self,
        key: &DataLabel,
        output: Arc<dyn Any + Send + Sync>,
    ) -> Result<Option<Arc<dyn Any + Send + Sync>>, InjectionError>;
    /// This a a core part of the plumbing of this crate - take the outputs of
    /// a parent node and use them to set the inputs of a child node.
    fn flow_data(
        &mut self,
        parent: &mut Box<dyn AnyNode>,
        output: DataLabel,
        input: DataLabel,
    ) -> Result<(), InjectionError>;
    /// Used to support `[Self::flow_data]`
    fn inputs_mut(
        &mut self,
    ) -> &mut HashMap<DataLabel, (Arc<dyn Any + Send + Sync>, ReevaluationRule)>;
    /// Used to support `[Self::flow_data]`
    fn outputs_mut(&mut self) -> &mut HashMap<DataLabel, Arc<dyn Any + Send + Sync>>;
    /// Look of the reftype of a particular input
    fn input_reftype(&self, name: &DataLabel) -> Option<RefType>;
    /// Used to indicate that an input has been modified from a previous run.
    fn set_input_changed(&mut self, val: bool);
    /// See `[Self::set_input_changed]`
    fn input_changed(&self) -> bool;
    fn input_names(&self) -> std::iter::Cloned<std::collections::hash_map::Keys<'_, DataLabel, (TypeId, RefType)>>;
    fn output_names(&self) -> std::iter::Cloned<std::collections::hash_map::Keys<'_, DataLabel, TypeId>>;
    fn stage_name(&self) -> &str;
}

impl<S: Stage + 'static> AnyNode for Node<S> {
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

    fn replace_output(
        &mut self,
        key: &DataLabel,
        output: Arc<dyn Any + Send + Sync>,
    ) -> Result<Option<Arc<dyn Any + Send + Sync>>, InjectionError> {
        let output_type = self
            .stage
            .outputs()
            .get(key)
            .ok_or_else(|| InjectionError::OutputNotFound(key.clone()))?;
        match (&*output).type_id() {
            id if id != *output_type => {
                return Err(InjectionError::OutputTypeMismatch(key.clone()));
            }
            _ => Ok(self.outputs.insert(key.clone(), output)),
        }
    }

    fn eval(&mut self) -> Result<HashMap<DataLabel, Arc<dyn Any + Send + Sync>>, InjectionError> {
        let mut old_outputs = HashMap::new();
        // TODO: This stage clone is silly spaghetti
        let outputs = self.stage.evaluate(&mut self.state, &mut self.inputs)?;

        match outputs {
            NodeOutput::Standard(val) => {
                if let Some(old_output) = self.replace_output(&UNNAMED_OUTPUT_NAME, val)? {
                    old_outputs.insert(UNNAMED_OUTPUT_NAME, old_output);
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
        parent: &mut Box<dyn AnyNode>,
        output: DataLabel,
        input: DataLabel,
    ) -> Result<(), InjectionError> {
        let stage_clone = self.stage.clone();
        stage_clone.inject_input(self, parent, output, input)
    }

    fn inputs_mut(
        &mut self,
    ) -> &mut HashMap<DataLabel, (Arc<dyn Any + Send + Sync>, ReevaluationRule)> {
        &mut self.inputs
    }

    fn outputs_mut(&mut self) -> &mut HashMap<DataLabel, Arc<dyn Any + Send + Sync>> {
        &mut self.outputs
    }

    fn set_input_changed(&mut self, val: bool) {
        self.input_changed = val;
    }

    fn input_changed(&self) -> bool {
        self.input_changed
    }

    fn input_reftype(&self, name: &DataLabel) -> Option<RefType> {
        self.stage.inputs().get(name).map(|input| input.1)
    }
    
    fn input_names(&self) -> std::iter::Cloned<std::collections::hash_map::Keys<'_, DataLabel, (TypeId, RefType)>> {
        self.stage.inputs().keys().cloned()
    }
    
    fn output_names(&self) -> std::iter::Cloned<std::collections::hash_map::Keys<'_, DataLabel, TypeId>> {
        self.stage.outputs().keys().cloned()
    }
    
    fn stage_name(&self) -> &str {
        self.stage.name()
    }
}
