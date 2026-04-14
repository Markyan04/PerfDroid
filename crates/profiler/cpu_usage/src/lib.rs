pub mod metadata;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use adb_client::{ADBDeviceExt, server::ADBServer, server_device::ADBServerDevice};
use pdcore::CoreError;
use pdcore::INVALID_METRIC_VALUE;
use pdcore::traits::{Collector, Profiler};
use pdcore::types::{CollectorMetadata, ProfilerMetadata};

use crate::metadata::{PROFILER_KEY, UNIT_PERCENT};

const CPU_STAT_COMMAND: &str = "cat /proc/stat";
const POLICY_DISCOVERY_COMMAND: &str = "for p in /sys/devices/system/cpu/cpufreq/policy*; do bn=$(basename $p); cpus=$(cat $p/related_cpus 2>/dev/null); echo $bn:$cpus; done";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CpuTick {
    total: u64,
    idle: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PolicyGroup {
    name: String,
    core_names: Vec<String>,
}

#[derive(Debug)]
pub struct CpuUsageCollector {
    metadata: CollectorMetadata,
    value_percent: Arc<AtomicI64>,
}

impl CpuUsageCollector {
    fn new(collector_key: String, order: usize) -> Result<Self, CoreError> {
        Ok(Self {
            metadata: CollectorMetadata::new(collector_key, UNIT_PERCENT, order)?,
            value_percent: Arc::new(AtomicI64::new(INVALID_METRIC_VALUE)),
        })
    }
}

impl Collector for CpuUsageCollector {
    fn metadata(&self) -> &CollectorMetadata {
        &self.metadata
    }

    fn read_buffer(&self, ordering: Ordering) -> i64 {
        self.value_percent.load(ordering)
    }
}

#[derive(Debug)]
struct SamplerRuntime {
    stop_tx: Sender<()>,
    pause_flag: Arc<AtomicBool>,
    join_handle: JoinHandle<()>,
}

#[derive(Debug)]
pub struct CpuUsageProfiler {
    serial: Option<String>,
    sample_interval: Duration,
    metadata: ProfilerMetadata,
    policies: Vec<PolicyGroup>,
    collectors: Vec<CpuUsageCollector>,
    connected: bool,
    sampler: Option<SamplerRuntime>,
}

impl CpuUsageProfiler {
    pub fn new(serial: Option<String>, sample_interval: Duration) -> Result<Self, CoreError> {
        Ok(Self {
            serial,
            sample_interval,
            metadata: ProfilerMetadata::new(
                PROFILER_KEY,
                vec![CollectorMetadata::new("policy0", UNIT_PERCENT, 0)?],
            )?,
            policies: Vec::new(),
            collectors: Vec::new(),
            connected: false,
            sampler: None,
        })
    }

    pub fn metadata_clone(&self) -> ProfilerMetadata {
        self.metadata.clone()
    }

    pub fn snapshot_values(&self) -> Vec<i64> {
        self.collectors
            .iter()
            .map(|collector| collector.read_buffer(Ordering::Relaxed))
            .collect()
    }

    fn ensure_connected(&self, operation: &'static str) -> Result<(), CoreError> {
        if self.connected {
            Ok(())
        } else {
            Err(CoreError::Runtime(format!(
                "CPU_USAGE profiler must be connected before `{operation}`"
            )))
        }
    }

    fn discover_policies(&self) -> Result<Vec<PolicyGroup>, CoreError> {
        let mut server = ADBServer::default();
        let mut device = open_target_device(&mut server, self.serial.as_deref())?;
        let output = run_shell(&mut device, POLICY_DISCOVERY_COMMAND)?;
        let policies = parse_policy_listing(&output);
        if policies.is_empty() {
            return Err(CoreError::Runtime(
                "no readable CPU usage policy was found on the target device".to_string(),
            ));
        }
        Ok(policies)
    }

    fn create_collectors(policies: &[PolicyGroup]) -> Result<Vec<CpuUsageCollector>, CoreError> {
        policies
            .into_iter()
            .enumerate()
            .map(|(order, policy)| CpuUsageCollector::new(policy.name.clone(), order))
            .collect()
    }
}

impl Profiler for CpuUsageProfiler {
    fn metadata(&self) -> &ProfilerMetadata {
        &self.metadata
    }

    fn collectors(&self) -> Vec<&dyn Collector> {
        self.collectors
            .iter()
            .map(|collector| collector as &dyn Collector)
            .collect()
    }

    fn connect(&mut self) -> Result<(), CoreError> {
        self.policies = self.discover_policies()?;
        self.collectors = Self::create_collectors(&self.policies)?;
        self.metadata = ProfilerMetadata::new(
            PROFILER_KEY,
            self.collectors
                .iter()
                .map(|collector| collector.metadata().clone())
                .collect(),
        )?;
        self.connected = true;
        Ok(())
    }

    fn start(&mut self) -> Result<(), CoreError> {
        self.ensure_connected("start")?;

        if let Some(runtime) = self.sampler.as_ref() {
            runtime.pause_flag.store(false, Ordering::Release);
            return Ok(());
        }

        let mut server = ADBServer::default();
        let mut device = open_target_device(&mut server, self.serial.as_deref())?;
        let policies = self.policies.clone();
        let writers = self
            .collectors
            .iter()
            .map(|collector| Arc::clone(&collector.value_percent))
            .collect::<Vec<_>>();
        let (stop_tx, stop_rx) = mpsc::channel();
        let pause_flag = Arc::new(AtomicBool::new(false));
        let pause_flag_for_thread = Arc::clone(&pause_flag);
        let sample_interval = self.sample_interval;

        let mut previous = None;
        previous = sample_once(&mut device, &policies, &writers, previous);

        let join_handle = thread::spawn(move || {
            sampling_loop(
                &mut device,
                policies,
                writers,
                stop_rx,
                pause_flag_for_thread,
                sample_interval,
                previous,
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
        let sampler = self
            .sampler
            .as_ref()
            .ok_or_else(|| CoreError::Runtime("CPU_USAGE sampler is not running".to_string()))?;
        sampler.pause_flag.store(true, Ordering::Release);
        Ok(())
    }

    fn restart(&mut self) -> Result<(), CoreError> {
        self.ensure_connected("restart")?;
        let sampler = self
            .sampler
            .as_ref()
            .ok_or_else(|| CoreError::Runtime("CPU_USAGE sampler is not running".to_string()))?;
        sampler.pause_flag.store(false, Ordering::Release);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), CoreError> {
        if let Some(runtime) = self.sampler.take() {
            let _ = runtime.stop_tx.send(());
            let _ = runtime.join_handle.join();
        }

        for collector in &self.collectors {
            collector
                .value_percent
                .store(INVALID_METRIC_VALUE, Ordering::Release);
        }

        self.connected = false;
        Ok(())
    }
}

fn sampling_loop(
    device: &mut ADBServerDevice,
    policies: Vec<PolicyGroup>,
    writers: Vec<Arc<AtomicI64>>,
    stop_rx: Receiver<()>,
    pause_flag: Arc<AtomicBool>,
    interval: Duration,
    mut previous: Option<HashMap<String, CpuTick>>,
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

        previous = sample_once(device, &policies, &writers, previous);

        let now = Instant::now();
        if next_tick > now {
            thread::sleep(next_tick - now);
        } else {
            next_tick = now;
        }
        next_tick += interval;
    }
}

fn sample_once(
    device: &mut impl ADBDeviceExt,
    policies: &[PolicyGroup],
    writers: &[Arc<AtomicI64>],
    previous: Option<HashMap<String, CpuTick>>,
) -> Option<HashMap<String, CpuTick>> {
    let output = match run_shell(device, CPU_STAT_COMMAND) {
        Ok(output) => output,
        Err(_) => {
            mark_all_invalid(writers);
            return previous;
        }
    };

    let current = parse_cpu_stat(&output).into_iter().collect::<HashMap<_, _>>();
    if current.is_empty() {
        mark_all_invalid(writers);
        return previous;
    }

    let Some(previous) = previous else {
        mark_all_invalid(writers);
        return Some(current);
    };

    for (idx, policy) in policies.iter().enumerate() {
        let mut current_total = 0_u64;
        let mut current_idle = 0_u64;
        let mut previous_total = 0_u64;
        let mut previous_idle = 0_u64;
        let mut valid = true;

        for core in &policy.core_names {
            let Some(current_tick) = current.get(core) else {
                valid = false;
                break;
            };
            let Some(previous_tick) = previous.get(core) else {
                valid = false;
                break;
            };

            current_total = current_total.saturating_add(current_tick.total);
            current_idle = current_idle.saturating_add(current_tick.idle);
            previous_total = previous_total.saturating_add(previous_tick.total);
            previous_idle = previous_idle.saturating_add(previous_tick.idle);
        }

        if !valid {
            writers[idx].store(INVALID_METRIC_VALUE, Ordering::Release);
            continue;
        }

        let total_delta = current_total.saturating_sub(previous_total);
        let idle_delta = current_idle.saturating_sub(previous_idle);
        if total_delta == 0 {
            writers[idx].store(INVALID_METRIC_VALUE, Ordering::Release);
            continue;
        }

        let busy_delta = total_delta.saturating_sub(idle_delta);
        let usage = ((busy_delta as f64 / total_delta as f64) * 100.0).round() as i64;
        writers[idx].store(usage.clamp(0, 100), Ordering::Release);
    }

    Some(current)
}

fn parse_cpu_stat(output: &str) -> Vec<(String, CpuTick)> {
    output
        .lines()
        .filter_map(parse_cpu_line)
        .collect::<Vec<(String, CpuTick)>>()
}

fn parse_cpu_line(line: &str) -> Option<(String, CpuTick)> {
    let mut tokens = line.split_whitespace();
    let core = tokens.next()?;
    if !core.starts_with("cpu") || core == "cpu" {
        return None;
    }
    let suffix = core.strip_prefix("cpu")?;
    if suffix.is_empty() || !suffix.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }

    let values = tokens
        .map(|token| token.parse::<u64>())
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if values.len() < 4 {
        return None;
    }

    let idle = values[3] + values.get(4).copied().unwrap_or(0);
    let total = values.into_iter().sum::<u64>();
    Some((core.to_string(), CpuTick { total, idle }))
}

fn parse_policy_listing(output: &str) -> Vec<PolicyGroup> {
    output
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            let (name, cpus) = trimmed.split_once(':')?;
            if !name.starts_with("policy") {
                return None;
            }

            let core_names = parse_related_cpu_set(cpus);
            if core_names.is_empty() {
                return None;
            }

            Some(PolicyGroup {
                name: name.trim().to_string(),
                core_names,
            })
        })
        .collect()
}

fn parse_related_cpu_set(raw: &str) -> Vec<String> {
    let mut cores = Vec::new();

    for part in raw
        .split(|ch: char| ch == ',' || ch.is_whitespace())
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        if let Some((start, end)) = part.split_once('-') {
            let Ok(start) = start.parse::<u32>() else {
                continue;
            };
            let Ok(end) = end.parse::<u32>() else {
                continue;
            };
            if start > end {
                continue;
            }

            for cpu in start..=end {
                cores.push(format!("cpu{cpu}"));
            }
        } else if let Ok(cpu) = part.parse::<u32>() {
            cores.push(format!("cpu{cpu}"));
        }
    }

    cores.sort();
    cores.dedup();
    cores
}

fn mark_all_invalid(writers: &[Arc<AtomicI64>]) {
    for writer in writers {
        writer.store(INVALID_METRIC_VALUE, Ordering::Release);
    }
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
    let mut out = Vec::with_capacity(256);
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

    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_per_core_cpu_stat_lines() {
        let input = "\
cpu  100 0 200 900 10 0 0 0 0 0
cpu0 50 0 100 450 5 0 0 0 0 0
cpu1 50 0 100 450 5 0 0 0 0 0";
        let rows = parse_cpu_stat(input);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "cpu0");
        assert_eq!(rows[1].0, "cpu1");
    }

    #[test]
    fn computes_usage_from_tick_deltas() {
        let previous = HashMap::from([(
            "cpu0".to_string(),
            CpuTick {
                total: 1000,
                idle: 800,
            },
        )]);
        let current = HashMap::from([(
            "cpu0".to_string(),
            CpuTick {
                total: 1100,
                idle: 850,
            },
        )]);
        let writers = vec![Arc::new(AtomicI64::new(INVALID_METRIC_VALUE))];
        let keys = vec!["cpu0".to_string()];

        // 100 total delta, 50 idle delta => 50% usage
        for (idx, key) in keys.iter().enumerate() {
            let current_tick = current.get(key).unwrap();
            let previous_tick = previous.get(key).unwrap();
            let total_delta = current_tick.total.saturating_sub(previous_tick.total);
            let idle_delta = current_tick.idle.saturating_sub(previous_tick.idle);
            let busy_delta = total_delta.saturating_sub(idle_delta);
            let usage = ((busy_delta as f64 / total_delta as f64) * 100.0).round() as i64;
            writers[idx].store(usage.clamp(0, 100), Ordering::Release);
        }

        assert_eq!(writers[0].load(Ordering::Relaxed), 50);
    }

    #[test]
    fn parses_policy_listing_with_related_cpus() {
        let input = "policy0:0 1 2 3\npolicy6:6 7\n";
        let policies = parse_policy_listing(input);
        assert_eq!(policies.len(), 2);
        assert_eq!(policies[0].name, "policy0");
        assert_eq!(policies[0].core_names, vec!["cpu0", "cpu1", "cpu2", "cpu3"]);
        assert_eq!(policies[1].name, "policy6");
        assert_eq!(policies[1].core_names, vec!["cpu6", "cpu7"]);
    }

    #[test]
    fn parses_related_cpu_ranges_and_lists() {
        assert_eq!(
            parse_related_cpu_set("0-3"),
            vec!["cpu0", "cpu1", "cpu2", "cpu3"]
        );
        assert_eq!(
            parse_related_cpu_set("0-1,4,6-7"),
            vec!["cpu0", "cpu1", "cpu4", "cpu6", "cpu7"]
        );
        assert_eq!(
            parse_related_cpu_set("0-3 6 7"),
            vec!["cpu0", "cpu1", "cpu2", "cpu3", "cpu6", "cpu7"]
        );
    }
}
