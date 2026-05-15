pub mod metadata;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use adb_client::{ADBDeviceExt, server::ADBServer, server_device::ADBServerDevice};
use pdcore::adb::workspace_adb_server;
use pdcore::traits::{Collector, Profiler};
use pdcore::types::{CollectorMetadata, ProfilerMetadata};
use pdcore::{CoreError, INVALID_METRIC_VALUE};

use crate::metadata::{
    CURRENT_COLLECTOR_KEY, CURRENT_PROFILER_KEY, POWER_COLLECTOR_KEY, POWER_PROFILER_KEY, UNIT_MA,
    UNIT_MV, UNIT_MW, VOLTAGE_COLLECTOR_KEY, VOLTAGE_PROFILER_KEY,
};

const BATTERY_DUMP_COMMAND: &str = "dumpsys battery";
const UEVENT_GLOB_PATTERN: &str = "/sys/class/power_supply/*/uevent";
const MICHARGE_LOGCAT_COMMAND: &str = "logcat -d -b system -s IMiCharge:E | tail -n 256";
const XIAOMI_BATTERY_VOLTAGE_PATH: &str =
    "/sys/devices/platform/soc/soc:mca_business_battery/power_supply/battery/voltage_now";
const XIAOMI_BATTERY_CURRENT_NOW_PATH: &str =
    "/sys/devices/platform/soc/soc:mca_business_battery/power_supply/battery/current_now";
const POWER_SUPPLY_SCAN_COMMAND: &str = r#"for d in /sys/class/power_supply/* "$(readlink -f /sys/class/power_supply/battery 2>/dev/null)" /sys/devices/platform/soc/soc:mca_business_battery/power_supply/battery; do
  [ -n "$d" ] || continue;
  [ -d "$d" ] || continue;
  echo "== $d ==";
  [ -f "$d/type" ] && echo -n "type=" && cat "$d/type";
  [ -f "$d/current_now" ] && echo -n "current_now=" && cat "$d/current_now";
  [ -f "$d/current_avg" ] && echo -n "current_avg=" && cat "$d/current_avg";
  [ -f "$d/batt_current_ua_now" ] && echo -n "batt_current_ua_now=" && cat "$d/batt_current_ua_now";
  [ -f "$d/voltage_now" ] && echo -n "voltage_now=" && cat "$d/voltage_now";
  [ -f "$d/status" ] && echo -n "status=" && cat "$d/status";
done 2>/dev/null"#;
const VOLTAGE_PATHS: [&str; 5] = [
    "/sys/class/power_supply/battery/voltage_now",
    "/sys/class/power_supply/Battery/voltage_now",
    "/sys/class/power_supply/maxfg/voltage_now",
    "/sys/class/power_supply/bms/voltage_now",
    XIAOMI_BATTERY_VOLTAGE_PATH,
];
const CURRENT_PATHS: [&str; 5] = [
    "/sys/class/power_supply/battery/current_now",
    "/sys/class/power_supply/Battery/current_now",
    "/sys/class/power_supply/maxfg/current_now",
    "/sys/class/power_supply/bms/current_now",
    XIAOMI_BATTERY_CURRENT_NOW_PATH,
];
const VOLTAGE_GLOB_PATTERNS: [&str; 1] = ["/sys/class/power_supply/*/voltage_now"];
const UEVENT_VOLTAGE_KEYS: [&str; 2] = ["POWER_SUPPLY_VOLTAGE_NOW", "POWER_SUPPLY_VOLTAGE_OCV"];
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VoltageReadStrategy {
    FixedSysfs,
    GlobSysfs,
    PowerSupplyScan,
    Uevent,
    DumpsysBattery,
    MiChargeLogcat,
}

impl VoltageReadStrategy {
    fn label(self) -> &'static str {
        match self {
            Self::FixedSysfs => "sysfs fixed path",
            Self::GlobSysfs => "sysfs glob path",
            Self::PowerSupplyScan => "power_supply directory scan",
            Self::Uevent => "power_supply uevent",
            Self::DumpsysBattery => "dumpsys battery",
            Self::MiChargeLogcat => "xiaomi micharge logcat",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CurrentReadStrategy {
    FixedSysfs,
}

impl CurrentReadStrategy {
    fn label(self) -> &'static str {
        match self {
            Self::FixedSysfs => "sysfs fixed path",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PowerReadStrategy {
    FixedSysfs,
    Hybrid {
        voltage: VoltageReadStrategy,
        current: CurrentReadStrategy,
    },
}

impl PowerReadStrategy {
    fn label(self) -> String {
        match self {
            Self::FixedSysfs => "fixed sysfs pair".to_string(),
            Self::Hybrid { voltage, current } => {
                format!(
                    "hybrid: voltage via {}, current via {}",
                    voltage.label(),
                    current.label()
                )
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectedPowerStrategy {
    Voltage(VoltageReadStrategy),
    Current(CurrentReadStrategy),
    Power(PowerReadStrategy),
}

impl SelectedPowerStrategy {
    fn label(self) -> String {
        match self {
            Self::Voltage(strategy) => strategy.label().to_string(),
            Self::Current(strategy) => strategy.label().to_string(),
            Self::Power(strategy) => strategy.label(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerMetricKind {
    Voltage,
    Current,
    Power,
}

impl PowerMetricKind {
    fn profiler_key(self) -> &'static str {
        match self {
            Self::Voltage => VOLTAGE_PROFILER_KEY,
            Self::Current => CURRENT_PROFILER_KEY,
            Self::Power => POWER_PROFILER_KEY,
        }
    }

    fn collector_key(self) -> &'static str {
        match self {
            Self::Voltage => VOLTAGE_COLLECTOR_KEY,
            Self::Current => CURRENT_COLLECTOR_KEY,
            Self::Power => POWER_COLLECTOR_KEY,
        }
    }

    fn unit(self) -> &'static str {
        match self {
            Self::Voltage => UNIT_MV,
            Self::Current => UNIT_MA,
            Self::Power => UNIT_MW,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PowerSnapshot {
    voltage_mv: Option<i64>,
    current_ma: Option<i64>,
    power_mw: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PowerSupplyEntry {
    path: String,
    name: String,
    supply_type: Option<String>,
    status: Option<String>,
    current_now_raw: Option<i64>,
    current_avg_raw: Option<i64>,
    batt_current_ua_now_raw: Option<i64>,
    voltage_now_raw: Option<i64>,
}

#[derive(Debug)]
pub struct PowerCollector {
    metadata: CollectorMetadata,
    value: Arc<AtomicI64>,
}

impl PowerCollector {
    fn new(metric: PowerMetricKind) -> Result<Self, CoreError> {
        Ok(Self {
            metadata: CollectorMetadata::new(metric.collector_key(), metric.unit(), 0)?,
            value: Arc::new(AtomicI64::new(INVALID_METRIC_VALUE)),
        })
    }
}

impl Collector for PowerCollector {
    fn metadata(&self) -> &CollectorMetadata {
        &self.metadata
    }

    fn read_buffer(&self, ordering: Ordering) -> i64 {
        self.value.load(ordering)
    }
}

#[derive(Debug)]
struct SamplerRuntime {
    stop_tx: Sender<()>,
    pause_flag: Arc<AtomicBool>,
    join_handle: JoinHandle<()>,
}

#[derive(Debug)]
pub struct BatteryPowerProfiler {
    serial: Option<String>,
    metric: PowerMetricKind,
    sample_interval: Duration,
    metadata: ProfilerMetadata,
    collector: PowerCollector,
    strategy: Option<SelectedPowerStrategy>,
    connected: bool,
    sampler: Option<SamplerRuntime>,
}

impl BatteryPowerProfiler {
    pub fn new(
        serial: Option<String>,
        metric: PowerMetricKind,
        sample_interval: Duration,
    ) -> Result<Self, CoreError> {
        let collector = PowerCollector::new(metric)?;
        Ok(Self {
            serial,
            metric,
            sample_interval,
            metadata: ProfilerMetadata::new(
                metric.profiler_key(),
                vec![collector.metadata().clone()],
            )?,
            collector,
            strategy: None,
            connected: false,
            sampler: None,
        })
    }

    pub fn voltage(serial: Option<String>, sample_interval: Duration) -> Result<Self, CoreError> {
        Self::new(serial, PowerMetricKind::Voltage, sample_interval)
    }

    pub fn current(serial: Option<String>, sample_interval: Duration) -> Result<Self, CoreError> {
        Self::new(serial, PowerMetricKind::Current, sample_interval)
    }

    pub fn power(serial: Option<String>, sample_interval: Duration) -> Result<Self, CoreError> {
        Self::new(serial, PowerMetricKind::Power, sample_interval)
    }

    pub fn metadata_clone(&self) -> ProfilerMetadata {
        self.metadata.clone()
    }

    pub fn snapshot_value(&self) -> i64 {
        self.collector.read_buffer(Ordering::Relaxed)
    }

    pub fn selected_strategy_label(&self) -> Option<String> {
        self.strategy.map(SelectedPowerStrategy::label)
    }

    fn ensure_connected(&self, operation: &'static str) -> Result<(), CoreError> {
        if self.connected {
            Ok(())
        } else {
            Err(CoreError::Runtime(format!(
                "{} profiler must be connected before `{operation}`",
                self.metric.profiler_key()
            )))
        }
    }
}

impl Profiler for BatteryPowerProfiler {
    fn metadata(&self) -> &ProfilerMetadata {
        &self.metadata
    }

    fn collectors(&self) -> Vec<&dyn Collector> {
        vec![&self.collector]
    }

    fn connect(&mut self) -> Result<(), CoreError> {
        let mut server = workspace_adb_server();
        let mut device = open_target_device(&mut server, self.serial.as_deref())?;

        let (strategy, value) = detect_strategy_and_value(&mut device, self.metric)?;
        self.collector.value.store(value, Ordering::Release);
        self.strategy = Some(strategy);

        if self.snapshot_value() == INVALID_METRIC_VALUE {
            return Err(CoreError::Runtime(format!(
                "failed to read a usable {} sample from the target device",
                self.metric.profiler_key()
            )));
        }

        self.connected = true;
        Ok(())
    }

    fn start(&mut self) -> Result<(), CoreError> {
        self.ensure_connected("start")?;

        if let Some(runtime) = self.sampler.as_ref() {
            runtime.pause_flag.store(false, Ordering::Release);
            return Ok(());
        }

        let mut server = workspace_adb_server();
        let mut device = open_target_device(&mut server, self.serial.as_deref())?;
        let metric = self.metric;
        let strategy = self.strategy.ok_or_else(|| {
            CoreError::Runtime(format!(
                "{} profiler has no selected read strategy",
                self.metric.profiler_key()
            ))
        })?;
        let writer = Arc::clone(&self.collector.value);
        let (stop_tx, stop_rx) = mpsc::channel();
        let pause_flag = Arc::new(AtomicBool::new(false));
        let pause_flag_for_thread = Arc::clone(&pause_flag);
        let sample_interval = self.sample_interval;

        sample_once_with_strategy(&mut device, metric, strategy, &writer)?;

        let join_handle = thread::spawn(move || {
            sampling_loop(
                &mut device,
                metric,
                strategy,
                writer,
                stop_rx,
                pause_flag_for_thread,
                sample_interval,
            )
        });

        self.sampler = Some(SamplerRuntime {
            stop_tx,
            pause_flag,
            join_handle,
        });
        Ok(())
    }

    fn pause(&mut self) -> Result<(), CoreError> {
        self.ensure_connected("pause")?;
        let sampler = self.sampler.as_ref().ok_or_else(|| {
            CoreError::Runtime(format!(
                "{} sampler is not running",
                self.metric.profiler_key()
            ))
        })?;
        sampler.pause_flag.store(true, Ordering::Release);
        Ok(())
    }

    fn restart(&mut self) -> Result<(), CoreError> {
        self.ensure_connected("restart")?;
        let sampler = self.sampler.as_ref().ok_or_else(|| {
            CoreError::Runtime(format!(
                "{} sampler is not running",
                self.metric.profiler_key()
            ))
        })?;
        sampler.pause_flag.store(false, Ordering::Release);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), CoreError> {
        if let Some(runtime) = self.sampler.take() {
            let _ = runtime.stop_tx.send(());
            let _ = runtime.join_handle.join();
        }

        self.collector
            .value
            .store(INVALID_METRIC_VALUE, Ordering::Release);
        self.strategy = None;
        self.connected = false;
        Ok(())
    }
}

fn sampling_loop(
    device: &mut ADBServerDevice,
    metric: PowerMetricKind,
    strategy: SelectedPowerStrategy,
    writer: Arc<AtomicI64>,
    stop_rx: Receiver<()>,
    pause_flag: Arc<AtomicBool>,
    interval: Duration,
) {
    let mut next_tick = Instant::now();

    loop {
        if stop_rx.try_recv().is_ok() {
            break;
        }

        if pause_flag.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(20));
            continue;
        }

        if sample_once_with_strategy(device, metric, strategy, &writer).is_err() {
            writer.store(INVALID_METRIC_VALUE, Ordering::Release);
        }

        let now = Instant::now();
        if next_tick > now {
            thread::sleep(next_tick - now);
        } else {
            next_tick = now;
        }
        next_tick += interval;
    }
}

fn sample_once_with_strategy(
    device: &mut impl ADBDeviceExt,
    metric: PowerMetricKind,
    strategy: SelectedPowerStrategy,
    writer: &Arc<AtomicI64>,
) -> Result<(), CoreError> {
    let snapshot = query_power_snapshot_with_strategy(device, strategy);
    let value = match metric {
        PowerMetricKind::Voltage => snapshot.voltage_mv,
        PowerMetricKind::Current => snapshot.current_ma,
        PowerMetricKind::Power => snapshot.power_mw,
    }
    .unwrap_or(INVALID_METRIC_VALUE);
    writer.store(value, Ordering::Release);
    Ok(())
}

fn detect_strategy_and_value(
    device: &mut impl ADBDeviceExt,
    metric: PowerMetricKind,
) -> Result<(SelectedPowerStrategy, i64), CoreError> {
    let (strategy, snapshot) = match metric {
        PowerMetricKind::Voltage => detect_voltage_strategy(device)?
            .map(|(strategy, value)| {
                (
                    SelectedPowerStrategy::Voltage(strategy),
                    PowerSnapshot {
                        voltage_mv: Some(value),
                        current_ma: None,
                        power_mw: None,
                    },
                )
            })
            .ok_or_else(|| {
                CoreError::Runtime("failed to find a usable voltage read strategy".to_string())
            })?,
        PowerMetricKind::Current => detect_current_strategy(device)?
            .map(|(strategy, value)| {
                (
                    SelectedPowerStrategy::Current(strategy),
                    PowerSnapshot {
                        voltage_mv: None,
                        current_ma: Some(value),
                        power_mw: None,
                    },
                )
            })
            .ok_or_else(|| {
                CoreError::Runtime("failed to find a usable current read strategy".to_string())
            })?,
        PowerMetricKind::Power => detect_power_strategy(device)?
            .map(|(strategy, snapshot)| (SelectedPowerStrategy::Power(strategy), snapshot))
            .ok_or_else(|| {
                CoreError::Runtime("failed to find a usable power read strategy".to_string())
            })?,
    };

    let value = match metric {
        PowerMetricKind::Voltage => snapshot.voltage_mv,
        PowerMetricKind::Current => snapshot.current_ma,
        PowerMetricKind::Power => snapshot.power_mw,
    }
    .unwrap_or(INVALID_METRIC_VALUE);
    Ok((strategy, value))
}

fn query_power_snapshot_with_strategy(
    device: &mut impl ADBDeviceExt,
    strategy: SelectedPowerStrategy,
) -> PowerSnapshot {
    match strategy {
        SelectedPowerStrategy::Voltage(strategy) => PowerSnapshot {
            voltage_mv: query_voltage_with_strategy(device, strategy),
            current_ma: None,
            power_mw: None,
        },
        SelectedPowerStrategy::Current(strategy) => PowerSnapshot {
            voltage_mv: None,
            current_ma: query_current_with_strategy(device, strategy),
            power_mw: None,
        },
        SelectedPowerStrategy::Power(strategy) => query_power_with_strategy(device, strategy),
    }
}

fn detect_voltage_strategy(
    device: &mut impl ADBDeviceExt,
) -> Result<Option<(VoltageReadStrategy, i64)>, CoreError> {
    for strategy in [
        VoltageReadStrategy::FixedSysfs,
        VoltageReadStrategy::GlobSysfs,
        VoltageReadStrategy::PowerSupplyScan,
        VoltageReadStrategy::Uevent,
        VoltageReadStrategy::DumpsysBattery,
        VoltageReadStrategy::MiChargeLogcat,
    ] {
        if let Some(value) = query_voltage_with_strategy(device, strategy) {
            return Ok(Some((strategy, value)));
        }
    }
    Ok(None)
}

fn detect_current_strategy(
    device: &mut impl ADBDeviceExt,
) -> Result<Option<(CurrentReadStrategy, i64)>, CoreError> {
    let strategy = CurrentReadStrategy::FixedSysfs;
    if let Some(value) = query_current_with_strategy(device, strategy) {
        return Ok(Some((strategy, value)));
    }
    Ok(None)
}

fn detect_power_strategy(
    device: &mut impl ADBDeviceExt,
) -> Result<Option<(PowerReadStrategy, PowerSnapshot)>, CoreError> {
    for strategy in [
        PowerReadStrategy::FixedSysfs,
    ] {
        let snapshot = query_power_with_strategy(device, strategy);
        if snapshot.power_mw.is_some() {
            return Ok(Some((strategy, snapshot)));
        }
    }

    let Some((voltage, voltage_mv)) = detect_voltage_strategy(device)? else {
        return Ok(None);
    };
    let Some((current, current_ma)) = detect_current_strategy(device)? else {
        return Ok(None);
    };
    let power_mw = Some(voltage_mv.saturating_mul(current_ma).saturating_div(1000));
    Ok(Some((
        PowerReadStrategy::Hybrid { voltage, current },
        PowerSnapshot {
            voltage_mv: Some(voltage_mv),
            current_ma: Some(current_ma),
            power_mw,
        },
    )))
}

fn query_power_with_strategy(
    device: &mut impl ADBDeviceExt,
    strategy: PowerReadStrategy,
) -> PowerSnapshot {
    let (voltage_mv, current_ma) = match strategy {
        PowerReadStrategy::FixedSysfs => (
            query_voltage_with_strategy(device, VoltageReadStrategy::FixedSysfs),
            query_current_with_strategy(device, CurrentReadStrategy::FixedSysfs),
        ),
        PowerReadStrategy::Hybrid { voltage, current } => (
            query_voltage_with_strategy(device, voltage),
            query_current_with_strategy(device, current),
        ),
    };
    let power_mw = voltage_mv
        .zip(current_ma)
        .map(|(voltage_mv, current_ma)| voltage_mv.saturating_mul(current_ma).saturating_div(1000));

    PowerSnapshot {
        voltage_mv,
        current_ma,
        power_mw,
    }
}

fn query_voltage_with_strategy(
    device: &mut impl ADBDeviceExt,
    strategy: VoltageReadStrategy,
) -> Option<i64> {
    match strategy {
        VoltageReadStrategy::FixedSysfs => {
            query_sysfs_value(device, &VOLTAGE_PATHS, normalize_voltage_mv)
        }
        VoltageReadStrategy::GlobSysfs => {
            query_sysfs_glob_value(device, &VOLTAGE_GLOB_PATTERNS, normalize_voltage_mv)
        }
        VoltageReadStrategy::PowerSupplyScan => query_power_supply_scan_snapshot(device).voltage_mv,
        VoltageReadStrategy::Uevent => {
            query_uevent_value(device, &UEVENT_VOLTAGE_KEYS, normalize_voltage_mv)
        }
        VoltageReadStrategy::DumpsysBattery => run_shell(device, BATTERY_DUMP_COMMAND)
            .ok()
            .and_then(|text| parse_dumpsys_voltage_mv(&text)),
        VoltageReadStrategy::MiChargeLogcat => query_micharge_snapshot(device).voltage_mv,
    }
}

fn query_current_with_strategy(
    device: &mut impl ADBDeviceExt,
    strategy: CurrentReadStrategy,
) -> Option<i64> {
    match strategy {
        CurrentReadStrategy::FixedSysfs => {
            query_sysfs_value(device, &CURRENT_PATHS, normalize_current_ma)
        }
    }
}

fn query_sysfs_value(
    device: &mut impl ADBDeviceExt,
    paths: &[&str],
    normalize: fn(i64) -> Option<i64>,
) -> Option<i64> {
    for path in paths {
        let command = format!("cat {path}");
        let output = match run_shell(device, &command) {
            Ok(output) => output,
            Err(_) => continue,
        };
        let raw = match output.trim().parse::<i64>() {
            Ok(raw) => raw,
            Err(_) => continue,
        };
        if let Some(value) = normalize(raw) {
            return Some(value);
        }
    }
    None
}

fn query_sysfs_glob_value(
    device: &mut impl ADBDeviceExt,
    patterns: &[&str],
    normalize: fn(i64) -> Option<i64>,
) -> Option<i64> {
    for pattern in patterns {
        let command = format!("cat {pattern}");
        let output = match run_shell(device, &command) {
            Ok(output) => output,
            Err(_) => continue,
        };
        for line in output.lines() {
            let raw = match line.trim().parse::<i64>() {
                Ok(raw) => raw,
                Err(_) => continue,
            };
            if let Some(value) = normalize(raw) {
                return Some(value);
            }
        }
    }
    None
}

fn query_uevent_value(
    device: &mut impl ADBDeviceExt,
    keys: &[&str],
    normalize: fn(i64) -> Option<i64>,
) -> Option<i64> {
    let output = run_shell(device, &format!("cat {UEVENT_GLOB_PATTERN}")).ok()?;
    parse_keyed_value(&output, keys, normalize)
}

fn query_power_supply_scan_snapshot(device: &mut impl ADBDeviceExt) -> PowerSnapshot {
    let output = match run_shell(device, POWER_SUPPLY_SCAN_COMMAND) {
        Ok(output) => output,
        Err(_) => {
            return PowerSnapshot {
                voltage_mv: None,
                current_ma: None,
                power_mw: None,
            };
        }
    };
    let entries = parse_power_supply_scan(&output);
    let voltage_mv = best_voltage_from_power_supply_entries(&entries);
    let current_ma = best_current_from_power_supply_entries(&entries);
    let power_mw = voltage_mv
        .zip(current_ma)
        .map(|(voltage_mv, current_ma)| voltage_mv.saturating_mul(current_ma).saturating_div(1000));

    PowerSnapshot {
        voltage_mv,
        current_ma,
        power_mw,
    }
}

fn query_micharge_snapshot(device: &mut impl ADBDeviceExt) -> PowerSnapshot {
    let output = match run_shell(device, MICHARGE_LOGCAT_COMMAND) {
        Ok(output) => output,
        Err(_) => {
            return PowerSnapshot {
                voltage_mv: None,
                current_ma: None,
                power_mw: None,
            };
        }
    };
    parse_micharge_snapshot(&output)
}

fn normalize_voltage_mv(raw: i64) -> Option<i64> {
    if raw <= 0 {
        return None;
    }

    if raw >= 10_000 {
        Some(raw / 1000)
    } else {
        Some(raw)
    }
}

fn normalize_current_ma(raw: i64) -> Option<i64> {
    if raw == 0 {
        return Some(0);
    }

    let magnitude = raw.checked_abs()?;
    if magnitude >= 10_000 {
        Some(magnitude / 1000)
    } else {
        Some(magnitude)
    }
}

fn parse_dumpsys_voltage_mv(output: &str) -> Option<i64> {
    output.lines().find_map(|line| {
        let trimmed = line.trim();
        let raw = trimmed
            .strip_prefix("voltage:")?
            .trim()
            .split_whitespace()
            .next()?;
        normalize_voltage_mv(raw.parse::<i64>().ok()?)
    })
}

fn parse_micharge_snapshot(output: &str) -> PowerSnapshot {
    let voltage_mv = parse_micharge_method_value(output, 11, normalize_voltage_mv);
    let current_ma = parse_micharge_method_value(output, 6, normalize_current_ma);
    let power_mw = voltage_mv
        .zip(current_ma)
        .map(|(voltage_mv, current_ma)| voltage_mv.saturating_mul(current_ma).saturating_div(1000));

    PowerSnapshot {
        voltage_mv,
        current_ma,
        power_mw,
    }
}

fn parse_micharge_method_value(
    output: &str,
    method_id: i32,
    normalize: fn(i64) -> Option<i64>,
) -> Option<i64> {
    output.lines().rev().find_map(|line| {
        let (parsed_method, raw_value) = parse_micharge_line(line)?;
        if parsed_method != method_id {
            return None;
        }
        normalize(raw_value)
    })
}

fn parse_micharge_line(line: &str) -> Option<(i32, i64)> {
    let (_, remainder) = line.split_once("MiCharge method ")?;
    let (method, value) = remainder.split_once(" val = ")?;
    Some((
        method.trim().parse::<i32>().ok()?,
        value.trim().parse::<i64>().ok()?,
    ))
}

fn parse_keyed_value(
    output: &str,
    keys: &[&str],
    normalize: fn(i64) -> Option<i64>,
) -> Option<i64> {
    output.lines().find_map(|line| {
        let trimmed = line.trim();
        keys.iter().find_map(|key| {
            let raw = trimmed.strip_prefix(key)?.strip_prefix('=')?.trim();
            normalize(raw.parse::<i64>().ok()?)
        })
    })
}

fn parse_power_supply_scan(output: &str) -> Vec<PowerSupplyEntry> {
    let mut entries = Vec::new();
    let mut current: Option<PowerSupplyEntry> = None;

    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(path) = trimmed
            .strip_prefix("== ")
            .and_then(|line| line.strip_suffix(" =="))
        {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            let path = path.to_string();
            let name = path
                .rsplit('/')
                .next()
                .map(str::to_string)
                .unwrap_or_else(|| path.clone());
            current = Some(PowerSupplyEntry {
                path,
                name,
                supply_type: None,
                status: None,
                current_now_raw: None,
                current_avg_raw: None,
                batt_current_ua_now_raw: None,
                voltage_now_raw: None,
            });
            continue;
        }

        let Some(entry) = current.as_mut() else {
            continue;
        };

        if let Some(value) = trimmed.strip_prefix("type=") {
            entry.supply_type = Some(value.trim().to_string());
        } else if let Some(value) = trimmed.strip_prefix("status=") {
            entry.status = Some(value.trim().to_string());
        } else if let Some(value) = trimmed.strip_prefix("current_now=") {
            entry.current_now_raw = value.trim().parse::<i64>().ok();
        } else if let Some(value) = trimmed.strip_prefix("current_avg=") {
            entry.current_avg_raw = value.trim().parse::<i64>().ok();
        } else if let Some(value) = trimmed.strip_prefix("batt_current_ua_now=") {
            entry.batt_current_ua_now_raw = value.trim().parse::<i64>().ok();
        } else if let Some(value) = trimmed.strip_prefix("voltage_now=") {
            entry.voltage_now_raw = value.trim().parse::<i64>().ok();
        }
    }

    if let Some(entry) = current {
        entries.push(entry);
    }

    entries
}

fn best_voltage_from_power_supply_entries(entries: &[PowerSupplyEntry]) -> Option<i64> {
    entries
        .iter()
        .filter_map(|entry| {
            Some((
                power_supply_entry_score(entry),
                normalize_voltage_mv(entry.voltage_now_raw?)?,
            ))
        })
        .max_by_key(|(score, _)| *score)
        .map(|(_, value)| value)
}

fn best_current_from_power_supply_entries(entries: &[PowerSupplyEntry]) -> Option<i64> {
    entries
        .iter()
        .filter_map(|entry| {
            Some((
                power_supply_entry_score(entry),
                power_supply_entry_current_ma(entry)?,
            ))
        })
        .max_by_key(|(score, _)| *score)
        .map(|(_, value)| value)
}

fn power_supply_entry_current_ma(entry: &PowerSupplyEntry) -> Option<i64> {
    entry
        .current_now_raw
        .and_then(normalize_current_ma)
        .or_else(|| entry.batt_current_ua_now_raw.and_then(normalize_current_ma))
        .or_else(|| entry.current_avg_raw.and_then(normalize_current_ma))
}

fn power_supply_entry_score(entry: &PowerSupplyEntry) -> i32 {
    let mut score = 0;
    if entry
        .supply_type
        .as_deref()
        .is_some_and(|kind| kind.eq_ignore_ascii_case("Battery"))
    {
        score += 100;
    }

    let name = entry.name.to_ascii_lowercase();
    if name.contains("battery") {
        score += 80;
    } else if name.contains("bms") {
        score += 60;
    } else if name.contains("fg") || name.contains("fuel") || name.contains("max") {
        score += 40;
    }

    if entry.voltage_now_raw.is_some() {
        score += 10;
    }
    if entry.current_now_raw.is_some() || entry.batt_current_ua_now_raw.is_some() {
        score += 15;
    } else if entry.current_avg_raw.is_some() {
        score += 8;
    }

    if entry.status.as_deref().is_some_and(|status| {
        matches!(
            status.to_ascii_lowercase().as_str(),
            "charging" | "discharging" | "full" | "not charging"
        )
    }) {
        score += 5;
    }

    score
}

fn open_target_device(
    server: &mut ADBServer,
    serial: Option<&str>,
) -> Result<ADBServerDevice, CoreError> {
    match serial {
        Some(serial) => server.get_device_by_name(serial).map_err(|err| {
            CoreError::Runtime(format!("failed to get adb device `{serial}`: {err}"))
        }),
        None => server
            .get_device()
            .map_err(|err| CoreError::Runtime(format!("failed to get adb device: {err}"))),
    }
}

fn run_shell(device: &mut impl ADBDeviceExt, command: &str) -> Result<String, CoreError> {
    let mut out = Vec::with_capacity(2048);
    let mut err = Vec::with_capacity(256);
    let status = device
        .shell_command(&command, Some(&mut out), Some(&mut err))
        .map_err(|err| CoreError::Runtime(format!("adb shell failed for `{command}`: {err}")))?;

    if status.is_some_and(|code| code != 0) {
        let stderr = String::from_utf8_lossy(&err).trim().to_string();
        return Err(CoreError::Runtime(format!(
            "adb shell returned non-zero for `{command}`: {stderr}"
        )));
    }

    Ok(String::from_utf8_lossy(&out).into_owned())
}

#[cfg(test)]
mod tests {
    use super::{
        PowerSnapshot, best_current_from_power_supply_entries,
        best_voltage_from_power_supply_entries, normalize_current_ma, normalize_voltage_mv,
        parse_dumpsys_voltage_mv, parse_keyed_value, parse_micharge_line,
        parse_micharge_snapshot, parse_power_supply_scan,
    };

    #[test]
    fn normalizes_voltage_from_microvolts() {
        assert_eq!(normalize_voltage_mv(4_321_000), Some(4321));
        assert_eq!(normalize_voltage_mv(4321), Some(4321));
        assert_eq!(normalize_voltage_mv(0), None);
    }

    #[test]
    fn normalizes_current_to_positive_milliamps() {
        assert_eq!(normalize_current_ma(-512_000), Some(512));
        assert_eq!(normalize_current_ma(875_000), Some(875));
        assert_eq!(normalize_current_ma(650), Some(650));
    }

    #[test]
    fn parses_voltage_from_dumpsys_battery() {
        let output = "\
Current Battery Service state:
  level: 74
  voltage: 4312
  temperature: 298";
        assert_eq!(parse_dumpsys_voltage_mv(output), Some(4312));
    }

    #[test]
    fn parses_current_from_power_supply_uevent() {
        let output = "\
POWER_SUPPLY_NAME=battery
POWER_SUPPLY_CURRENT_NOW=-623000
POWER_SUPPLY_VOLTAGE_NOW=4312000";
        assert_eq!(
            parse_keyed_value(output, &["POWER_SUPPLY_CURRENT_NOW"], normalize_current_ma),
            Some(623)
        );
    }

    #[test]
    fn parses_power_supply_scan_and_prefers_battery_entry() {
        let output = "\
== /sys/class/power_supply/usb ==
type=USB
current_now=1500000
status=Charging
== /sys/class/power_supply/battery ==
type=Battery
current_now=-623000
current_avg=-500000
voltage_now=4312000
status=Discharging";
        let entries = parse_power_supply_scan(output);
        assert_eq!(entries.len(), 2);
        assert_eq!(best_current_from_power_supply_entries(&entries), Some(623));
        assert_eq!(best_voltage_from_power_supply_entries(&entries), Some(4312));
    }

    #[test]
    fn parses_micharge_voltage_and_current_from_latest_bundle() {
        let output = "\
04-16 17:01:24.412  3462  3663 E IMiCharge: MiCharge method 11 val = 3931000
04-16 17:01:24.413  3462  3663 E IMiCharge: MiCharge method 6 val = -540000
04-16 17:14:57.662  3462  3663 E IMiCharge: MiCharge method 11 val = 3884000
04-16 17:14:57.663  3462  3663 E IMiCharge: MiCharge method 6 val = -362000
04-16 17:14:57.663  3462  3663 E IMiCharge: MiCharge method 9 val = 445";
        let snapshot = parse_micharge_snapshot(output);
        assert_eq!(snapshot.voltage_mv, Some(3884));
        assert_eq!(snapshot.current_ma, Some(362));
        assert_eq!(snapshot.power_mw, Some(1406));
    }

    #[test]
    fn ignores_micharge_lines_without_numeric_values() {
        assert_eq!(
            parse_micharge_line(
                "04-16 17:14:57.651  3462  3663 E IMiCharge: MiCharge method 4 val = CDP"
            ),
            None
        );
    }

    #[test]
    fn computes_power_from_voltage_and_current() {
        let snapshot = PowerSnapshot {
            voltage_mv: Some(4312),
            current_ma: Some(512),
            power_mw: Some(4312 * 512 / 1000),
        };
        assert_eq!(snapshot.power_mw, Some(2207));
    }
}
