//! The registry is the "global" store of logic and state.
use std::any::TypeId;

use crate::{
    node::{AnyNode, Node}, stage::Stage, NodeTypeMismatchError, NodesNotFoundError, RegistryError
};

/// A [Registry] stores each node, its state, and the logical [Stage]
/// associated with it.
pub struct Registry(pub(super) Vec<Option<Box<dyn AnyNode>>>);

impl Registry {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// Get a reference to the state of a specific node.
    pub fn state<S: Stage + 'static>(&self, id: usize) -> Result<&S::State, RegistryError> {
        self.validate_node_type::<S>(id)?;
        match self.0.get(id) {
            Some(Some(any_node)) => match any_node.as_any().downcast_ref::<Node<S>>() {
                Some(node) => Ok(&node.state),
                None => Err(NodeTypeMismatchError{ got: any_node.type_id(), expected: TypeId::of::<Node<S>>() }.into()),
            },
            // TODO: Handle Some(None) as a special case, as this means the node is busy
            None | Some(None) => Err(NodesNotFoundError::from(&[id] as &[usize]).into()),
        }
    }

    /// Get a mutable reference to the state of a specific node.
    pub fn state_mut<S: Stage + 'static>(&mut self, id: usize) -> Result<&mut S::State, RegistryError> {
        self.validate_node_type::<S>(id)?;
        match self.0.get_mut(id) {
            Some(Some(any_node)) => {
                let node_type_id = any_node.type_id();
                match any_node.as_any_mut().downcast_mut::<Node<S>>() {
                    Some(node) => Ok(&mut node.state),
                    None => Err(NodeTypeMismatchError{ got: node_type_id, expected: TypeId::of::<Node<S>>() }.into()),
                }
            },
            // TODO: Handle Some(None) as a special case, as this means the node is busy
            None | Some(None) => Err(NodesNotFoundError::from(&[id] as &[usize]).into()),
        }
    }

    /// Add a node to the registry. This returns a unique identifier for that
    /// node, which can be used to add it to a [crate::Graph]. This uses default
    /// state
    pub fn register<S: Stage + Send + 'static>(&mut self, stage: S) -> usize
    where S::State: Default + Send {
        let next = self.0.len();
        self.0.push(Some(Box::new(Node::new(stage, S::State::default()))));
        next
    }

    /// Add a node to the registry. This returns a unique identifier for that
    /// node, which can be used to add it to a [crate::Graph]
    pub fn register_with_state<S: Stage + Send + 'static>(&mut self, stage: S, state: S::State) -> usize
        where S::State: Send {
        let next = self.0.len();
        self.0.push(Some(Box::new(Node::new(stage, state))));
        next
    }

    /// Returns an error if the registry doesn't contain a node with a stage
    /// of the specified type with the given id.
    pub fn validate_node_type<S: Stage + 'static>(&self, id: usize) -> Result<(), RegistryError> {
        match self.0.get(id) {
            Some(Some(node)) => match node.as_any().downcast_ref::<Node<S>>() {
                Some(_) => Ok(()),
                None => Err(NodeTypeMismatchError{got: TypeId::of::<Node<S>>(), expected: node.as_any().type_id()}.into()),
            },
            // TODO: Handle Some(None) as a special case, as this means the node is busy
            None | Some(None) => Err(NodesNotFoundError::from(&[id] as &[usize]).into()),
        }
    }

    /// Remove a node from the registry and return it. This will return an error if
    /// S doesn't match the stage type for that node.
    pub fn unregister<S: Stage + 'static>(&mut self, id: usize) -> Result<Option<Node<S>>, RegistryError> {
        self.validate_node_type::<S>(id)?;
        match self.0.get_mut(id) {
            Some(maybe_node) => {
                maybe_node.take().map(|node| match node.into_any().downcast() {
                    Ok(node) => Ok(Some(*node)),
                    Err(node) => Err(NodeTypeMismatchError{got: TypeId::of::<Node<S>>(), expected: node.type_id()}.into()),
                }).unwrap_or(Ok(None))
            },
            None => Ok(None),
        }
    }

    /// Remove a node from the registry and drop it.
    pub fn unregister_and_drop(&mut self, id: usize) -> Result<(), RegistryError> {
        match self.0.get_mut(id).take().map(drop) {
            Some(_) => Ok(()),
            None => Err(NodesNotFoundError::from(&[id] as &[usize]).into()),
        }
    }

    /// Get a type-erased node
    pub fn get(&self, id: usize) -> Option<&Box<dyn AnyNode>> {
        match self.0.get(id) {
            Some(Some(node)) => Some(node),
            // TODO: Handle Some(None) as a special case, as this means the node is busy
            Some(None) => None,
            None => None,
        }
    }

    /// Get a mutable type-erased node
    pub fn get_mut(&mut self, id: usize) -> Option<&mut Box<dyn AnyNode>> {
        match self.0.get_mut(id) {
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
    pub fn get2_mut(
        &mut self,
        id0: usize,
        id1: usize,
    ) -> Result<(&mut Box<dyn AnyNode>, &mut Box<dyn AnyNode>), NodesNotFoundError> {
        if id0 == id1 {
            panic!("Attempted to borrow node id {id0} twice")
        }
        let first_id = std::cmp::min(id0, id1);
        let second_id = std::cmp::max(id0, id1);
        match (self.0.len() < id0, self.0.len() < id1) {
            (true, true) => return Err(NodesNotFoundError::from(&[id0, id1] as &[usize])),
            (true, false) => return Err(NodesNotFoundError::from(&[id0] as &[usize])),
            (false, true) => return Err(NodesNotFoundError::from(&[id1] as &[usize])),
            _ => (),
        }

        if let [first, .., second] = &mut self.0[first_id..=second_id] {
            match (first, second) {
                (None, None) => Err(NodesNotFoundError::from(&[id0, id1] as &[usize])),
                (None, Some(_)) => Err(NodesNotFoundError::from(&[id0] as &[usize])),
                (Some(_), None) => Err(NodesNotFoundError::from(&[id1] as &[usize])),
                (Some(first), Some(second)) => if first_id == id1 {Ok((first, second))} else {Ok((second, first))},
            }
        } else {
            unreachable!()
        }
    }

    /// Takes a node out of the registry, gaining ownership of it.
    /// This does not unregister the node, and it can be placed back
    /// inside the registry via it's ID. Returns None if the node is 
    /// already taken or never existed.
    pub fn take_node(&mut self, id: usize) -> Option<Box<dyn AnyNode>> {
        match self.0.get_mut(id) {
            Some(maybe_node) => maybe_node.take(),
            None => None,
        }
    }

    /// Insert a node with the given ID back into the registry. Panics
    /// if the node never existed at that ID in the first place.
    // TODO: This could be a more frindly error
    pub fn replace_node(&mut self, id: usize, node: Box<dyn AnyNode>) {
        *self.0.get_mut(id).unwrap() = Some(node);
    }
}
