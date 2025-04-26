//! The registry is the "global" store of logic and state.
use slab::Slab;
use std::any::TypeId;

use crate::{
    node::{AnyNode, Node}, stage::Stage, NodeTypeMismatchError, NodesNotFoundError, RegistryError
};

/// A [Registry] stores each node, its state, and the logical [Stage]
/// associated with it.
pub struct Registry(pub(super) Slab<Box<dyn AnyNode>>);

impl Registry {
    pub fn new() -> Self {
        Self(Slab::new())
    }

    /// Get a reference to the state of a specific node.
    pub fn state<S: Stage + 'static>(&self, id: usize) -> Result<&S::State, RegistryError> {
        self.validate_node_type::<S>(id)?;
        match self.0.get(id) {
            Some(any_node) => match any_node.as_any().downcast_ref::<Node<S>>() {
                Some(node) => Ok(&node.state),
                None => unreachable!(),
            },
            None => Err(NodesNotFoundError::from(&[id] as &[usize]).into()),
        }
    }

    /// Get a mutable reference to the state of a specific node.
    pub fn state_mut<S: Stage + 'static>(&mut self, id: usize) -> Result<&mut S::State, RegistryError> {
        self.validate_node_type::<S>(id)?;
        match self.0.get_mut(id) {
            Some(any_node) => match any_node.as_any_mut().downcast_mut::<Node<S>>() {
                Some(node) => Ok(&mut node.state),
                None => unreachable!(),
            },
            None => Err(NodesNotFoundError::from(&[id] as &[usize]).into()),
        }
    }

    /// Add a node to the registry. This returns a unique identifier for that
    /// node, which can be used to add it to a [crate::Graph]. This uses default
    /// state
    pub fn register<S: Stage + 'static>(&mut self, stage: S) -> usize
    where S::State: Default {
        self.0.insert(Box::new(Node::new(stage, S::State::default())))
    }

    /// Add a node to the registry. This returns a unique identifier for that
    /// node, which can be used to add it to a [crate::Graph]
    pub fn register_with_state<S: Stage + 'static>(&mut self, stage: S, state: S::State) -> usize {
        self.0.insert(Box::new(Node::new(stage, state)))
    }

    /// Returns an error if the registry doesn't contain a node with a stage
    /// of the specified type with the given id.
    pub fn validate_node_type<S: Stage + 'static>(&self, id: usize) -> Result<(), RegistryError> {
        match self.0.get(id) {
            Some(node) => match node.as_any().downcast_ref::<Node<S>>() {
                Some(_) => Ok(()),
                None => Err(NodeTypeMismatchError{got: TypeId::of::<Node<S>>(), expected: node.as_any().type_id()}.into()),
            },
            None => Err(NodesNotFoundError::from(&[id] as &[usize]).into()),
        }
    }

    /// Remove a node from the registry and return it. This will return an error if
    /// S doesn't match the stage type for that node.
    pub fn unregister<S: Stage + 'static>(&mut self, id: usize) -> Result<Option<Node<S>>, RegistryError> {
        self.validate_node_type::<S>(id)?;
        match self.0.try_remove(id) {
            Some(node) => match node.into_any().downcast() {
                Ok(node) => Ok(Some(*node)),
                Err(node) => Err(NodeTypeMismatchError{got: TypeId::of::<Node<S>>(), expected: node.type_id()}.into()),
            },
            None => Ok(None),
        }
    }

    /// Remove a node from the registry and drop it.
    pub fn unregister_and_drop(&mut self, id: usize) -> Result<(), RegistryError> {
        match self.0.try_remove(id).map(drop) {
            Some(_) => Ok(()),
            None => Err(NodesNotFoundError::from(&[id] as &[usize]).into()),
        }
    }

    /// Get a type-erased node
    pub fn get(&self, id: usize) -> Option<&Box<dyn AnyNode>> {
        self.0.get(id)
    }

    /// Get a mutable type-erased node
    pub fn get_mut(&mut self, id: usize) -> Option<&mut Box<dyn AnyNode>> {
        self.0.get_mut(id)
    }

    /// Get 2 mutable type-erased nodes
    ///
    /// This is an important internal detail: a parent a child node often need
    /// to be modified together.
    /// 
    /// This function will panic if id0 and id1 are the same.
    pub fn get2_mut(
        &mut self,
        id0: usize,
        id1: usize,
    ) -> Result<(&mut Box<dyn AnyNode>, &mut Box<dyn AnyNode>), NodesNotFoundError> {
        match self.0.get2_mut(id0, id1) {
            Some((node0, node1)) => Ok((node0, node1)),
            None => Err(NodesNotFoundError::from(&[id0, id1] as &[usize])),
        }
    }
}
