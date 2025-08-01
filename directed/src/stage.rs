use crate::DynFields;
use crate::{
    InjectionError,
    node::{AnyNode, Node},
};
use std::collections::HashMap;

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
    pub inputs: &'static [&'static str],
    pub outputs: &'static [&'static str],
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

    fn reeval_rule(&self) -> ReevaluationRule {
        ReevaluationRule::Move
    }

    /// Stage-level connection processing logic. See [Node::flow_data] for more
    /// information.  
    fn inject_input(
        &self,
        node: &mut Node<Self>,
        parent: &mut Box<dyn AnyNode>,
        output: Option<&'static str>,
        input: Option<&'static str>,
    ) -> Result<(), InjectionError>;
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

/// Empty version of a stage. Does nothing
#[cfg_attr(feature = "tokio", async_trait::async_trait)]
impl Stage for () {
    const SHAPE: StageShape = StageShape {
        stage_name: "()",
        inputs: &[],
        outputs: &[],
    };
    type State = ();
    type Input = ();
    type Output = ();
    fn evaluate(
        &self,
        _: &mut Self::State,
        _: &mut Self::Input,
        _: &mut HashMap<u64, Vec<crate::Cached<Self>>>,
    ) -> Result<Self::Output, InjectionError> {
        Ok(())
    }
    #[cfg(feature = "tokio")]
    async fn evaluate_async(
        &self,
        _: &mut Self::State,
        _: &mut Self::Input,
        _: &mut HashMap<u64, Vec<crate::Cached<Self>>>,
    ) -> Result<Self::Output, InjectionError> {
        Ok(())
    }
    fn inject_input(
        &self,
        _: &mut Node<Self>,
        _: &mut Box<dyn AnyNode>,
        _: Option<&'static str>,
        _: Option<&'static str>,
    ) -> Result<(), InjectionError> {
        Ok(())
    }
}

/// A stage that just returns a value. Currently this is somewhat naive and will always clone the value.
#[derive(Clone, Copy)]
pub struct ValueStage<T: Send + Sync + Clone + 'static>(std::marker::PhantomData<T>);
impl<T: Send + Sync + Clone + 'static> ValueStage<T> {
    pub fn new() -> Self {
        Self(std::marker::PhantomData)
    }
}

/// Wrapper used by [`ValueStage`]
#[derive(Clone)]
pub struct ValueWrapper<T: Send + Sync + Clone + 'static>(pub Option<T>);
impl<T: Send + Sync + Clone + 'static> Default for ValueWrapper<T> {
    fn default() -> Self {
        Self(None)
    }
}

impl<T: Send + Sync + Clone + 'static> DynFields for ValueWrapper<T> {
    fn field<'a>(&'a self, _: Option<&'static str>) -> Option<&'a (dyn std::any::Any + 'static)> {
        self.0.as_ref().map(|t| t as &dyn std::any::Any)
    }

    fn field_mut<'a>(
        &'a mut self,
        _: Option<&'static str>,
    ) -> Option<&'a mut (dyn std::any::Any + 'static)> {
        self.0.as_mut().map(|t| t as &mut dyn std::any::Any)
    }

    fn take_field(&mut self, _: Option<&'static str>) -> Option<Box<dyn std::any::Any>> {
        self.0.take().map(|t| Box::new(t) as Box<dyn std::any::Any>)
    }

    fn replace(&mut self, other: Box<dyn std::any::Any>) -> Box<dyn DynFields> {
        if let Ok(other) = other.downcast() {
            Box::new(std::mem::replace(self, *other))
        } else {
            panic!("Attempted to replace value with wrong type")
        }
    }

    fn clear(&mut self) {
        self.0 = None;
    }
}

#[cfg_attr(feature = "tokio", async_trait::async_trait)]
impl<T: Send + Sync + Clone + 'static> Stage for ValueStage<T> {
    const SHAPE: StageShape = StageShape {
        stage_name: "_",
        inputs: &[],
        outputs: &["_"],
    };
    type State = ValueWrapper<T>;
    type Input = ();
    type Output = ValueWrapper<T>;
    fn evaluate(
        &self,
        state: &mut Self::State,
        _: &mut Self::Input,
        _: &mut HashMap<u64, Vec<crate::Cached<Self>>>,
    ) -> Result<Self::Output, InjectionError> {
        Ok(state.clone())
    }
    #[cfg(feature = "tokio")]
    async fn evaluate_async(
        &self,
        state: &mut Self::State,
        _: &mut Self::Input,
        _: &mut HashMap<u64, Vec<crate::Cached<Self>>>,
    ) -> Result<Self::Output, InjectionError> {
        Ok(state.clone())
    }
    fn inject_input(
        &self,
        _: &mut Node<Self>,
        _: &mut Box<dyn AnyNode>,
        _: Option<&'static str>,
        _: Option<&'static str>,
    ) -> Result<(), InjectionError> {
        Ok(())
    }
}
