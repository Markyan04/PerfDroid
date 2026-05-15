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

use crate::metadata::{COLLECTOR_KEY, PROFILER_KEY, UNIT_DECI_C};

const BATTERY_DUMP_COMMAND: &str = "dumpsys battery";

#[derive(Debug)]
pub struct BatteryTemperatureCollector {
    metadata: CollectorMetadata,
    value_deci_c: Arc<AtomicI64>,
}

impl BatteryTemperatureCollector {
    fn new() -> Result<Self, CoreError> {
        Ok(Self {
            metadata: CollectorMetadata::new(COLLECTOR_KEY, UNIT_DECI_C, 0)?,
            value_deci_c: Arc::new(AtomicI64::new(INVALID_METRIC_VALUE)),
        })
    }
}

impl Collector for BatteryTemperatureCollector {
    fn metadata(&self) -> &CollectorMetadata {
        &self.metadata
    }

    fn read_buffer(&self, ordering: Ordering) -> i64 {
        self.value_deci_c.load(ordering)
    }
}

#[derive(Debug)]
struct SamplerRuntime {
    stop_tx: Sender<()>,
    pause_flag: Arc<AtomicBool>,
    join_handle: JoinHandle<()>,
}

#[derive(Debug)]
pub struct BatteryTemperatureProfiler {
    serial: Option<String>,
    sample_interval: Duration,
    metadata: ProfilerMetadata,
    collector: BatteryTemperatureCollector,
    connected: bool,
    sampler: Option<SamplerRuntime>,
}

impl BatteryTemperatureProfiler {
    pub fn new(serial: Option<String>, sample_interval: Duration) -> Result<Self, CoreError> {
        let collector = BatteryTemperatureCollector::new()?;
        Ok(Self {
            serial,
            sample_interval,
            metadata: ProfilerMetadata::new(PROFILER_KEY, vec![collector.metadata().clone()])?,
            collector,
            connected: false,
            sampler: None,
        })
    }

    pub fn metadata_clone(&self) -> ProfilerMetadata {
        self.metadata.clone()
    }

    pub fn snapshot_value(&self) -> i64 {
        self.collector.read_buffer(Ordering::Relaxed)
    }

    fn ensure_connected(&self, operation: &'static str) -> Result<(), CoreError> {
        if self.connected {
            Ok(())
        } else {
            Err(CoreError::Runtime(format!(
                "BATTERY_TEMP profiler must be connected before `{operation}`"
            )))
        }
    }
}

impl Profiler for BatteryTemperatureProfiler {
    fn metadata(&self) -> &ProfilerMetadata {
        &self.metadata
    }

    fn collectors(&self) -> Vec<&dyn Collector> {
        vec![&self.collector]
    }

    fn connect(&mut self) -> Result<(), CoreError> {
        let mut server = workspace_adb_server();
        let mut device = open_target_device(&mut server, self.serial.as_deref())?;

        sample_once(&mut device, &self.collector.value_deci_c)?;

        if self.snapshot_value() == INVALID_METRIC_VALUE {
            return Err(CoreError::Runtime(
                "failed to read a usable battery temperature from `adb shell dumpsys battery`"
                    .to_string(),
            ));
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
        let writer = Arc::clone(&self.collector.value_deci_c);
        let (stop_tx, stop_rx) = mpsc::channel();
        let pause_flag = Arc::new(AtomicBool::new(false));
        let pause_flag_for_thread = Arc::clone(&pause_flag);
        let sample_interval = self.sample_interval;

        sample_once(&mut device, &writer)?;

        let join_handle = thread::spawn(move || {
            sampling_loop(
                &mut device,
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
        let sampler = self
            .sampler
            .as_ref()
            .ok_or_else(|| CoreError::Runtime("BATTERY_TEMP sampler is not running".to_string()))?;
        sampler.pause_flag.store(true, Ordering::Release);
        Ok(())
    }

    fn restart(&mut self) -> Result<(), CoreError> {
        self.ensure_connected("restart")?;
        let sampler = self
            .sampler
            .as_ref()
            .ok_or_else(|| CoreError::Runtime("BATTERY_TEMP sampler is not running".to_string()))?;
        sampler.pause_flag.store(false, Ordering::Release);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), CoreError> {
        if let Some(runtime) = self.sampler.take() {
            let _ = runtime.stop_tx.send(());
            let _ = runtime.join_handle.join();
        }

        self.collector
            .value_deci_c
            .store(INVALID_METRIC_VALUE, Ordering::Release);
        self.connected = false;
        Ok(())
    }
}

fn sampling_loop(
    device: &mut ADBServerDevice,
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

        if sample_once(device, &writer).is_err() {
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

fn sample_once(device: &mut impl ADBDeviceExt, writer: &Arc<AtomicI64>) -> Result<(), CoreError> {
    let output = run_shell(device, BATTERY_DUMP_COMMAND)?;
    let value = parse_battery_temperature(&output).unwrap_or(INVALID_METRIC_VALUE);
    writer.store(value, Ordering::Release);
    Ok(())
}

fn parse_battery_temperature(output: &str) -> Option<i64> {
    output.lines().find_map(|line| {
        let trimmed = line.trim();
        let raw = trimmed
            .strip_prefix("temperature:")?
            .trim()
            .split_whitespace()
            .next()?;
        raw.parse::<i64>().ok()
    })
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
    use super::parse_battery_temperature;

    #[test]
    fn parses_standard_dumpsys_battery_temperature() {
        let output = "\
Current Battery Service state:
  AC powered: false
  USB powered: true
  level: 79
  voltage: 4321
  temperature: 295
  technology: Li-ion";
        assert_eq!(parse_battery_temperature(output), Some(295));
    }

    #[test]
    fn parses_temperature_with_extra_spacing() {
        let output = "  temperature:    321   ";
        assert_eq!(parse_battery_temperature(output), Some(321));
    }

    #[test]
    fn missing_temperature_returns_none() {
        let output = "level: 80\nvoltage: 4300\ntechnology: Li-ion";
        assert_eq!(parse_battery_temperature(output), None);
    }
}
