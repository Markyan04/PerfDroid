# PerfDroid Profiler Interface Guide

## Goal

This document defines the minimum interface and development workflow for adding a new profiler to PerfDroid.

Target readers:

- contributors who want to add a new metric such as `FPS`, `CPU_USAGE`, `TEMPERATURE`, or `POWER`
- contributors who need to adapt the existing demo to another device metric

This guide is based on the current demo implementation:

- shared core contract: [`crates/pdcore/src/traits/mod.rs`](../crates/pdcore/src/traits/mod.rs)
- shared data types: [`crates/pdcore/src/types/mod.rs`](../crates/pdcore/src/types/mod.rs)
- reference profiler: [`crates/profiler/cpu_clock/src/lib.rs`](../crates/profiler/cpu_clock/src/lib.rs)
- aggregation runtime: [`crates/app/src/aggregation/mod.rs`](../crates/app/src/aggregation/mod.rs)

## Architecture Contract

Each profiler belongs to the `Profiler Layer` and must satisfy these rules:

1. One profiler owns one metric family.
2. One profiler can contain one or more collectors.
3. Each collector writes exactly one `i64` value into an atomic buffer.
4. Invalid or unavailable data must be written as `-1`.
5. The GUI must never depend on profiler-specific shell commands or device file paths.
6. The aggregation layer reads only metadata plus the latest atomic values.

The intended data flow is:

1. Collector reads device data.
2. Collector writes to atomic buffer.
3. Aggregator reads the latest buffered values.
4. Aggregator builds `MetricBatch`.
5. GUI renders `MetricBatch`.

## Required Shared Types

All profilers must use the shared types in `pdcore`.

### CollectorMetadata

```rust
pub struct CollectorMetadata {
    pub collector_key: String,
    pub unit: String,
    pub order: usize,
}
```

Rules:

- `collector_key` must be stable and deterministic
- `unit` must be the displayed unit for that collector
- `order` must be unique inside one profiler

### ProfilerMetadata

```rust
pub struct ProfilerMetadata {
    pub profiler_key: String,
    pub collector: Vec<CollectorMetadata>,
}
```

Rules:

- `profiler_key` should be uppercase snake case, for example `CPU_CLOCK`
- `collector` order must remain stable after connection

### MetricBatch

```rust
pub struct MetricBatch {
    pub metric_key: String,
    pub unit: String,
    pub values: Vec<i64>,
}
```

Rules:

- the aggregation layer builds this type
- `values` is fixed-width and padded to length `10`
- unused positions must remain `-1`

## Required Traits

The current core interfaces are in [`pdcore::traits`](../crates/pdcore/src/traits/mod.rs).

### Collector

```rust
pub trait Collector: Send + Sync {
    fn metadata(&self) -> &CollectorMetadata;
    fn read_buffer(&self, ordering: Ordering) -> i64;
}
```

A new collector must expose:

- stable metadata
- read access to the latest atomic value

### Profiler

```rust
pub trait Profiler: Send + Sync {
    fn metadata(&self) -> &ProfilerMetadata;
    fn collectors(&self) -> Vec<&dyn Collector>;
    fn connect(&mut self) -> Result<(), CoreError>;
    fn start(&mut self) -> Result<(), CoreError>;
    fn pause(&mut self) -> Result<(), CoreError>;
    fn restart(&mut self) -> Result<(), CoreError>;
    fn stop(&mut self) -> Result<(), CoreError>;
}
```

A new profiler must implement:

- `connect`: discover device resources, build metadata, allocate collectors
- `start`: launch the sampling thread
- `pause`: stop sampling without destroying the thread context
- `restart`: resume sampling after pause
- `stop`: stop the thread, release resources, reset atomic buffers if needed

## Recommended Crate Layout

Add a new crate under `crates/profiler/<metric_name>/`.

Recommended files:

```text
crates/profiler/<metric_name>/
├── Cargo.toml
└── src/
    ├── lib.rs
    └── metadata.rs
```

Recommended contents:

- `metadata.rs`
  - `PROFILER_ID`
  - `PROFILER_KEY`
  - unit constants
- `lib.rs`
  - collector struct
  - profiler struct
  - adb or system access helpers
  - parsing helpers
  - unit tests for parser logic

## Implementation Checklist

### 1. Add the crate

Update the workspace manifest and add the new profiler crate.

### 2. Define metric constants

Example:

```rust
pub const PROFILER_ID: &str = "temperature";
pub const PROFILER_KEY: &str = "TEMPERATURE";
pub const UNIT_CELSIUS_X10: &str = "0.1°C";
```

### 3. Define collector state

Each collector should usually contain:

- `CollectorMetadata`
- source descriptor, such as a sysfs path or adb command target
- `Arc<AtomicI64>` for the latest value

Example shape:

```rust
pub struct ExampleCollector {
    metadata: CollectorMetadata,
    source_name: String,
    value: Arc<AtomicI64>,
}
```

### 4. Define profiler state

Each profiler should usually contain:

- target device identity, for example `serial: Option<String>`
- sampling interval
- `ProfilerMetadata`
- `Vec<CollectorStruct>`
- connection flag
- sampler runtime handle

Example shape:

```rust
pub struct ExampleProfiler {
    serial: Option<String>,
    sample_interval: Duration,
    metadata: ProfilerMetadata,
    collectors: Vec<ExampleCollector>,
    connected: bool,
    sampler: Option<SamplerRuntime>,
}
```

### 5. Implement `connect`

`connect` should do all one-time initialization:

- verify the device is reachable
- probe available data sources
- create collectors
- build final `ProfilerMetadata`

Do not start periodic sampling inside `connect`.

### 6. Implement `start`

`start` should:

- validate that `connect` already succeeded
- create a sampling thread
- reuse one adb session when practical
- update all collector atomic values on each tick

### 7. Implement `pause`, `restart`, `stop`

Recommended behavior:

- `pause`: set a shared pause flag
- `restart`: clear the pause flag
- `stop`: signal exit, join the thread, mark values invalid when appropriate

### 8. Parse and normalize values

Convert raw device output into the GUI-facing integer format early.

Examples:

- CPU kHz -> MHz integer
- temperature in `°C` with one decimal place -> store as `0.1°C` integer
- percentage -> integer percent

Rules:

- parse failures must return `-1`
- do not panic on malformed output

## ADB Integration Rules

If the profiler uses `adb_client`, follow the `CPU_CLOCK` pattern:

1. use `ADBServer::default()`
2. use `get_device()` or `get_device_by_name()`
3. use `shell_command(...)`
4. keep parsing logic separate from adb transport logic

Recommended separation:

- `run_shell(...)`: transport helper
- `parse_* (...)`: pure parser
- `sample_once(...)`: one complete collection pass

This separation is important because pure parser functions are easy to unit test without a device.

## Aggregation Contract

Adding a profiler is a two-step integration, not just a new crate.

### Step 1. Register metadata

When the runtime connects, register the profiler metadata in the registry.

### Step 2. Add a data-plane adapter

Create a small adapter in the aggregation layer that:

- reads the profiler’s atomic buffers
- orders values by `CollectorMetadata.order`
- constructs a `MetricBatch`

Current reference:

- [`CpuClockDataPlane`](../crates/app/src/aggregation/mod.rs)

For a future profiler, create a similar adapter:

```rust
pub struct TemperatureDataPlane;

impl TemperatureDataPlane {
    pub fn build_metric_batch(profiler: &TemperatureProfiler) -> Result<MetricBatch, CoreError> {
        // 1. read atomic values
        // 2. order them
        // 3. build MetricBatch
    }
}
```

## GUI Integration Contract

A new profiler should not directly render itself.

Instead, expose:

- `ProfilerMetadata`
- `MetricBatch`

Then the GUI can decide:

- chart title
- line count
- labels
- current value cards

This keeps the GUI generic and prevents profiler crates from depending on GPUI.

## Must-Have Tests

Each new profiler should include:

1. parser tests for raw adb or sysfs output
2. metadata construction tests
3. invalid-data tests that verify `-1` behavior

Good examples:

- one policy line
- multiple collector lines
- malformed shell output
- empty shell output

## Contributor Workflow

When adding a new profiler, follow this order:

1. create the profiler crate
2. implement and test parser helpers
3. implement `Collector` and `Profiler`
4. add aggregation adapter
5. register metadata in runtime connect flow
6. expose the metric in the GUI
7. run formatting and checks

## Reference Example

Use `CPU_CLOCK` as the reference implementation for:

- dynamic collector discovery from device policy files
- one thread per profiler
- atomic buffer updates
- aggregation into a fixed-width `MetricBatch`

Reference files:

- [`crates/profiler/cpu_clock/src/lib.rs`](../crates/profiler/cpu_clock/src/lib.rs)
- [`crates/app/src/runtime/mod.rs`](../crates/app/src/runtime/mod.rs)
- [`crates/app/src/aggregation/mod.rs`](../crates/app/src/aggregation/mod.rs)

## Non-Goals

This interface guide does not currently standardize:

- multi-profiler scheduling
- export formats
- GUI chart style configuration
- automatic device capability negotiation across all metrics

Those can be added later without changing the core profiler contract above.
