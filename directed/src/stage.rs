use std::{any::TypeId, collections::HashMap};

use crate::{
    node::{AnyNode, Node},
    types::{DataLabel, NodeOutput},
};

/// Defines all the information about how a stage is handled.
pub trait Stage: Clone {
    /// Internal state only, no special rules apply to this
    type State;
    /// The base function for this stage
    type BaseFn;

    /// Used for typechecking inputs
    fn inputs(&self) -> &HashMap<DataLabel, TypeId>;
    /// Used for typechecking outputs
    fn outputs(&self) -> &HashMap<DataLabel, TypeId>;
    /// Evaluate the stage with the given input and state
    fn evaluate(
        &self,
        state: &mut Option<Self::State>,
        inputs: &mut HashMap<DataLabel, Box<dyn std::any::Any>>,
    ) -> anyhow::Result<NodeOutput>;

    fn eval_strategy(&self) -> EvalStrategy {
        EvalStrategy::Lazy
    }

    fn reeval_rule(&self) -> ReevaluationRule {
        ReevaluationRule::Move
    }

    /// Stage-level connection processing logic. See [Node::flow_data] for more
    /// information.  
    fn process_connection(
        &self,
        node: &mut Node<Self>,
        parent: &mut Box<dyn AnyNode>,
        output: DataLabel,
        input: DataLabel,
    ) -> Result<(), anyhow::Error>;

    fn get_fn() -> Self::BaseFn;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvalStrategy {
    /// Only evaluate when necessary to evaluate an "Urgent" stage.
    Lazy,
    /// Evaluate as soon as possible. There must be at least 1 "Urgent" stage
    /// for anything to execute at all.
    Urgent,
}

// TODO: Remove this dead code
// /// TODO: Plan to change this to the following variants:
// ///     - Move: Always move outputs, reevaluate every time
// ///     - CacheLast: If all inputs == previous inputs, don't evalueate and 
// ///                  return cloned output
// ///     - CacheAll: If all inputs == ANMY previous set of inputs, don't
// ///                 evaluate and return cloned output associated with those 
// ///                 inputs
// #[derive(Debug, Clone, Copy, PartialEq, Eq)]
// pub enum ReevaluationRule {
//     /// Never reevaluate unless input differs, used cached outputs if input is the same.
//     Transparent,
//     /// Always reevaluate, even if inputs are unmodified.
//     Opaque,
// }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReevaluationRule {
    /// Always move outputs, reevaluate every time. If the receiving node takes
    /// a reference, it will be pased in, then dropped after that node
    /// evaluates.
    Move,
    /// If all inputs are previous inputs, don't evaluate and just return a
    /// clone of the cached output.
    CacheLast,
    /// If all inputs are equal to ANY previous input combination, don't 
    /// evaluate and just return a clone of the cached output associated with
    /// that exact set of inputs.
    // TODO: Currently unimplemented
    CacheAll,
}
