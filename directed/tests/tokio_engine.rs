#![cfg(feature = "tokio")]

use std::sync::Arc;

use directed::{Registry, StageHandle};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn execute_on_multi_thread_runtime() {
    #[directed::stage(lazy)]
    async fn Left() -> i32 {
        20
    }

    #[directed::stage(lazy)]
    async fn Right() -> i32 {
        22
    }

    #[directed::stage]
    fn Sink(a: i32, b: i32) {
        assert_eq!(a + b, 42);
    }

    let mut registry = Registry::new();
    let left = registry.register::<Left>();
    let right = registry.register::<Right>();
    let sink = registry.register::<Sink>();
    let sink_id = sink.id();

    let graph = directed::graph! {
        nodes: [left, right, sink],
        connections: {
            left: out => sink: a,
            right: out => sink: b,
        }
    }
    .unwrap();

    let outputs = graph
        .execute_tokio(Arc::new(registry), &[sink_id])
        .await
        .unwrap();
    assert!(outputs.get::<()>(sink_id, 0).is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn execute_tokio_propagates_stage_errors() {
    #[directed::stage]
    fn Broken(_input: i32) {}

    let mut registry = Registry::new();
    let broken = registry.register::<Broken>();

    // `Broken` has an unconnected input, so evaluation must fail.
    let graph = directed::graph! {
        nodes: [broken],
        connections: {}
    }
    .unwrap();

    let result = graph
        .execute_tokio(Arc::new(registry), &[broken.id()])
        .await;
    assert!(result.is_err());
}
