//! The registry is the "global" store of logic and state.
use crate::{
    NodeTypeMismatchError, NodesNotFoundError, RegistryError,
    node::{AnyNode, Node},
    stage::{Stage, StageShape},
};
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
pub struct Registry(pub(super) Vec<Option<Box<dyn AnyNode>>>);

impl Registry {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// Get a reference to the state of a specific node.
    pub fn state<S: Stage + 'static>(&self, id: NodeId<S>) -> Result<&S::State, RegistryError> {
        self.validate_node_type::<S>((&id).into())?;
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
        self.validate_node_type::<S>((&id).into())?;
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
    pub fn validate_node_type<S: Stage + 'static>(
        &self,
        id: NodeReflection,
    ) -> Result<(), RegistryError> {
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
    pub fn unregister_and_drop<S: Stage + 'static>(
        &mut self,
        id: NodeId<S>,
    ) -> Result<(), RegistryError> {
        match self.0.get_mut(id.0).take().map(drop) {
            Some(_) => Ok(()),
            None => Err(NodesNotFoundError::from(&[id.into()] as &[NodeReflection]).into()),
        }
    }

    /// Get a type-erased node
    pub fn get(&self, id: impl Into<NodeReflection>) -> Option<&Box<dyn AnyNode>> {
        match self.0.get(id.into().id) {
            Some(Some(node)) => Some(node),
            // TODO: Handle Some(None) as a special case, as this means the node is busy
            Some(None) => None,
            None => None,
        }
    }

    /// Get a mutable type-erased node
    pub fn get_mut(&mut self, id: impl Into<NodeReflection>) -> Option<&mut Box<dyn AnyNode>> {
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
    pub fn get2_mut(
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

    /// Takes a node out of the registry, gaining ownership of it.
    /// This does not unregister the node, and it can be placed back
    /// inside the registry via it's ID. Returns None if the node is
    /// already taken or never existed.
    pub fn take_node(&mut self, id: NodeReflection) -> Option<Box<dyn AnyNode>> {
        match self.0.get_mut(id.id) {
            Some(maybe_node) => maybe_node.take(),
            None => None,
        }
    }

    /// Insert a node with the given ID back into the registry. Panics
    /// if the node never existed at that ID in the first place.
    // TODO: This could be a more friendly error
    pub fn replace_node(&mut self, id: NodeReflection, node: Box<dyn AnyNode>) {
        *self.0.get_mut(id.id).unwrap() = Some(node);
    }
}
