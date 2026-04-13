pub mod metadata;

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

use crate::metadata::{PROFILER_KEY, UNIT_MHZ};

const POLICY_DISCOVERY_COMMAND: &str = "for p in /sys/devices/system/cpu/cpufreq/policy*; do bn=$(basename $p); cpus=$(cat $p/related_cpus 2>/dev/null); echo $bn:$cpus; done";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuPolicy {
    pub policy_name: String,
    pub related_cpus: String,
}

#[derive(Debug)]
pub struct CpuClockCollector {
    metadata: CollectorMetadata,
    policy_name: String,
    related_cpus: String,
    value_mhz: Arc<AtomicI64>,
}

impl CpuClockCollector {
    fn new(policy: CpuPolicy, order: usize) -> Result<Self, CoreError> {
        Ok(Self {
            metadata: CollectorMetadata::new(policy.policy_name.clone(), UNIT_MHZ, order)?,
            policy_name: policy.policy_name,
            related_cpus: policy.related_cpus,
            value_mhz: Arc::new(AtomicI64::new(INVALID_METRIC_VALUE)),
        })
    }

    pub fn policy_name(&self) -> &str {
        &self.policy_name
    }

    pub fn related_cpus(&self) -> &str {
        &self.related_cpus
    }
}

impl Collector for CpuClockCollector {
    fn metadata(&self) -> &CollectorMetadata {
        &self.metadata
    }

    fn read_buffer(&self, ordering: Ordering) -> i64 {
        self.value_mhz.load(ordering)
    }
}

#[derive(Debug)]
struct SamplerRuntime {
    stop_tx: Sender<()>,
    pause_flag: Arc<AtomicBool>,
    join_handle: JoinHandle<()>,
}

#[derive(Debug)]
pub struct CpuClockProfiler {
    serial: Option<String>,
    sample_interval: Duration,
    metadata: ProfilerMetadata,
    collectors: Vec<CpuClockCollector>,
    connected: bool,
    sampler: Option<SamplerRuntime>,
}

impl CpuClockProfiler {
    pub fn new(serial: Option<String>, sample_interval: Duration) -> Result<Self, CoreError> {
        Ok(Self {
            serial,
            sample_interval,
            metadata: ProfilerMetadata::new(
                PROFILER_KEY,
                vec![CollectorMetadata::new("policy0", UNIT_MHZ, 0)?],
            )?,
            collectors: Vec::new(),
            connected: false,
            sampler: None,
        })
    }

    pub fn discover_policies(&mut self) -> Result<Vec<CpuPolicy>, CoreError> {
        let mut server = ADBServer::default();
        let mut device = open_target_device(&mut server, self.serial.as_deref())?;
        let output = run_shell(&mut device, POLICY_DISCOVERY_COMMAND)?;
        let policies = parse_policy_listing(&output);

        if policies.is_empty() {
            return Err(CoreError::Runtime(
                "no readable CPU frequency policy was found on the target device".to_string(),
            ));
        }

        Ok(policies)
    }

    pub fn snapshot_values(&self) -> Vec<i64> {
        self.collectors
            .iter()
            .map(|collector| collector.read_buffer(Ordering::Relaxed))
            .collect()
    }

    pub fn collector_labels(&self) -> Vec<String> {
        self.collectors
            .iter()
            .map(|collector| format!("{} ({})", collector.policy_name(), collector.related_cpus()))
            .collect()
    }

    pub fn metadata_clone(&self) -> ProfilerMetadata {
        self.metadata.clone()
    }

    fn ensure_connected(&self, operation: &'static str) -> Result<(), CoreError> {
        if self.connected {
            Ok(())
        } else {
            Err(CoreError::Runtime(format!(
                "CPU_CLOCK profiler must be connected before `{operation}`"
            )))
        }
    }

    fn create_collectors(policies: Vec<CpuPolicy>) -> Result<Vec<CpuClockCollector>, CoreError> {
        policies
            .into_iter()
            .enumerate()
            .map(|(order, policy)| CpuClockCollector::new(policy, order))
            .collect()
    }
}

impl Profiler for CpuClockProfiler {
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
        let policies = self.discover_policies()?;
        self.collectors = Self::create_collectors(policies)?;
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
        let policy_names = self
            .collectors
            .iter()
            .map(|collector| collector.policy_name.clone())
            .collect::<Vec<_>>();
        let sample_command = build_sampling_command(&policy_names);
        let writers = self
            .collectors
            .iter()
            .map(|collector| Arc::clone(&collector.value_mhz))
            .collect::<Vec<_>>();
        let (stop_tx, stop_rx) = mpsc::channel();
        let pause_flag = Arc::new(AtomicBool::new(false));
        let pause_flag_for_thread = Arc::clone(&pause_flag);
        let sample_interval = self.sample_interval;

        sample_once(&mut device, &sample_command, &writers)?;

        let sample_command_for_thread = sample_command.clone();
        let join_handle = thread::spawn(move || {
            sampling_loop(
                &mut device,
                &sample_command_for_thread,
                writers,
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
        let sampler = self
            .sampler
            .as_ref()
            .ok_or_else(|| CoreError::Runtime("CPU_CLOCK sampler is not running".to_string()))?;
        sampler.pause_flag.store(true, Ordering::Release);
        Ok(())
    }

    fn restart(&mut self) -> Result<(), CoreError> {
        self.ensure_connected("restart")?;
        let sampler = self
            .sampler
            .as_ref()
            .ok_or_else(|| CoreError::Runtime("CPU_CLOCK sampler is not running".to_string()))?;
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
                .value_mhz
                .store(INVALID_METRIC_VALUE, Ordering::Release);
        }

        self.connected = false;
        Ok(())
    }
}

fn sampling_loop(
    device: &mut impl ADBDeviceExt,
    sample_command: &str,
    writers: Vec<Arc<AtomicI64>>,
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

        if sample_once(device, sample_command, &writers).is_err() {
            mark_all_invalid(&writers);
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

fn build_sampling_command(policy_names: &[String]) -> String {
    let policies = policy_names
        .iter()
        .map(|policy| format!("cat /sys/devices/system/cpu/cpufreq/{policy}/scaling_cur_freq 2>/dev/null || echo -1"))
        .collect::<Vec<_>>()
        .join("; ");

    format!("for x in 1; do {policies}; done")
}

fn sample_once(
    device: &mut impl ADBDeviceExt,
    sample_command: &str,
    writers: &[Arc<AtomicI64>],
) -> Result<(), CoreError> {
    let output = run_shell(device, sample_command)?;
    let values = parse_sampling_output(&output);

    if values.len() != writers.len() {
        mark_all_invalid(writers);
        return Err(CoreError::Runtime(format!(
            "CPU_CLOCK sample returned {} values, expected {}",
            values.len(),
            writers.len()
        )));
    }

    for (writer, value) in writers.iter().zip(values) {
        writer.store(value, Ordering::Release);
    }

    Ok(())
}

fn parse_policy_listing(output: &str) -> Vec<CpuPolicy> {
    output
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }

            let (policy_name, related_cpus) = trimmed.split_once(':')?;
            Some(CpuPolicy {
                policy_name: policy_name.trim().to_string(),
                related_cpus: related_cpus.trim().to_string(),
            })
        })
        .collect()
}

fn parse_sampling_output(output: &str) -> Vec<i64> {
    output
        .lines()
        .filter_map(|line| {
            let raw = line.trim().parse::<i64>().ok()?;
            if raw < 0 {
                Some(INVALID_METRIC_VALUE)
            } else {
                Some(raw / 1000)
            }
        })
        .collect()
}

fn mark_all_invalid(writers: &[Arc<AtomicI64>]) {
    for writer in writers {
        writer.store(INVALID_METRIC_VALUE, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_policy_listing, parse_sampling_output};
    use pdcore::INVALID_METRIC_VALUE;

    #[test]
    fn parses_policy_listing() {
        let policies = parse_policy_listing("policy0:0 1 2 3\npolicy6:6 7\n");
        assert_eq!(policies.len(), 2);
        assert_eq!(policies[0].policy_name, "policy0");
        assert_eq!(policies[1].related_cpus, "6 7");
    }

    #[test]
    fn parses_sampling_output_to_mhz() {
        let values = parse_sampling_output("710400\n1305600\n-1\n");
        assert_eq!(values, vec![710, 1305, INVALID_METRIC_VALUE]);
    }
}
