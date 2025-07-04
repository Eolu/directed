//! The registry is the "global" store of logic and state.

use crate::{
    NodeTypeMismatchError, NodesNotFoundError, RegistryError,
    node::{AnyNode, Node},
    stage::{Stage, StageShape},
};
#[cfg(feature = "tokio")]
use std::sync::Arc;
use std::{any::TypeId, marker::PhantomData};

/// Used to access nodes within the registry, just a simple `usize` alias
#[derive(Debug, Clone, Copy, Eq, Hash, Ord)]
#[repr(transparent)]
pub struct NodeId<S: Stage>(pub(crate) usize, pub(crate) PhantomData<S>);
impl<S: Stage> From<NodeId<S>> for usize {
    fn from(value: NodeId<S>) -> Self {
        value.0
    }
}
impl<S0: Stage, S1: Stage> PartialEq<NodeId<S1>> for NodeId<S0> {
    fn eq(&self, other: &NodeId<S1>) -> bool {
        self.0 == other.0
    }
}
impl<S0: Stage, S1: Stage> PartialOrd<NodeId<S1>> for NodeId<S0> {
    fn partial_cmp(&self, other: &NodeId<S1>) -> Option<std::cmp::Ordering> {
        self.0.partial_cmp(&other.0)
    }
}
impl<S: Stage> NodeId<S> {
    pub fn stage_shape(&self) -> &'static StageShape {
        Into::<NodeReflection>::into(self).shape
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeReflection {
    pub(crate) id: usize,
    pub(crate) shape: &'static StageShape,
}
impl<S: Stage + 'static> From<NodeId<S>> for NodeReflection {
    fn from(value: NodeId<S>) -> Self {
        Self {
            id: value.0,
            shape: &S::SHAPE,
        }
    }
}
impl<S: Stage + 'static> From<&NodeId<S>> for NodeReflection {
    fn from(value: &NodeId<S>) -> Self {
        Self {
            id: value.0,
            shape: &S::SHAPE,
        }
    }
}
impl PartialOrd for NodeReflection {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.id.partial_cmp(&other.id)
    }
}
impl Ord for NodeReflection {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.id.cmp(&other.id)
    }
}

/// A [Registry] stores each node, its state, and the logical [Stage]
/// associated with it.
pub struct Registry(
    pub(super) Vec<Option<Box<dyn AnyNode>>>, 
    #[cfg(feature = "tokio")] pub(super) Vec<(tokio::sync::watch::Sender<bool>, tokio::sync::watch::Receiver<bool>)>
);

impl Registry {

    pub fn new() -> Self {
        Self(Vec::new(), #[cfg(feature = "tokio")] Vec::new())
    }

    /// Get a reference to the state of a specific node.
    pub fn state<S: Stage + 'static>(&self, id: NodeId<S>) -> Result<&S::State, RegistryError> {
        self.validate_node_type::<S>(&id)?;
        match self.0.get(id.0) {
            Some(Some(any_node)) => match any_node.as_any().downcast_ref::<Node<S>>() {
                Some(node) => Ok(&node.state),
                None => Err(NodeTypeMismatchError {
                    got: any_node.type_id(),
                    expected: TypeId::of::<Node<S>>(),
                }
                .into()),
            },
            // TODO: Handle Some(None) as a special case, as this means the node is busy
            None | Some(None) => {
                Err(NodesNotFoundError::from(&[id.into()] as &[NodeReflection]).into())
            }
        }
    }

    /// Get a mutable reference to the state of a specific node.
    pub fn state_mut<S: Stage + 'static>(
        &mut self,
        id: NodeId<S>,
    ) -> Result<&mut S::State, RegistryError> {
        self.validate_node_type::<S>(&id)?;
        match self.0.get_mut(id.0) {
            Some(Some(any_node)) => {
                let node_type_id = any_node.type_id();
                match any_node.as_any_mut().downcast_mut::<Node<S>>() {
                    Some(node) => Ok(&mut node.state),
                    None => Err(NodeTypeMismatchError {
                        got: node_type_id,
                        expected: TypeId::of::<Node<S>>(),
                    }
                    .into()),
                }
            }
            // TODO: Handle Some(None) as a special case, as this means the node is busy
            None | Some(None) => {
                Err(NodesNotFoundError::from(&[id.into()] as &[NodeReflection]).into())
            }
        }
    }

    /// Add a node to the registry. This returns a unique identifier for that
    /// node, which can be used to add it to a [crate::Graph]. This uses default
    /// state
    pub fn register<S: Stage + Send + Sync + 'static>(&mut self, stage: S) -> NodeId<S>
    where
        S::State: Default,
    {
        let next = self.0.len();
        self.0
            .push(Some(Box::new(Node::new(stage, S::State::default()))));
        #[cfg(feature = "tokio")]
        self.1.push(tokio::sync::watch::channel(true));
        NodeId(next, PhantomData)
    }

    /// Add a node to the registry. This returns a unique identifier for that
    /// node, which can be used to add it to a [crate::Graph].
    ///
    /// [crate::stage::Stage::State] is taken as a tuple of all the state
    /// parameters in the order they were defined.
    pub fn register_with_state<S: Stage + Send + Sync + 'static>(
        &mut self,
        stage: S,
        state: S::State,
    ) -> NodeId<S>
    {
        let next = self.0.len();
        self.0.push(Some(Box::new(Node::new(stage, state))));
        #[cfg(feature = "tokio")]
        self.1.push(tokio::sync::watch::channel(true));
        NodeId(next, PhantomData)
    }

    /// Returns an error if the registry doesn't contain a node with a stage
    /// of the specified type with the given id.
    pub fn validate_node_type<S: Stage + 'static>(
        &self,
        id: impl Into<NodeReflection>,
    ) -> Result<(), RegistryError> {
        let id = id.into();
        match self.0.get(id.id) {
            Some(Some(node)) => match node.as_any().downcast_ref::<Node<S>>() {
                Some(_) => Ok(()),
                None => Err(NodeTypeMismatchError {
                    got: TypeId::of::<Node<S>>(),
                    expected: node.as_any().type_id(),
                }
                .into()),
            },
            // TODO: Handle Some(None) as a special case, as this means the node is busy
            None | Some(None) => {
                Err(NodesNotFoundError::from(&[id.clone().into()] as &[NodeReflection]).into())
            }
        }
    }

    /// Remove a node from the registry and return it. This will return an error if
    /// S doesn't match the stage type for that node.
    pub fn unregister<S: Stage + 'static>(
        &mut self,
        id: NodeReflection,
    ) -> Result<Option<Node<S>>, RegistryError> {
        self.validate_node_type::<S>(id)?;
        match self.0.get_mut(id.id) {
            Some(maybe_node) => maybe_node
                .take()
                .map(|node| match node.into_any().downcast() {
                    Ok(node) => Ok(Some(*node)),
                    Err(node) => Err(NodeTypeMismatchError {
                        got: TypeId::of::<Node<S>>(),
                        expected: node.type_id(),
                    }
                    .into()),
                })
                .unwrap_or(Ok(None)),
            None => Ok(None),
        }
    }

    /// Remove a node from the registry and drop it.
    pub fn unregister_and_drop(
        &mut self,
        id: impl Into<NodeReflection>,
    ) -> Result<(), RegistryError> {
        let id = id.into();
        match self.0.get_mut(id.id).take().map(drop) {
            Some(_) => Ok(()),
            None => Err(NodesNotFoundError::from(&[id.into()] as &[NodeReflection]).into()),
        }
    }

    /// Get a node
    pub fn get_node<S: Stage + 'static>(&self, id: NodeId<S>) -> Option<&Node<S>> {
        match self.0.get(id.0) {
            Some(Some(node)) => node.as_any().downcast_ref(),
            // TODO: Handle Some(None) as a special case, as this means the node is busy
            Some(None) => None,
            None => None,
        }
    }

    /// Get a type-erased node
    pub fn get_node_any(&self, id: impl Into<NodeReflection>) -> Option<&Box<dyn AnyNode>> {
        match self.0.get(id.into().id) {
            Some(Some(node)) => Some(node),
            // TODO: Handle Some(None) as a special case, as this means the node is busy
            Some(None) => None,
            None => None,
        }
    }

    /// Get a mutable node
    pub fn get_node_mut<S: Stage + 'static>(&mut self, id: NodeId<S>) -> Option<&mut Node<S>> {
        match self.0.get_mut(id.0) {
            Some(Some(node)) => node.as_any_mut().downcast_mut(),
            // TODO: Handle Some(None) as a special case, as this means the node is busy
            Some(None) => None,
            None => None,
        }
    }

    /// Get a mutable type-erased node
    pub fn get_node_any_mut(&mut self, id: impl Into<NodeReflection>) -> Option<&mut Box<dyn AnyNode>> {
        match self.0.get_mut(id.into().id) {
            Some(Some(node)) => Some(node),
            // TODO: Handle Some(None) as a special case, as this means the node is busy
            Some(None) => None,
            None => None,
        }
    }

    /// Get 2 mutable type-erased nodes
    ///
    /// This is an important internal detail: a parent a child node often need
    /// to be modified together.
    ///
    /// This function will panic if id0 and id1 are the same.
    pub fn get2_nodes_any_mut(
        &mut self,
        id0: NodeReflection,
        id1: NodeReflection,
    ) -> Result<(&mut Box<dyn AnyNode>, &mut Box<dyn AnyNode>), NodesNotFoundError> {
        if id0.id == id1.id {
            panic!(
                "Attempted to borrow node id {:?} twice",
                Into::<NodeReflection>::into(id0)
            )
        }
        let first_id = std::cmp::min(id0.id, id1.id);
        let second_id = std::cmp::max(id0.id, id1.id);
        match (self.0.len() < id0.id, self.0.len() < id1.id) {
            (true, true) => {
                return Err(NodesNotFoundError::from(
                    &[id0.into(), id1.into()] as &[NodeReflection]
                ));
            }
            (true, false) => {
                return Err(NodesNotFoundError::from(&[id0.into()] as &[NodeReflection]));
            }
            (false, true) => {
                return Err(NodesNotFoundError::from(&[id1.into()] as &[NodeReflection]));
            }
            _ => (),
        }

        if let [first, .., second] = &mut self.0[first_id..=second_id] {
            match (first, second) {
                (None, None) => Err(NodesNotFoundError::from(
                    &[id0.into(), id1.into()] as &[NodeReflection]
                )),
                (None, Some(_)) => {
                    Err(NodesNotFoundError::from(&[id0.into()] as &[NodeReflection]))
                }
                (Some(_), None) => {
                    Err(NodesNotFoundError::from(&[id1.into()] as &[NodeReflection]))
                }
                (Some(first), Some(second)) => {
                    if first_id == id1.id {
                        Ok((first, second))
                    } else {
                        Ok((second, first))
                    }
                }
            }
        } else {
            unreachable!()
        }
    }

    /// Used to await node availability
    #[cfg(feature = "tokio")]
    pub fn node_availability(&self, id: NodeReflection) -> Option<tokio::sync::watch::Receiver<bool>> {
        match self.1.get(id.id) {
            Some((_, rx)) => Some(rx.clone()),
            None => None,
        }
    }

    /// Takes a node out of the registry, gaining ownership of it.
    /// This does not unregister the node, and it can be placed back
    /// inside the registry via it's ID. Returns None if the node is
    /// already taken or never existed.
    #[cfg(feature = "tokio")]
    pub async fn take_node(&mut self, id: NodeReflection) -> Option<Box<dyn AnyNode>> {
        match self.0.get_mut(id.id) {
            Some(maybe_node) => {
                match maybe_node.take() {
                    Some(node) => {
                        // TODO: Handle errors sanely
                        self.1.get_mut(id.id).unwrap().0.send(false).unwrap();
                        Some(node)
                    },
                    None => {
                        panic!("TODO: Handle missing node")
                    },
                }
            },
            None => None,
        }
    }

    /// Insert a node with the given ID back into the registry. Panics
    /// if the node never existed at that ID in the first place.
    pub fn replace_node(&mut self, id: NodeReflection, node: Box<dyn AnyNode>) {
        *self.0.get_mut(id.id).unwrap() = Some(node);
        // TODO: Handle errors sanely
        #[cfg(feature = "tokio")]
        self.1.get_mut(id.id).unwrap().0.send(true).unwrap();
    }

    /// Gets a reference to the inputs from a node
    pub fn get_inputs<S: Stage + 'static>(&self, node_id: NodeId<S>) -> Option<&S::Input> {
        self.get_node(node_id).map(|node| &node.inputs)
    }

    /// Gets a mutable reference to the inputs from a node
    pub fn get_inputs_mut<S: Stage + 'static>(&mut self, node_id: NodeId<S>) -> Option<&mut S::Input> {
        self.get_node_mut(node_id).map(|node| &mut node.inputs)
    }

    /// Gets a reference to the outputs from a node
    pub fn get_outputs<S: Stage + 'static>(&self, node_id: NodeId<S>) -> Option<&S::Output> {
        self.get_node(node_id).map(|node| &node.outputs)
    }

    /// Gets a mutable reference to the outputs from a node
    pub fn get_outputs_mut<S: Stage + 'static>(&mut self, node_id: NodeId<S>) -> Option<&mut S::Output> {
        self.get_node_mut(node_id).map(|node| &mut node.outputs)
    }
}
