use std::{any::Any, collections::HashMap};

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
pub enum NodeOutput {
    Standard(Box<dyn Any>),
    Named(HashMap<DataLabel, Box<dyn Any>>),
}

impl NodeOutput {
    pub fn new() -> Self {
        Self::Named(HashMap::new())
    }

    /// Wraps a single return value. Note that calling "add" on this afterwards
    /// will result in a panic.
    pub fn new_simple<T: 'static>(value: T) -> Self {
        Self::Standard(Box::new(value))
    }

    /// Adds an additional output. Will panic if this was instantiated to
    /// represent a single output.
    pub fn add<T: 'static>(mut self, name: &str, value: T) -> Self {
        match &mut self {
            // This panic is the primary reason direct interaction with this 
            // type is not recommended
            NodeOutput::Standard(_) => panic!("Attempted to add '{name}' to a simple output"),
            NodeOutput::Named(hash_map) => {
                hash_map.insert(DataLabel::new(name), Box::new(value));
            }
        }
        self
    }
}

/// Associates a name with a type, internal detail
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DataLabel(std::borrow::Cow<'static, str>);
impl DataLabel {
    pub fn new(name: impl Into<String>) -> Self {
        Self(std::borrow::Cow::from(name.into()))
    }

    pub const fn new_const(name: &'static str) -> Self {
        Self(std::borrow::Cow::Borrowed(name))
    }

    pub fn inner(&self) -> &str {
        self.0.as_ref()
    }
}

impl From<&str> for DataLabel {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}
