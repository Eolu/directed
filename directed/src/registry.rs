//! The registry is the "global" store of logic and state. 
use anyhow::anyhow;
use slab::Slab;
use std::any::{Any, TypeId};

use crate::{
    node::{AnyNode, Node},
    stage::Stage,
};

/// A [Registry] stores each node, its state, and the logical [Stage] 
/// associated with it.
pub struct Registry(pub(super) Slab<Box<dyn AnyNode>>);

impl Registry {
    pub fn new() -> Self {
        Self(Slab::new())
    }

    /// Add a node to the registry. This returns a unique identifier for that
    /// node, which can be used to add it to a [crate::Graph]
    pub fn register<S: Stage + 'static>(&mut self, stage: S) -> usize {
        self.0.insert(Box::new(Node::new(stage, None)))
    }

    /// Returns an error if the registry doesn't contain a node with a stage 
    /// of the specified type with the given id.
    pub fn validate_node_type<S: Stage + 'static>(&self, id: usize) -> anyhow::Result<()> {
        match self.0.get(id) {
            Some(node) => {
                if node.type_id() != TypeId::of::<Node<S>>() {
                    Err(anyhow!(
                        "Attempted to unregister invalid node type: {}. Expected: {}",
                        std::any::type_name::<Node<S>>(),
                        std::any::type_name_of_val(&*node)
                    ))
                } else {
                    Ok(())
                }
            }
            None => Err(anyhow!("Node id {id} does not exist")),
        }
    }

    /// Remove a node from the registry and return it. This will return an error if
    /// S doesn't match the stage type for that node.
    pub fn unregister<S: Stage + 'static>(&mut self, id: usize) -> anyhow::Result<Option<Node<S>>> {
        self.validate_node_type::<S>(id)?;
        match self.0.try_remove(id) {
            Some(node) => match node.into_any().downcast() {
                Ok(node) => Ok(Some(*node)),
                Err(node) => Err(anyhow!(
                    "Type conversion error: {}. Expected: {}",
                    std::any::type_name::<Node<S>>(),
                    std::any::type_name_of_val(&*node)
                )),
            },
            None => Ok(None),
        }
    }

    /// Remove a node from the registry and drop it.
    pub fn unregister_and_drop(&mut self, id: usize) -> anyhow::Result<()> {
        match self.0.try_remove(id).map(drop) {
            Some(_) => Ok(()),
            None => Err(anyhow!("Node id {id} does not exist")),
        }
    }

    /// Get a type-erased node
    pub fn get(&self, id: usize) -> anyhow::Result<&Box<dyn AnyNode>> {
        match self.0.get(id) {
            Some(node) => Ok(node),
            None => Err(anyhow!("Node id {id} does not exist")),
        }
    }

    /// Get a mutable type-erased node
    pub fn get_mut(&mut self, id: usize) -> anyhow::Result<&mut Box<dyn AnyNode>> {
        match self.0.get_mut(id) {
            Some(node) => Ok(node),
            None => Err(anyhow!("Node id {id} does not exist")),
        }
    }

    /// Get 2 mutable type-erased nodes
    /// 
    /// This is an important internal detail: a parent a child node often need
    /// to be modified together.
    pub fn get2_mut(
        &mut self,
        id0: usize,
        id1: usize,
    ) -> anyhow::Result<(&mut Box<dyn AnyNode>, &mut Box<dyn AnyNode>)> {
        match self.0.get2_mut(id0, id1) {
            Some((node0, node1)) => Ok((node0, node1)),
            None => Err(anyhow!("Either node id {id0} OR {id1} do not exist")),
        }
    }
}
