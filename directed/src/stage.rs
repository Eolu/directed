use facet::{Facet, Field, Shape};
use std::collections::HashMap;

use crate::DynFields;
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
    pub inputs: &'static Shape,
    pub outputs: &'static Shape,
}

impl StageShape {
    pub fn input_fields(&self) -> &'static [Field] {
        match self.inputs.ty {
            facet::Type::User(user_type) => match user_type {
                facet::UserType::Struct(struct_type) => struct_type.fields,
                _ => unreachable!(),
            },
            _ => unreachable!(),
        }
    }

    pub fn output_fields(&self) -> &'static [Field] {
        match self.outputs.ty {
            facet::Type::User(user_type) => match user_type {
                facet::UserType::Struct(struct_type) => struct_type.fields,
                _ => unreachable!(),
            },
            _ => unreachable!(),
        }
    }
}

/// Defines all the information about how a stage is handled.
#[cfg_attr(feature = "tokio", async_trait::async_trait)]
pub trait Stage: Clone + 'static {
    /// Used for reflection
    const SHAPE: StageShape;
    /// Internal state only, no special rules apply to this. This is stored
    /// as a tuple of all state parameters in order.
    #[cfg(not(feature = "tokio"))]
    type State;
    #[cfg(feature = "tokio")]
    type State: Send + Sync;
    /// TODO: The input of this stage
    #[cfg(feature = "tokio")]
    type Input: Send + Sync + Default + DynFields + Facet<'static>;
    #[cfg(not(feature = "tokio"))]
    type Input: Default + DynFields + Facet<'static>;
    /// TODO: The output of this stage
    #[cfg(feature = "tokio")]
    type Output: Send + Sync + Default + DynFields + Facet<'static>;
    #[cfg(not(feature = "tokio"))]
    type Output: Default + DynFields + Facet<'static>;

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
        output: Option<&'static Field>,
        input: Option<&'static Field>,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Facet)]
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
