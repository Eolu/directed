//! Type-erased port values and the capability table used by the caching engine.

use std::any::{Any, TypeId, type_name};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// A type-erased value flowing along a graph edge.
pub type Value = Arc<dyn Any + Send + Sync>;

/// Optional comparison capabilities a port's type can provide. The `#[stage]`
/// macro emits only what the node's [`crate::CachePolicy`] needs, so opaque
/// stages may use port types that are neither `PartialEq` nor `Hash`.
#[derive(Clone, Copy)]
pub struct ValueOps {
    pub type_id: TypeId,
    pub type_name: &'static str,
    pub eq_fn: Option<fn(&Value, &Value) -> bool>,
    pub hash_fn: Option<fn(&Value) -> u64>,
}

impl ValueOps {
    /// No capabilities. Used by [`CachePolicy::None`](crate::CachePolicy).
    pub fn opaque<T: Send + Sync + 'static>() -> Self {
        Self {
            type_id: TypeId::of::<T>(),
            type_name: type_name::<T>(),
            eq_fn: None,
            hash_fn: None,
        }
    }

    /// Equality. Used by [`CachePolicy::Last`](crate::CachePolicy).
    pub fn eq<T: PartialEq + Send + Sync + 'static>() -> Self {
        Self {
            type_id: TypeId::of::<T>(),
            type_name: type_name::<T>(),
            eq_fn: Some(eq_impl::<T>),
            hash_fn: None,
        }
    }

    /// Equality + hash. Used by [`CachePolicy::All`](crate::CachePolicy).
    pub fn eq_hash<T: PartialEq + Hash + Send + Sync + 'static>() -> Self {
        Self {
            type_id: TypeId::of::<T>(),
            type_name: type_name::<T>(),
            eq_fn: Some(eq_impl::<T>),
            hash_fn: Some(hash_impl::<T>),
        }
    }

    /// Value equality, if the type is comparable.
    pub fn eq_values(&self, a: &Value, b: &Value) -> Option<bool> {
        self.eq_fn.map(|f| f(a, b))
    }

    /// A fast hash of the value, if the type is hashable.
    pub fn hash_value(&self, value: &Value) -> Option<u64> {
        self.hash_fn.map(|f| f(value))
    }
}

fn eq_impl<T: PartialEq + Send + Sync + 'static>(a: &Value, b: &Value) -> bool {
    a.downcast_ref::<T>() == b.downcast_ref::<T>()
}

fn hash_impl<T: Hash + Send + Sync + 'static>(value: &Value) -> u64 {
    let mut hasher = DefaultHasher::new();
    value
        .downcast_ref::<T>()
        .expect("port type is validated against the static signature")
        .hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_has_no_capabilities() {
        let ops = ValueOps::opaque::<i32>();
        assert!(ops.eq_fn.is_none());
        assert!(ops.hash_fn.is_none());
    }

    #[test]
    fn eq_roundtrip() {
        let ops = ValueOps::eq::<String>();
        let a: Value = Arc::new(String::from("hello"));
        let b: Value = Arc::new(String::from("hello"));
        assert_eq!(ops.eq_values(&a, &b), Some(true));
        assert!(ops.hash_value(&a).is_none());
    }

    #[test]
    fn eq_hash_roundtrip() {
        let ops = ValueOps::eq_hash::<u32>();
        let a: Value = Arc::new(7u32);
        let b: Value = Arc::new(7u32);
        assert_eq!(ops.eq_values(&a, &b), Some(true));
        assert_eq!(ops.hash_value(&a), ops.hash_value(&b));
    }
}
