//! The registry is the "global" store of logic and state.
use crate::{
    node::{AnyNode, Node}, stage::Stage, NodeTypeMismatchError, NodesNotFoundError, RegistryError
};
use std::{any::{Any, TypeId}, marker::PhantomData};

/// Used to access nodes within the registry
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct NodeId<S: Stage>(pub(crate) usize, PhantomData<S>);

impl<S: Stage> From<&NodeId<S>> for usize {
    fn from(value: &NodeId<S>) -> Self {
        value.0
    }
}

impl<S: Stage> From<NodeId<S>> for usize {
    fn from(value: NodeId<S>) -> Self {
        value.0
    }
}

impl<S: Stage> From<usize> for NodeId<S> {
    fn from(value: usize) -> Self {
        Self(value, PhantomData)
    }
}

/// A [Registry] stores each node, its state, and the logical [Stage]
/// associated with it.
pub struct Registry(pub(super) Vec<Option<Box<dyn AnyNode>>>);

impl Registry {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// Get a reference to the state of a specific node.
    pub fn state<S: Stage + 'static>(&self, id: NodeId<S>) -> Result<&S::State, RegistryError> {
        self.validate_node_type::<S>(id.clone())?;
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
            None | Some(None) => Err(NodesNotFoundError::from(&[id.0] as &[usize]).into()),
        }
    }

    /// Get a mutable reference to the state of a specific node.
    pub fn state_mut<S: Stage + 'static>(
        &mut self,
        id: NodeId<S>,
    ) -> Result<&mut S::State, RegistryError> {
        self.validate_node_type::<S>(id.clone())?;
        match self.0.get_mut(id.0) {
            Some(Some(any_node)) => {
                let node_type_id = (*any_node).type_id();
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
            None | Some(None) => Err(NodesNotFoundError::from(&[id.0] as &[usize]).into()),
        }
    }

    /// Add a node to the registry. This returns a unique identifier for that
    /// node, which can be used to add it to a [crate::Graph]. This uses default
    /// state
    pub fn register<S: Stage + Send + 'static>(&mut self, stage: S) -> NodeId<S>
    where
        S::State: Default + Send,
    {
        let next = self.0.len();
        self.0
            .push(Some(Box::new(Node::new(stage, S::State::default()))));
        NodeId(next, PhantomData)
    }

    /// Add a node to the registry. This returns a unique identifier for that
    /// node, which can be used to add it to a [crate::Graph].
    /// 
    /// [crate::stage::Stage::State] is taken as a tuple of all the state
    /// parameters in the order they were defined.
    pub fn register_with_state<S: Stage + Send + 'static>(
        &mut self,
        stage: S,
        state: S::State,
    ) -> NodeId<S>
    where
        S::State: Send,
    {
        let next = self.0.len();
        self.0.push(Some(Box::new(Node::new(stage, state))));
        NodeId(next, PhantomData)
    }

    /// Returns an error if the registry doesn't contain a node with a stage
    /// of the specified type with the given id.
    pub fn validate_node_type<S: Stage + 'static>(&self, id: NodeId<S>) -> Result<(), RegistryError> {
        match self.0.get(id.0) {
            Some(Some(node)) => match node.as_any().downcast_ref::<Node<S>>() {
                Some(_) => Ok(()),
                None => Err(NodeTypeMismatchError {
                    got: TypeId::of::<Node<S>>(),
                    expected: node.type_id(),
                }
                .into()),
            },
            // TODO: Handle Some(None) as a special case, as this means the node is busy
            None | Some(None) => Err(NodesNotFoundError::from(&[id.0] as &[usize]).into()),
        }
    }

    /// Remove a node from the registry and return it. This will return an error if
    /// S doesn't match the stage type for that node.
    pub fn unregister<S: Stage + 'static>(
        &mut self,
        id: NodeId<S>,
    ) -> Result<Option<Node<S>>, RegistryError> {
        self.validate_node_type::<S>(id.clone())?;
        match self.0.get_mut(id.0) {
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
    pub fn unregister_and_drop<S: Stage>(&mut self, id: NodeId<S>) -> Result<(), RegistryError> {
        match self.0.get_mut(id.0).take().map(drop) {
            Some(_) => Ok(()),
            None => Err(NodesNotFoundError::from(&[id.0] as &[usize]).into()),
        }
    }

    /// Get a node
    pub fn get<S: Stage + 'static>(&self, id: NodeId<S>) -> Option<&Node<S>> {
        match self.0.get(id.0) {
            Some(Some(node)) => node.as_any().downcast_ref(),
            // TODO: Handle Some(None) as a special case, as this means the node is busy
            Some(None) => None,
            None => None,
        }
    }

    /// Get a mutable node
    pub fn get_mut<S: Stage + 'static>(&mut self, id: NodeId<S>) -> Option<&mut Node<S>> {
        match self.0.get_mut(id.0) {
            Some(Some(node)) => node.as_any_mut().downcast_mut(),
            // TODO: Handle Some(None) as a special case, as this means the node is busy
            Some(None) => None,
            None => None,
        }
    }

    /// Get a type-erased node
    pub fn get_any(&self, id: impl Into<usize>) -> Option<&Box<dyn AnyNode>> {
        match self.0.get(id.into()) {
            Some(Some(node)) => Some(node),
            // TODO: Handle Some(None) as a special case, as this means the node is busy
            Some(None) => None,
            None => None,
        }
    }

    /// Get a mutable type-erased node
    pub fn get_any_mut(&mut self, id: impl Into<usize>) -> Option<&mut Box<dyn AnyNode>> {
        match self.0.get_mut(id.into()) {
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
    pub fn get_any_2_mut(
        &mut self,
        id0: impl Into<usize> + Copy,
        id1: impl Into<usize> + Copy,
    ) -> Result<(&mut Box<dyn AnyNode>, &mut Box<dyn AnyNode>), NodesNotFoundError> {
        if id0.into() == id1.into() {
            panic!("Attempted to borrow node id {} twice", id0.into())
        }
        let first_id = std::cmp::min(id0.into(), id1.into());
        let second_id = std::cmp::max(id0.into(), id1.into());
        match (self.0.len() < id0.into(), self.0.len() < id1.into()) {
            (true, true) => return Err(NodesNotFoundError::from(&[id0.into(), id1.into()] as &[usize])),
            (true, false) => return Err(NodesNotFoundError::from(&[id0.into()] as &[usize])),
            (false, true) => return Err(NodesNotFoundError::from(&[id1.into()] as &[usize])),
            _ => (),
        }

        if let [first, .., second] = &mut self.0[first_id..=second_id] {
            match (first, second) {
                (None, None) => Err(NodesNotFoundError::from(&[id0.into(), id1.into()] as &[usize])),
                (None, Some(_)) => Err(NodesNotFoundError::from(&[id0.into()] as &[usize])),
                (Some(_), None) => Err(NodesNotFoundError::from(&[id1.into()] as &[usize])),
                (Some(first), Some(second)) => {
                    if first_id == id1.into() {
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

    /// Takes a node out of the registry, gaining ownership of it.
    /// This does not unregister the node, and it can be placed back
    /// inside the registry via it's ID. Returns None if the node is
    /// already taken or never existed.
    pub fn take_node<S: Stage + 'static>(&mut self, id: NodeId<S>) -> Option<Node<S>> {
        match self.0.get_mut(id.0) {
            Some(maybe_node) => maybe_node.take().and_then(|node| node.into_any().downcast().ok().map(|b|*b)),
            None => None,
        }
    }

    /// Insert a node with the given ID back into the registry. Panics
    /// if the node never existed at that ID in the first place.
    // TODO: This could be a more friendly error
    pub fn replace_node(&mut self, id: impl Into<usize>, node: Box<dyn AnyNode>) {
        *self.0.get_mut(id.into()).unwrap() = Some(node);
    }
}
