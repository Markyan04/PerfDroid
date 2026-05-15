pub mod metadata;

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use adb_client::{ADBDeviceExt, server::ADBServer, server_device::ADBServerDevice};
use pdcore::adb::workspace_adb_server;
use pdcore::traits::{Collector, Profiler};
use pdcore::types::{CollectorMetadata, ProfilerMetadata};
use pdcore::{CoreError, INVALID_METRIC_VALUE};

use crate::metadata::{COLLECTOR_KEY, PROFILER_KEY, UNIT_FPS};

const TIMESTATS_ENABLE_COMMAND: &str = "dumpsys SurfaceFlinger --timestats -clear -enable";
const TIMESTATS_DUMP_COMMAND: &str = "dumpsys SurfaceFlinger --timestats -dump";

#[derive(Debug)]
pub struct FpsCollector {
    metadata: CollectorMetadata,
    value_fps: Arc<AtomicI64>,
}

impl FpsCollector {
    fn new() -> Result<Self, CoreError> {
        Ok(Self {
            metadata: CollectorMetadata::new(COLLECTOR_KEY, UNIT_FPS, 0)?,
            value_fps: Arc::new(AtomicI64::new(INVALID_METRIC_VALUE)),
        })
    }
}

impl Collector for FpsCollector {
    fn metadata(&self) -> &CollectorMetadata {
        &self.metadata
    }

    fn read_buffer(&self, ordering: Ordering) -> i64 {
        self.value_fps.load(ordering)
    }
}

#[derive(Debug)]
struct SamplerRuntime {
    stop_tx: Sender<()>,
    pause_flag: Arc<AtomicBool>,
    join_handle: JoinHandle<()>,
}

#[derive(Debug)]
pub struct FpsProfiler {
    serial: Option<String>,
    sample_interval: Duration,
    metadata: ProfilerMetadata,
    collector: FpsCollector,
    package_name: Arc<Mutex<String>>,
    connected: bool,
    sampler: Option<SamplerRuntime>,
}

impl FpsProfiler {
    pub fn new(
        serial: Option<String>,
        sample_interval: Duration,
        package_name: impl Into<String>,
    ) -> Result<Self, CoreError> {
        let collector = FpsCollector::new()?;
        Ok(Self {
            serial,
            sample_interval,
            metadata: ProfilerMetadata::new(PROFILER_KEY, vec![collector.metadata().clone()])?,
            collector,
            package_name: Arc::new(Mutex::new(package_name.into())),
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

    pub fn package_name(&self) -> String {
        self.package_name
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default()
    }

    pub fn set_package_name(&mut self, package_name: impl Into<String>) {
        if let Ok(mut value) = self.package_name.lock() {
            *value = package_name.into();
        }
    }

    fn ensure_connected(&self, operation: &'static str) -> Result<(), CoreError> {
        if self.connected {
            Ok(())
        } else {
            Err(CoreError::Runtime(format!(
                "FPS profiler must be connected before `{operation}`"
            )))
        }
    }
}

impl Profiler for FpsProfiler {
    fn metadata(&self) -> &ProfilerMetadata {
        &self.metadata
    }

    fn collectors(&self) -> Vec<&dyn Collector> {
        vec![&self.collector]
    }

    fn connect(&mut self) -> Result<(), CoreError> {
        let mut server = workspace_adb_server();
        let mut device = open_target_device(&mut server, self.serial.as_deref())?;
        let _ = run_shell(&mut device, TIMESTATS_ENABLE_COMMAND)?;
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
        let (stop_tx, stop_rx) = mpsc::channel();
        let pause_flag = Arc::new(AtomicBool::new(false));
        let pause_flag_for_thread = Arc::clone(&pause_flag);
        let sample_interval = self.sample_interval;
        let writer = Arc::clone(&self.collector.value_fps);
        let package_name = Arc::clone(&self.package_name);

        let _ = run_shell(&mut device, TIMESTATS_ENABLE_COMMAND)?;
        sample_once(&mut device, &writer, &self.package_name)?;

        let join_handle = thread::spawn(move || {
            sampling_loop(
                &mut device,
                writer,
                package_name,
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
            .ok_or_else(|| CoreError::Runtime("FPS sampler is not running".to_string()))?;
        sampler.pause_flag.store(true, Ordering::Release);
        Ok(())
    }

    fn restart(&mut self) -> Result<(), CoreError> {
        self.ensure_connected("restart")?;
        let sampler = self
            .sampler
            .as_ref()
            .ok_or_else(|| CoreError::Runtime("FPS sampler is not running".to_string()))?;
        sampler.pause_flag.store(false, Ordering::Release);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), CoreError> {
        if let Some(runtime) = self.sampler.take() {
            let _ = runtime.stop_tx.send(());
            let _ = runtime.join_handle.join();
        }

        self.collector
            .value_fps
            .store(INVALID_METRIC_VALUE, Ordering::Release);
        self.connected = false;
        Ok(())
    }
}

fn sampling_loop(
    device: &mut ADBServerDevice,
    writer: Arc<AtomicI64>,
    package_name: Arc<Mutex<String>>,
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

        if sample_once(device, &writer, &package_name).is_err() {
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

fn sample_once(
    device: &mut ADBServerDevice,
    writer: &Arc<AtomicI64>,
    package_name: &Arc<Mutex<String>>,
) -> Result<(), CoreError> {
    let output = run_shell(device, TIMESTATS_DUMP_COMMAND)?;
    let package_name = package_name
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    let value = parse_fps_dump(&output, &package_name).unwrap_or(INVALID_METRIC_VALUE);
    writer.store(value, Ordering::Release);
    let _ = run_shell(device, TIMESTATS_ENABLE_COMMAND)?;
    Ok(())
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

fn parse_fps_dump(output: &str, package_name: &str) -> Option<i64> {
    if package_name.trim().is_empty() {
        return None;
    }

    let mut current_layer = None::<String>;
    let mut best = None::<i64>;

    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(rest) = extract_field_value(trimmed, "layerName") {
            current_layer = Some(rest.trim_matches('"').trim().to_string());
            continue;
        }

        if let Some(rest) = extract_field_value(trimmed, "averageFPS") {
            let raw = rest.trim().parse::<f64>().ok()?;
            let value = raw.round() as i64;
            if is_interesting_layer(current_layer.as_deref(), package_name) && value > 0 {
                best = Some(best.map_or(value, |prev| prev.max(value)));
            }
        }
    }

    best
}

fn extract_field_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    if let Some(rest) = line.strip_prefix(&format!("{key}=")) {
        return Some(rest);
    }
    if let Some(rest) = line.strip_prefix(&format!("{key}:")) {
        return Some(rest);
    }
    if let Some((prefix, rest)) = line.split_once('=') {
        if prefix.trim() == key {
            return Some(rest);
        }
    }
    if let Some((prefix, rest)) = line.split_once(':') {
        if prefix.trim() == key {
            return Some(rest);
        }
    }
    None
}

fn is_interesting_layer(layer_name: Option<&str>, package_name: &str) -> bool {
    let Some(layer_name) = layer_name else {
        return false;
    };

    let lowered = layer_name.to_ascii_lowercase();
    let package_name = package_name.trim().to_ascii_lowercase();
    if !lowered.contains(&package_name) {
        return false;
    }

    !lowered.contains("statusbar")
        && !lowered.contains("navigationbar")
        && !lowered.contains("systemui")
        && !lowered.contains("splash")
}

#[cfg(test)]
mod tests {
    use super::parse_fps_dump;

    #[test]
    fn parses_average_fps_from_dump() {
        let output = "layerName=com.demo.game/MainActivity#0\naverageFPS=58.6\n";
        assert_eq!(parse_fps_dump(output, "com.demo.game"), Some(59));
    }

    #[test]
    fn parses_average_fps_with_spaces_and_colons() {
        let output = "layerName = com.demo.game/MainActivity#0\naverageFPS : 60.0\n";
        assert_eq!(parse_fps_dump(output, "com.demo.game"), Some(60));
    }

    #[test]
    fn ignores_system_layers() {
        let output = "layerName=com.android.systemui.statusbar\naverageFPS=120.0\nlayerName=com.demo.game\naverageFPS=59.2\n";
        assert_eq!(parse_fps_dump(output, "com.demo.game"), Some(59));
    }

    #[test]
    fn empty_package_name_disables_matching() {
        let output = "layerName=com.demo.game\naverageFPS=59.2\n";
        assert_eq!(parse_fps_dump(output, ""), None);
    }
}
