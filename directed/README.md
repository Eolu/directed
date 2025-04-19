# directed

A Rust library for building directed acyclic graph (DAG) based computation pipelines with declarative data flow.

## Core Concepts

### Stages

A Stage defines . Use the `#[stage]` macro to convert a regular function into a stage:

```rust
#[stage]
fn simple_stage(input: String) -> usize {
    input.len()
}

// Lazy: only evaluated when its output is needed by another node
#[stage(lazy)]
fn lazy_stage(a: i32, b: i32) -> i32 {
    a + b
}

// Transparent: caches results and skips reevaluation if inputs haven't changed
#[stage(lazy, transparent)]
fn cached_stage(input: String) -> String {
    println!("Computing cached_stage...");  // This will only print if input changes
    input.to_uppercase()
}

// Multiple named outputs are supported
// TODO: out(result_1: f64, result_2: String)
#[stage(lazy, transparent, out(result_1: f64), out(result_2: String))]
fn efficient_stage(x: f64) -> NodeOutput {
    output!{
        result_1: x.sqrt(),
        result_2: format!("{x}")
    }
}
```

### Nodes and Registry

Nodes are instantiated stages managed by a Registry:

```rust
// Create a registry to store nodes
let mut registry = Registry::new();

// Register stages as nodes and get their unique IDs
let converter_node = registry.register(default_stage::new());
let processor_node = registry.register(cached_stage::new());
```

### Graphs

A Graph defines connections between nodes:

```rust
// Create a graph connecting nodes
let graph = graph! {
    nodes: &[converter_node, processor_node],
    connections: {
        // Connect default_stage's unnamed output to cached_stage's "input"
        converter_node => "_" => "input" => processor_node,
    }
}?;

// Execute the graph
graph.execute(&mut registry)?;
```

- The `"_"` name represents a function's unnamed (single) return value
- Named outputs (from NodeOutput) use their explicit names in connections

## Complete Example

Here's a complete example showing various types of stages connected in a graph:

```rust
use directed::*;

// Basic stage with default behavior (eager evaluation, no caching)
#[stage]
fn generate_data() -> String {
    println!("Generating data...");
    String::from("Hello, directed!")
}

// Transparent stage that caches results
#[stage(transparent)]
fn process_data(input: String) -> String {
    println!("Processing data...");  // Only executes when input changes
    input.to_uppercase()
}

// Stage with multiple named outputs
#[stage(out(chars: Vec<char>), out(word_count: usize))]
fn analyze_text(text: String) -> NodeOutput {
    println!("Analyzing text...");
    let chars = text.chars().collect();
    
    output! {
        chars,
        word_count: text.split_whitespace().count()
    }
}

// Lazy stage that only executes when needed
#[stage(lazy)]
fn format_chars(chars: Vec<char>) -> String {
    println!("Formatting characters...");
    chars.into_iter().collect()
}

// Final stage that forces execution of the graph
#[stage]
fn print_results(uppercase: String, word_count: usize, formatted: String) {
    println!("Results:");
    println!("  Uppercase: {}", uppercase);
    println!("  Word count: {}", word_count);
    println!("  Formatted: {}", formatted);
}

fn main() -> anyhow::Result<()> {
    // Create registry and register all nodes
    let mut registry = Registry::new();
    let generator = registry.register(generate_data::new());
    let processor = registry.register(process_data::new());
    let analyzer = registry.register(analyze_text::new());
    let formatter = registry.register(format_chars::new());
    let printer = registry.register(print_results::new());
    
    // Build the graph with connections
    let graph = graph! {
        nodes: &[generator, processor, analyzer, formatter, printer],
        connections: {
            // Connect generator to processor and analyzer
            generator => "_" => "input" => processor,
            generator => "_" => "text" => analyzer,
            
            // Connect analyzer's named outputs
            analyzer => "chars" => "chars" => formatter,
            analyzer => "word_count" => "word_count" => printer,
            
            // Connect remaining outputs to the printer
            processor => "_" => "uppercase" => printer,
            formatter => "_" => "formatted" => printer,
        }
    }?;
    
    // Execute the graph
    graph.execute(&mut registry)?;
    
    Ok(())
}
```

## How Execution Works

1. When you call `graph.execute()`, the library searches for non-lazy nodes
2. Starting from those nodes, it:
   - Recursively evaluates upstream dependencies
   - Passes data from parent nodes to child nodes
   - Determines if a node needs to be reevaluated based on its caching strategy
   - Executes nodes in the correct order to satisfy all dependencies

## Upcoming Features

- Serializing node state between runs
- Better visualization and debugging tools
- More caching strategies, including `CacheAll` to remember all input/output combinations
- Support for creating nodes out of a Graph+Registry pair
