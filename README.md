# PerfDroid

PerfDroid 是一个面向 Android 设备性能分析场景的开源桌面工具，目标是在 PC 侧通过 ADB 对设备进行低侵入式指标采集、聚合、可视化与导出。项目当前采用 Rust workspace 组织，围绕可扩展的 profiler 架构构建。

## 项目简介

移动应用，尤其是移动游戏，对 FPS、CPU 频率、CPU 利用率、温度和功耗等指标非常敏感。PerfDroid 希望提供一个免费、开源、可扩展的 Android 性能测试基础设施，用于替代闭源或商业化工具在教学和研究场景中的限制。

和直接运行在手机上的性能工具不同，PerfDroid 的核心思路是将采集链路尽量放在 PC 侧完成：

- 通过 ADB 与 Android 设备通信
- 由独立的 profiler 模块采集不同指标
- 统一聚合为标准化数据结构
- 在上层完成可视化、会话管理和导出

## 计划支持的能力

- Android 设备识别与连接
- 会话控制：`Connect`、`Start`、`Pause`、`Restart`、`Stop`
- 多指标采集：
  - FPS
  - CPU Clock
  - CPU Usage
  - Temperature
  - Power
- 实时数据聚合与图表展示
- 会话数据导出（如 CSV / JSON / PNG / HTML）

## 架构概览

PerfDroid 采用三层结构：

- `Profiler Layer`：各个指标采集器独立运行，负责直接读取设备或系统接口数据
- `Aggregation Layer`：统一读取 profiler 最新结果，组装标准化 `MetricBatch`
- `GUI Layer`：负责展示、控制、会话管理与导出

在线程模型上，项目采用 `1 + n` 模式：

- `1` 个应用线程负责聚合与上层控制
- `n` 个 profiler 线程分别采集各自指标

![Architecture](docs/images/archtitecture.png)

Profiler 层的设计重点是模块化与低耦合，每个指标理论上都可以作为独立 crate 演进：

![Profiler Layer](docs/images/profiler_layer.png)

## 仓库结构

```text
PerfDroid/
├── crates/
│   ├── app/                    # 应用层模块骨架
│   ├── pdcore/                 # 核心抽象、类型、错误和常量
│   ├── registry/               # profiler 注册与元数据管理
│   └── profiler/
│       ├── cpu_clock/
│       ├── cpu_usage/
│       ├── fps/
│       ├── power/
│       └── temperature/
├── docs/
│   ├── tech_doc.md             # 技术设计文档
│   └── images/                 # 架构图和示意图
└── Cargo.toml                  # Rust workspace 配置
```

### 环境要求

- Rust stable
- Cargo

## License

本项目基于 Apache-2.0 License 开源，详见 [`LICENSE`](LICENSE)。
