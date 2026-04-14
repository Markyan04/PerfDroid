use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pdcore::CoreError;
use pdcore::traits::Profiler;
use pdcore::types::ControlCommand;
use profiler_cpu_clock::CpuClockProfiler;
use profiler_fps::FpsProfiler;
use registry::ProfilerRegistry;

use crate::aggregation::{AggregationWorker, AggregatorEvent};
use crate::device::{connect_wireless, list_adb_devices, query_device_descriptor, DeviceDescriptor};
use crate::session::SessionState;

#[derive(Clone)]
pub struct PerfDroidRuntime {
    inner: Arc<Mutex<RuntimeInner>>,
    event_tx: Sender<AggregatorEvent>,
}

struct RuntimeInner {
    state: SessionState,
    cpu_clock_profiler: Option<Arc<Mutex<CpuClockProfiler>>>,
    fps_profiler: Option<Arc<Mutex<FpsProfiler>>>,
    worker: Option<AggregationWorker>,
    registry: ProfilerRegistry,
    device: Option<DeviceDescriptor>,
    selected_hz: u64,
    package_name: String,
}

impl PerfDroidRuntime {
    pub fn new(event_tx: Sender<AggregatorEvent>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(RuntimeInner {
                state: SessionState::Disconnected,
                cpu_clock_profiler: None,
                fps_profiler: None,
                worker: None,
                registry: ProfilerRegistry::default(),
                device: None,
                selected_hz: 4,
                package_name: String::new(),
            })),
            event_tx,
        }
    }

    pub fn request_refresh_devices(&self) {
        match list_adb_devices() {
            Ok(devices) => {
                let count = devices.len();
                let _ = self
                    .event_tx
                    .send(AggregatorEvent::DeviceDiscoveryUpdated(devices));
                let _ = self.event_tx.send(AggregatorEvent::Status(format!(
                    "ADB detection complete: {count} device(s) found."
                )));
            }
            Err(err) => {
                let _ = self.event_tx.send(AggregatorEvent::Status(err));
            }
        }
    }

    pub fn request_connect_usb(&self, serial: String) {
        if let Err(err) = self.connect(serial.clone()) {
            let _ = self.event_tx.send(AggregatorEvent::Status(err.to_string()));
        } else {
            let _ = self.event_tx.send(AggregatorEvent::Status(format!(
                "Connected to `{serial}` through USB."
            )));
        }
    }

    pub fn request_connect_wireless(&self, serial: String) {
        match connect_wireless(&serial) {
            Ok((wireless_serial, detail)) => {
                if let Err(err) = self.connect(wireless_serial.clone()) {
                    let _ = self.event_tx.send(AggregatorEvent::Status(format!(
                        "{detail} Failed to finish wireless session setup for `{wireless_serial}`: {err}"
                    )));
                } else {
                    let _ = self.event_tx.send(AggregatorEvent::Status(format!(
                        "{detail} Connected through WiFi as `{wireless_serial}`."
                    )));
                }
            }
            Err(err) => {
                let _ = self.event_tx.send(AggregatorEvent::Status(err));
            }
        }
    }

    pub fn request_connect(&self, serial: Option<String>) {
        if let Err(err) = self.connect(serial.unwrap_or_else(|| "default".to_string())) {
            let _ = self.event_tx.send(AggregatorEvent::Status(err.to_string()));
        }
    }

    pub fn request_start(&self, hz: u64) {
        if let Err(err) = self.start(hz) {
            let _ = self.event_tx.send(AggregatorEvent::Status(err.to_string()));
        }
    }

    pub fn request_pause(&self) {
        if let Err(err) = self.pause() {
            let _ = self.event_tx.send(AggregatorEvent::Status(err.to_string()));
        }
    }

    pub fn request_restart(&self) {
        if let Err(err) = self.restart() {
            let _ = self.event_tx.send(AggregatorEvent::Status(err.to_string()));
        }
    }

    pub fn request_stop(&self) {
        if let Err(err) = self.stop() {
            let _ = self.event_tx.send(AggregatorEvent::Status(err.to_string()));
        }
    }

    pub fn request_set_hz(&self, hz: u64) -> Result<(), CoreError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| CoreError::Runtime("runtime lock poisoned".to_string()))?;
        inner.selected_hz = hz.clamp(1, 10);
        let _ = self
            .event_tx
            .send(AggregatorEvent::SamplingRateChanged(inner.selected_hz));
        Ok(())
    }

    pub fn selected_hz(&self) -> u64 {
        self.inner
            .lock()
            .map(|inner| inner.selected_hz)
            .unwrap_or(4)
    }

    pub fn request_set_package_name(
        &self,
        package_name: impl Into<String>,
    ) -> Result<(), CoreError> {
        let package_name = package_name.into();
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| CoreError::Runtime("runtime lock poisoned".to_string()))?;
        inner.package_name = package_name.clone();

        if let Some(profiler) = inner.fps_profiler.as_ref() {
            profiler
                .lock()
                .map_err(|_| CoreError::Runtime("FPS profiler lock poisoned".to_string()))?
                .set_package_name(package_name.clone());
        }

        let _ = self
            .event_tx
            .send(AggregatorEvent::PackageNameChanged(package_name));
        Ok(())
    }

    pub fn package_name(&self) -> String {
        self.inner
            .lock()
            .map(|inner| inner.package_name.clone())
            .unwrap_or_default()
    }

    pub fn state(&self) -> SessionState {
        self.inner
            .lock()
            .map(|inner| inner.state)
            .unwrap_or(SessionState::Disconnected)
    }

    fn connect(&self, serial: String) -> Result<(), CoreError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| CoreError::Runtime("runtime lock poisoned".to_string()))?;
        ensure_transition(inner.state, ControlCommand::Connect)?;

        let serial_arg = if serial == "default" {
            None
        } else {
            Some(serial.as_str())
        };
        let device = query_device_descriptor(serial_arg).map_err(CoreError::Runtime)?;
        inner.registry.clear();
        inner.device = Some(device.clone());

        let mut cpu_clock_profiler =
            CpuClockProfiler::new(serial_arg.map(str::to_string), Duration::from_millis(100))?;
        cpu_clock_profiler.connect()?;
        let cpu_clock_metadata = cpu_clock_profiler.metadata_clone();
        inner.registry.register(cpu_clock_metadata.clone());
        inner.cpu_clock_profiler = Some(Arc::new(Mutex::new(cpu_clock_profiler)));

        let fps_profiler = match FpsProfiler::new(
            serial_arg.map(str::to_string),
            Duration::from_secs(1),
            inner.package_name.clone(),
        ) {
            Ok(mut profiler) => match profiler.connect() {
                Ok(()) => {
                    let metadata = profiler.metadata_clone();
                    inner.registry.register(metadata.clone());
                    Some((Arc::new(Mutex::new(profiler)), metadata))
                }
                Err(err) => {
                    let _ = self.event_tx.send(AggregatorEvent::Status(format!(
                        "FPS profiler disabled: {err}"
                    )));
                    None
                }
            },
            Err(err) => {
                let _ = self.event_tx.send(AggregatorEvent::Status(format!(
                    "FPS profiler construction failed: {err}"
                )));
                None
            }
        };
        inner.fps_profiler = fps_profiler
            .as_ref()
            .map(|(profiler, _)| Arc::clone(profiler));
        inner.state = SessionState::Connected;

        let _ = self.event_tx.send(AggregatorEvent::DeviceUpdated(device));
        let _ = self
            .event_tx
            .send(AggregatorEvent::MetadataRegistered(format!(
                "{} [{}]",
                cpu_clock_metadata.profiler_key,
                cpu_clock_metadata
                    .ordered_collectors()
                    .iter()
                    .map(|collector| collector.collector_key.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        if let Some((_, metadata)) = fps_profiler {
            let _ = self
                .event_tx
                .send(AggregatorEvent::MetadataRegistered(format!(
                    "{} [{}]",
                    metadata.profiler_key,
                    metadata
                        .ordered_collectors()
                        .iter()
                        .map(|collector| collector.collector_key.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
        }
        let _ = self
            .event_tx
            .send(AggregatorEvent::SamplingRateChanged(inner.selected_hz));
        let _ = self.event_tx.send(AggregatorEvent::PackageNameChanged(
            inner.package_name.clone(),
        ));
        let _ = self
            .event_tx
            .send(AggregatorEvent::StateChanged(SessionState::Connected));
        Ok(())
    }

    fn start(&self, hz: u64) -> Result<(), CoreError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| CoreError::Runtime("runtime lock poisoned".to_string()))?;
        ensure_transition(inner.state, ControlCommand::Start)?;
        inner.selected_hz = hz.clamp(1, 10);

        let cpu_clock_profiler = inner.cpu_clock_profiler.clone();
        let fps_profiler = inner.fps_profiler.clone();

        if let Some(profiler) = cpu_clock_profiler.as_ref() {
            profiler
                .lock()
                .map_err(|_| CoreError::Runtime("CPU_CLOCK profiler lock poisoned".to_string()))?
                .start()?;
        }
        if let Some(profiler) = fps_profiler.as_ref() {
            profiler
                .lock()
                .map_err(|_| CoreError::Runtime("FPS profiler lock poisoned".to_string()))?
                .start()?;
        }

        inner.worker = Some(
            AggregationWorker::spawn(
                cpu_clock_profiler,
                fps_profiler,
                inner.selected_hz,
                self.event_tx.clone(),
            )
            .map_err(CoreError::Runtime)?,
        );
        let _ = self
            .event_tx
            .send(AggregatorEvent::SamplingRateChanged(inner.selected_hz));
        inner.state = SessionState::Running;
        let _ = self
            .event_tx
            .send(AggregatorEvent::StateChanged(SessionState::Running));
        Ok(())
    }

    fn pause(&self) -> Result<(), CoreError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| CoreError::Runtime("runtime lock poisoned".to_string()))?;
        ensure_transition(inner.state, ControlCommand::Pause)?;

        if let Some(profiler) = inner.cpu_clock_profiler.as_ref() {
            profiler
                .lock()
                .map_err(|_| CoreError::Runtime("CPU_CLOCK profiler lock poisoned".to_string()))?
                .pause()?;
        }
        if let Some(profiler) = inner.fps_profiler.as_ref() {
            profiler
                .lock()
                .map_err(|_| CoreError::Runtime("FPS profiler lock poisoned".to_string()))?
                .pause()?;
        }
        if let Some(worker) = inner.worker.as_ref() {
            worker.pause();
        }
        inner.state = SessionState::Paused;
        let _ = self
            .event_tx
            .send(AggregatorEvent::StateChanged(SessionState::Paused));
        Ok(())
    }

    fn restart(&self) -> Result<(), CoreError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| CoreError::Runtime("runtime lock poisoned".to_string()))?;
        ensure_transition(inner.state, ControlCommand::Restart)?;

        if let Some(profiler) = inner.cpu_clock_profiler.as_ref() {
            profiler
                .lock()
                .map_err(|_| CoreError::Runtime("CPU_CLOCK profiler lock poisoned".to_string()))?
                .restart()?;
        }
        if let Some(profiler) = inner.fps_profiler.as_ref() {
            profiler
                .lock()
                .map_err(|_| CoreError::Runtime("FPS profiler lock poisoned".to_string()))?
                .restart()?;
        }
        if let Some(worker) = inner.worker.as_ref() {
            worker.restart();
        }
        inner.state = SessionState::Running;
        let _ = self
            .event_tx
            .send(AggregatorEvent::StateChanged(SessionState::Running));
        Ok(())
    }

    fn stop(&self) -> Result<(), CoreError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| CoreError::Runtime("runtime lock poisoned".to_string()))?;
        ensure_transition(inner.state, ControlCommand::Stop)?;

        if let Some(worker) = inner.worker.as_mut() {
            worker.stop();
        }
        inner.worker = None;

        if let Some(profiler) = inner.cpu_clock_profiler.as_ref() {
            profiler
                .lock()
                .map_err(|_| CoreError::Runtime("CPU_CLOCK profiler lock poisoned".to_string()))?
                .stop()?;
        }
        if let Some(profiler) = inner.fps_profiler.as_ref() {
            profiler
                .lock()
                .map_err(|_| CoreError::Runtime("FPS profiler lock poisoned".to_string()))?
                .stop()?;
        }
        inner.cpu_clock_profiler = None;
        inner.fps_profiler = None;
        inner.state = SessionState::Stopped;
        let _ = self
            .event_tx
            .send(AggregatorEvent::StateChanged(SessionState::Stopped));
        Ok(())
    }
}

fn ensure_transition(state: SessionState, command: ControlCommand) -> Result<(), CoreError> {
    if state.allows(command) {
        Ok(())
    } else {
        Err(CoreError::InvalidStateTransition {
            state: state.as_str().to_string(),
            operation: command.as_str().to_string(),
        })
    }
}
