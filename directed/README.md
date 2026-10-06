# directed

A directed-acyclic-graph execution engine for Rust. Wrap a function with
`#[stage]` to turn it into a statically-typed node definition, register it in a
`Registry`, and wire nodes together with `graph!`. Execution walks to the
nearest *urgent* nodes, evaluating dependencies on demand and reusing cached
results when inputs have not changed.

```rust
use directed::{Registry, graph, stage};

#[stage(lazy, cache_last)]
fn Source() -> i32 {
    21
}

#[stage(lazy)]
fn Double(input: i32) -> i32 {
    input * 2
}

#[stage]
fn Sink(value: i32) {
    assert_eq!(value, 42);
}

fn main() {
    let mut registry = Registry::new();
    let source = registry.register::<Source>();
    let double = registry.register::<Double>();
    let sink = registry.register::<Sink>();

    let graph = graph! {
        nodes: [source, double, sink],
        connections: {
            source: out => double: input,
            double: out => sink: value,
        }
    }
    .unwrap();

    graph.execute(&registry).unwrap();
}
```

## Design

- A **`Stage`** is a wrapped function. The `#[stage]` macro emits a marker type,
  a typed handle, and a small `Stage` impl. All caching and data-flow logic
  lives in the runtime, not in generated code.
- A **`Registry`** owns node state. It is separate from a `Graph`, so any number
  of graphs can share one registry and connections can be rewired at runtime
  without losing state.
- A **`Graph`** stores only connectivity. Execution is stateless and takes
  `&Registry` (node state uses interior mutability so independent nodes run
  concurrently).
- Connections are type-checked at graph construction: `PortOut<T>` and
  `PortIn<T>` must share `T`, and `graph!` fails to compile otherwise.

## Stages

### Outputs

A single return value is published on the implicit output port `out`. Declare
named outputs with `out(...)`; the function then returns a tuple in the same
order:

```rust
#[stage(out(number: i32, text: String))]
fn Produce() -> (i32, String) {
    (42, String::from("hello"))
}
```

Connect them with `produce: number` and `produce: text`.

### Evaluation strategy

- `lazy` nodes run only when an urgent descendant needs their output.
- Non-lazy (default) nodes are **urgent** and are the entry points of an
  execution. A graph with no urgent node does nothing.

### Caching

- `cache_last` (transparent) reuses the previous outputs when the inputs are
  unchanged. Requires the input types to be `PartialEq` (enforced at compile
  time); owned inputs must also be `Clone`.
- `cache_all` memoizes every distinct input combination. Inputs must also be
  `Hash`.
- No attribute (opaque) means the node runs on every execution.

### State

Stages may carry arbitrary per-node state. It is available as `state` inside the
function body:

```rust
#[stage(state(u32))]
fn Counter() {
    *state += 1;
}

let node = registry.register_with_state::<Counter>(0);
```

`registry.register::<S>()` uses `S::State::default()`.

### Async

`async fn` stages are supported. `execute_async` evaluates independent nodes
concurrently on the current thread; the synchronous `execute` wraps it with
`block_on`:

```rust
#[stage]
async fn Fetch(url: String) -> String {
    // ...
    url
}

let sink = registry.register::<Sink>();
let outputs = directed::block_on(graph.execute_async(&registry, &[sink.id()])).unwrap();
```

With the `tokio` feature, `execute_tokio` spawns each ready node onto a
multi-thread runtime for real parallelism:

```rust
use std::sync::Arc;

let outputs = graph
    .execute_tokio(Arc::new(registry), &[sink.id()])
    .await
    .unwrap();
```

### Mutable inputs

A `&mut T` parameter receives a local copy of the input and may mutate it; the
change is not visible to the producing node. The type must be `Clone`.

## Graph construction and rewiring

```rust
use directed::GraphBuilder;

let mut builder = GraphBuilder::new();
builder.add(&node_a);
builder.add(&node_b);

// Typed connection: the compiler checks that both ports carry the same type.
builder.connect(node_a.out(), node_b.input()).unwrap();

// Name-based connection, validated at runtime: useful for rewiring.
builder.connect_by_name(node_a.id(), "out", node_b.id(), "input", &registry).unwrap();

let graph = builder.build();
```

Cycles are rejected when an edge is added.

## Diagnostics

`Graph::trace` snapshots a graph (optionally highlighting nodes and edges) and
`Trace::mermaid` renders it as a Mermaid flowchart:

```rust
let trace = graph.trace(&registry, &[sink.id()], &[]);
println!("```mermaid\n{}\n```", trace.mermaid());
```

