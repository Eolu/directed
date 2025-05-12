use std::{any::Any, collections::HashMap, sync::Arc};

use crate::{node::UNNAMED_OUTPUT_NAME, NodeId};

/// Simple macro to simulate a function that can return mutlieple names outputs
#[macro_export]
macro_rules! output {
    ($($name:ident $(: $val:expr)?),*) => {
        directed::NodeOutput::new()
        $(
            .add(stringify!($name), output!(@internal $name $(, $val)?))
        )*
    };
    (@internal $name:ident, $val:expr) => { $val };
    (@internal $name:ident) => { $name };
}

/// Represents one or more outputs of a node. This can be used as a return type
/// to represent a function with multiple outputs. The [output] macro is the
/// preferred way to construct this in that case. Any more direct interaction
/// with this type is not recommended.
#[derive(Debug)]
pub enum NodeOutput {
    Standard(Arc<dyn Any + Send + Sync>),
    Named(HashMap<DataLabel, Arc<dyn Any + Send + Sync>>),
}

impl NodeOutput {
    pub fn new() -> Self {
        Self::Named(HashMap::new())
    }

    /// Wraps a single return value. Note that calling "add" on this afterwards
    /// will result in a panic.
    pub fn new_simple<T: Send + Sync + 'static>(value: T) -> Self {
        Self::Standard(Arc::new(value))
    }

    /// Wraps a single return value. Note that calling "add" on this afterwards
    /// will result in a panic.
    pub fn dyn_new_simple(value: Arc<dyn Any + Send + Sync>) -> Self {
        Self::Standard(value)
    }

    /// Adds an additional output. Will panic if this was instantiated to
    /// represent a single output.
    pub fn add<T: Send + Sync + 'static>(mut self, name: &str, value: T) -> Self {
        match &mut self {
            // This panic is the primary reason direct interaction with this
            // type is not recommended
            NodeOutput::Standard(_) => panic!("Attempted to add '{name}' to a simple output"),
            NodeOutput::Named(hash_map) => {
                hash_map.insert(DataLabel::new(name), Arc::new(value));
            }
        }
        self
    }

    /// Adds an additional output. Will panic if this was instantiated to
    /// represent a single output.
    pub fn dyn_add(mut self, name: &str, value: Arc<dyn Any + Send + Sync>) -> Self {
        match &mut self {
            // This panic is the primary reason direct interaction with this
            // type is not recommended
            NodeOutput::Standard(_) => panic!("Attempted to add '{name}' to a simple output"),
            NodeOutput::Named(hash_map) => {
                hash_map.insert(DataLabel::new(name), value);
            }
        }
        self
    }
}

/// Associates a name with a type, internal detail
#[derive(Debug, Clone, Eq, PartialOrd, Ord)]
pub struct DataLabel {
    pub(crate) name: Option<std::borrow::Cow<'static, str>>,
    /// This is for informational purposes, discluded from PartialEq and Hash
    pub(crate) type_name: Option<std::borrow::Cow<'static, str>>,
}

impl PartialEq for DataLabel {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl std::hash::Hash for DataLabel {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

impl DataLabel {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: Some(std::borrow::Cow::from(name.into())),
            type_name: None,
        }
    }

    pub const fn new_const(name: &'static str) -> Self {
        Self {
            name: Some(std::borrow::Cow::Borrowed(name)),
            type_name: None,
        }
    }

    /// Type names are included to make error messages more meaningful.
    /// They make no semantic difference.
    pub const fn new_with_type_name(name: &'static str, type_name: &'static str) -> Self {
        Self {
            name: Some(std::borrow::Cow::Borrowed(name)),
            type_name: Some(std::borrow::Cow::Borrowed(type_name)),
        }
    }

    /// A blank datalabel is a placeholder used for a connection between nodes
    /// with no associated data
    pub const fn new_blank() -> Self {
        Self {
            name: None,
            type_name: None,
        }
    }

    pub fn inner(&self) -> Option<&str> {
        self.name.as_ref().map(|inner| inner.as_ref())
    }
}

impl From<&str> for DataLabel {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

/// Used to represent graph outputs.
pub struct GraphOutput(HashMap<NodeId, HashMap<DataLabel, Arc<dyn Any + Send + Sync>>>);

impl GraphOutput {
    pub(crate) fn new() -> Self {
        Self(HashMap::new())
    }

    pub(crate) fn insert(&mut self, node_id: NodeId, data_label: impl Into<DataLabel>, item: Arc<dyn Any + Send + Sync>) {
        if self.0.get(&node_id).is_none() {
            self.0.insert(node_id, HashMap::new());
        }
        self.0.get_mut(&node_id).unwrap().insert(data_label.into(), item);
    }

    /// Gets an output from a particular node
    pub fn get<T: Any + Send + Sync>(&self, node_id: NodeId, data_label: impl Into<DataLabel>) -> Option<&T> {
        // TODO: More in-depth error reporting here
        self.0.get(&node_id).and_then(|map| map.get(&data_label.into())).and_then(|arc| arc.downcast_ref())
    }

    /// Gets an unnamed output from a particular node
    /// 
    /// Note that the unnamed output can also be accessed via the name '_'
    pub fn get_unnamed<T: Any + Send + Sync>(&self, node_id: NodeId) -> Option<&T> {
        // TODO: More in-depth error reporting here
        self.0.get(&node_id).and_then(|map| map.get(&UNNAMED_OUTPUT_NAME)).and_then(|arc| arc.downcast_ref())
    }

    /// Attempts to take and remove a specific output
    pub fn take<T: Any + Send + Sync>(&mut self, node_id: NodeId, data_label: impl Into<DataLabel>) -> Option<T> {
        // TODO: More in-depth error reporting here
        self.0.get_mut(&node_id).and_then(|map| map.remove(&data_label.into())).and_then(|arc| {
            arc.downcast().ok().and_then(|arc| Arc::into_inner(arc))
        })
    }

    /// Attempts to take and remove the output.
    /// 
    /// Note that the unnamed output can also be accessed via the name '_'
    pub fn take_unnamed<T: Any + Send + Sync>(&mut self, node_id: NodeId) -> Option<T> {
        // TODO: More in-depth error reporting here
        self.0.get_mut(&node_id).and_then(|map| map.remove(&UNNAMED_OUTPUT_NAME)).and_then(|arc| {
            arc.downcast().ok().and_then(|arc| Arc::into_inner(arc))
        })
    }
}