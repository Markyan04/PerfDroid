# PerfDroid

[中文文档 (Chinese README)](README.zh-CN.md)

PerfDroid is an open-source desktop tool for Android performance profiling. Version `1.0.0` focuses on low-intrusion metric collection from the PC side through ADB, then aggregates, visualizes, and exports session data.

## What 1.0.0 Includes

- Device discovery and connection over USB / WiFi ADB
- Session controls: `Connect`, `Start`, `Pause`, `Restart`, `Stop`
- Multi-metric collection:
  - `CPU_CLOCK` (MHz)
  - `CPU_USAGE` (%)
  - `FPS`
  - `BATTERY_TEMP` (0.1C)
  - `VOLTAGE` (mV)
  - `CURRENT` (mA)
  - `POWER` (mW)
- Real-time aggregation and GUI visualization
- Session export:
  - CSV
  - JSON
  - HTML report
  - PNG report

## 1.0.0 Runtime Notes

- Sampling rate is configurable at runtime, currently clamped to `1~10 Hz`.
- Export actions are available only in `Paused` or `Stopped` session states.
- FPS sampling depends on the target Android package name configured in GUI.
- Chart interaction is supported:
  - Inspect detailed values at a specific timestamp
  - Select a time range to view range statistics (for example avg/min/max)
  - Delete data inside a selected time range


## Architecture

PerfDroid follows a 3-layer structure:

- `Profiler Layer`: metric collectors running in independent profiler modules
- `Aggregation Layer`: builds standardized `MetricBatch` payloads
- `GUI Layer`: control flow, visualization, session storage, and export

Thread model: `1 + n`

- `1` application thread for control and aggregation
- `n` profiler threads for metric sampling

![Architecture](docs/images/archtitecture.png)

## Repository Layout

```text
PerfDroid/
├── crates/
│   ├── app/
│   ├── pdcore/
│   ├── registry/
│   └── profiler/
│       ├── cpu_clock/
│       ├── cpu_usage/
│       ├── fps/
│       ├── power/
│       └── temperature/
├── docs/
│   ├── tech_doc.md
│   └── images/
├── scripts/
├── justfile
└── Cargo.toml
```

## Requirements

- Rust stable
- Cargo
- [`just`](https://github.com/casey/just)

## Common Development Commands

```bash
just --list
just check
just test
just run
just clippy
```

## Build Release Packages (Windows / Linux / macOS)

Install Rust targets first:

```bash
just install-targets
```

Package per platform:

```bash
just package-linux
just package-macos
just package-windows
```

Artifacts are generated under `dist/`, for example:

- `perfdroid-1.0.0-linux-x86_64.tar.gz`
- `perfdroid-1.0.0-macos-x86_64.tar.gz`
- `perfdroid-1.0.0-windows-x86_64.zip`

Each release package includes platform-specific bundled ADB binaries in `adb/`:

- Linux: `adb/adb`
- macOS: `adb/adb`
- Windows: `adb/adb.exe` + `AdbWinApi.dll` + `AdbWinUsbApi.dll`

## ADB Permission Notes (Linux / macOS)

If executable permission is lost after extracting archives or moving filesystems, you may see `Permission denied`. Fix with:

```bash
chmod +x adb/linux/adb adb/mac/adb
```

## License

This project is licensed under Apache-2.0. See [`LICENSE`](LICENSE).
