# PerfDroid

PerfDroid 是一个面向 Android 设备性能分析场景的开源桌面工具。`1.0.0` 版本聚焦于在 PC 侧通过 ADB 进行低侵入式指标采集，并完成聚合、可视化与导出。

## 1.0.0 已实现能力

- 设备发现与连接（USB / WiFi ADB）
- 会话控制：`Connect`、`Start`、`Pause`、`Restart`、`Stop`
- 多指标采集：
  - `CPU_CLOCK`（MHz）
  - `CPU_USAGE`（%）
  - `FPS`
  - `BATTERY_TEMP`（0.1C）
  - `VOLTAGE`（mV）
  - `CURRENT`（mA）
  - `POWER`（mW）
- 实时数据聚合与 GUI 展示
- 会话导出：
  - CSV
  - JSON
  - HTML 报告
  - PNG 报告

## 1.0.0 运行时说明

- 采样频率支持运行时调整，当前限制为 `1~10 Hz`。
- 导出动作仅在 `Paused` 或 `Stopped` 状态可用。
- FPS 采样依赖 GUI 中配置的目标应用包名。
- 会话数据支持导出前按时间区间删除。
- 已支持图表交互能力：
  - 查看某一时间点的详细指标数据
  - 选择某一时间区间并查看统计数据（如平均值/最小值/最大值）
  - 删除选中时间区间内的数据

## 架构概览

PerfDroid 采用三层结构：

- `Profiler Layer`：各指标采集器独立运行
- `Aggregation Layer`：统一组装标准化 `MetricBatch`
- `GUI Layer`：控制流程、可视化、会话管理与导出

线程模型为 `1 + n`：

- `1` 个应用线程负责控制与聚合
- `n` 个 profiler 线程负责指标采样

![Architecture](docs/images/archtitecture.png)

## 仓库结构

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

## 环境要求

- Rust stable
- Cargo
- [`just`](https://github.com/casey/just)

## 常用开发命令

```bash
just --list
just check
just test
just run
just clippy
```

## 构建发布包（Windows / Linux / macOS）

先安装 Rust 目标：

```bash
just install-targets
```

分别打包：

```bash
just package-linux
just package-macos
just package-windows
```

产物默认输出在 `dist/`，示例：

- `perfdroid-1.0.0-linux-x86_64.tar.gz`
- `perfdroid-1.0.0-macos-x86_64.tar.gz`
- `perfdroid-1.0.0-windows-x86_64.zip`

每个发布包都包含对应平台的 ADB 可执行文件（位于包内 `adb/` 目录）：

- Linux: `adb/adb`
- macOS: `adb/adb`
- Windows: `adb/adb.exe` + `AdbWinApi.dll` + `AdbWinUsbApi.dll`

## ADB 权限说明（Linux / macOS）

如果从压缩包解压或跨文件系统复制后丢失可执行权限，可能出现 `Permission denied`。可执行：

```bash
chmod +x adb/linux/adb adb/mac/adb
```

## License

本项目基于 Apache-2.0 License 开源，详见 [`LICENSE`](LICENSE)。
