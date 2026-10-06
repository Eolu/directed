//! The `Stage` trait, its object-safe erased form, and the caching / evaluation
//! policy enums.

use std::future::Future;
use std::pin::Pin;

use crate::error::CallError;
use crate::io::Io;
use crate::registry::NodeId;
use crate::signature::Signature;

/// When a node is evaluated relative to a graph execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvalStrategy {
    /// Only evaluate when an urgent descendant needs it.
    Lazy,
    /// Evaluate as soon as possible. A graph with no urgent nodes does nothing.
    Urgent,
}

/// How a node reuses previous results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CachePolicy {
    /// Opaque: consume inputs and re-evaluate every time.
    None,
    /// Transparent: reuse the previous outputs when the inputs are unchanged.
    Last,
    /// Memoize every distinct combination of inputs.
    All,
}

/// A statically-typed node handle (the `T` in [`Stage::Handle`]).
pub trait StageHandle: Copy {
    fn id(&self) -> NodeId;
}

pub(crate) type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A wrapped function. This trait is *not* object-safe on purpose: erasure
/// happens exactly once, through [`Erased`].
pub trait Stage: Send + Sync + Sized + 'static {
    /// Arbitrary per-node state, mutated across evaluations.
    type State: Send + 'static;
    /// A generated, statically-typed handle to a registered node.
    type Handle: StageHandle + 'static;

    const EVAL: EvalStrategy = EvalStrategy::Urgent;
    const CACHE: CachePolicy = CachePolicy::None;

    /// The static port description, used for validation and diagnostics.
    fn signature() -> &'static Signature;

    /// Build a typed handle for a registered node id.
    fn handle(id: NodeId) -> Self::Handle;

    /// The single method the `#[stage]` macro implements.
    fn call<'a>(
        state: &'a mut Self::State,
        io: &'a mut Io,
    ) -> impl Future<Output = Result<(), CallError>> + Send + 'a;
}

/// The object-safe view the engine actually sees.
pub(crate) trait DynStage: Send {
    fn signature(&self) -> &'static Signature;
    fn eval_strategy(&self) -> EvalStrategy;
    fn cache_policy(&self) -> CachePolicy;
    fn run<'a>(&'a mut self, io: &'a mut Io) -> BoxFuture<'a, Result<(), CallError>>;
}

/// Adapter that erases a [`Stage`] plus its state into a [`DynStage`].
pub(crate) struct Erased<S: Stage> {
    pub(crate) state: S::State,
}

impl<S: Stage> DynStage for Erased<S> {
    fn signature(&self) -> &'static Signature {
        S::signature()
    }

    fn eval_strategy(&self) -> EvalStrategy {
        S::EVAL
    }

    fn cache_policy(&self) -> CachePolicy {
        S::CACHE
    }

    fn run<'a>(&'a mut self, io: &'a mut Io) -> BoxFuture<'a, Result<(), CallError>> {
        Box::pin(S::call(&mut self.state, io))
    }
}
