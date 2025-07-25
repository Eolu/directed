use std::collections::HashMap;
use crate::{DynFields, TypeReflection};
use crate::{
    InjectionError,
    node::{AnyNode, Node},
};

#[derive(Debug, Clone, Copy, PartialEq, Hash)]
pub enum RefType {
    Owned,
    Borrowed,
    BorrowedMut,
}

/// Type reflection for graph I/O
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StageShape {
    pub stage_name: &'static str,
    pub inputs: &'static [TypeReflection],
    pub outputs: &'static [TypeReflection],
}

/// Defines all the information about how a stage is handled.
#[cfg_attr(feature = "tokio", async_trait::async_trait)]
pub trait Stage: Clone + 'static {
    /// Used for reflection
    const SHAPE: StageShape;
    /// Internal state only, no special rules apply to this. This is stored
    /// as a tuple of all state parameters in order.
    /// TODO: Should be possible to relax Send+Sync bounds in sync contexts
    type State: Send + Sync;
    /// The input of this stage
    /// TODO: Should be possible to relax Send+Sync bounds in sync contexts
    type Input: Send + Sync + Default + DynFields;
    /// The output of this stage
    /// TODO: Should be possible to relax Send+Sync bounds in sync contexts
    type Output: Send + Sync + Default + DynFields;

    /// Evaluate the stage with the given input and state
    fn evaluate(
        &self,
        state: &mut Self::State,
        inputs: &mut Self::Input,
        cache: &mut HashMap<u64, Vec<crate::Cached<Self>>>,
    ) -> Result<Self::Output, InjectionError>;
    /// async version of evaluate
    #[cfg(feature = "tokio")]
    async fn evaluate_async(
        &self,
        state: &mut Self::State,
        inputs: &mut Self::Input,
        cache: &mut HashMap<u64, Vec<crate::Cached<Self>>>,
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
        parent: &mut Box<dyn AnyNode>,
        output: Option<&'static TypeReflection>,
        input: Option<&'static TypeReflection>,
    ) -> Result<(), InjectionError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvalStrategy {
    /// Only evaluate when necessary to evaluate an "Urgent" stage.
    Lazy,
    /// Evaluate as soon as possible. There must be at least 1 "Urgent" stage
    /// for anything to execute at all.
    Urgent,
}

#[repr(u8)]
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
