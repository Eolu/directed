use std::collections::HashMap;

use crate::{
    node::Node, AnyNode, InjectionError
};
use facet::Facet;

#[derive(Debug, Clone, Copy, PartialEq, Hash)]
pub enum RefType {
    Owned,
    Borrowed,
    BorrowedMut,
}

/// Defines all the information about how a stage is handled.
#[cfg_attr(feature = "tokio", async_trait::async_trait)]
pub trait Stage: Clone {
    /// Internal state only, no special rules apply to this. This is stored
    /// as a tuple of all state parameters in order.
    type State: Facet<'static>;
    /// TODO: The input of this stage
    #[cfg(feature="tokio")]
    type Input: Send + Sync + Facet<'static>;
    #[cfg(not(feature="tokio"))]
    type Input: Facet<'static>;
    /// TODO: The output of this stage
    #[cfg(feature="tokio")]
    type Output: Send + Sync + Facet<'static>;
    #[cfg(not(feature="tokio"))]
    type Output: Facet<'static>;

    /// Evaluate the stage with the given input and state
    fn evaluate(
        &self,
        state: &mut Self::State,
        inputs: &mut Self::Input,
        cache: &mut HashMap<u64, Vec<crate::Cached<Self::Input, Self::Output>>>,
    ) -> Result<Self::Output, InjectionError>;
    /// async version of evaluate
    #[cfg(feature = "tokio")]
    async fn evaluate_async(
        &self,
        state: &mut Self::State,
        inputs: &mut Self::Input,
        cache: &mut HashMap<u64, Vec<crate::Cached<Self::Input, Self::Output>>>,
    ) -> Result<Self::Output, InjectionError>;

    fn eval_strategy(&self) -> EvalStrategy {
        EvalStrategy::Lazy
    }

    fn reeval_rule(&self) -> ReevaluationRule {
        ReevaluationRule::Move
    }

    /// Stage-level connection processing logic. See [Node::flow_data] for more
    /// information.  
    fn inject_input(
        &self,
        node: &mut Node<Self>,
        parent: &mut Box<dyn AnyNode>
    ) -> Result<(), InjectionError>;

    /// Stage name, used for debugging information
    fn name(&self) -> &str;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvalStrategy {
    /// Only evaluate when necessary to evaluate an "Urgent" stage.
    Lazy,
    /// Evaluate as soon as possible. There must be at least 1 "Urgent" stage
    /// for anything to execute at all.
    Urgent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    CacheAll,
}
