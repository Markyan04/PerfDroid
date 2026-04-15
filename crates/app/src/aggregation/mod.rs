use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pdcore::CoreError;
use pdcore::types::MetricBatch;
use profiler_cpu_clock::CpuClockProfiler;
use profiler_cpu_usage::CpuUsageProfiler;
use profiler_fps::FpsProfiler;

use crate::device::{AdbDetectedDevice, DeviceDescriptor};
use crate::session::SessionState;
use crate::storage::TimestampedBatch;

#[derive(Debug, Clone)]
pub enum AggregatorEvent {
    StateChanged(SessionState),
    DeviceUpdated(DeviceDescriptor),
    DeviceDiscoveryUpdated(Vec<AdbDetectedDevice>),
    MetadataRegistered(String),
    MetricBatch(TimestampedBatch),
    PackageNameChanged(String),
    SamplingRateChanged(u64),
    Status(String),
}

pub struct CpuClockDataPlane;

impl CpuClockDataPlane {
    pub fn build_metric_batch(profiler: &CpuClockProfiler) -> Result<MetricBatch, CoreError> {
        let metadata = profiler.metadata_clone();
        let ordered = metadata.ordered_collectors();
        let values_by_key = metadata
            .collector
            .iter()
            .map(|collector| (collector.collector_key.clone(), collector.order))
            .collect::<std::collections::HashMap<_, _>>();

        let snapshot = profiler.snapshot_values();
        let mut ordered_values = vec![pdcore::INVALID_METRIC_VALUE; snapshot.len()];
        for collector in ordered {
            if let Some(index) = values_by_key.get(&collector.collector_key) {
                ordered_values[collector.order] = snapshot[*index];
            }
        }

        MetricBatch::new(metadata.profiler_key, "MHz", ordered_values)
    }
}

pub struct FpsDataPlane;

impl FpsDataPlane {
    pub fn build_metric_batch(profiler: &FpsProfiler) -> Result<MetricBatch, CoreError> {
        let metadata = profiler.metadata_clone();
        MetricBatch::new(
            metadata.profiler_key,
            "FPS",
            vec![profiler.snapshot_value()],
        )
    }
}

pub struct CpuUsageDataPlane;

impl CpuUsageDataPlane {
    pub fn build_metric_batch(profiler: &CpuUsageProfiler) -> Result<MetricBatch, CoreError> {
        let metadata = profiler.metadata_clone();
        MetricBatch::new(metadata.profiler_key, "%", profiler.snapshot_values())
    }
}

pub struct AggregationWorker {
    stop_tx: Sender<()>,
    pause_flag: Arc<AtomicBool>,
    join_handle: Option<JoinHandle<()>>,
}

impl AggregationWorker {
    pub fn spawn(
        cpu_clock_profiler: Option<Arc<Mutex<CpuClockProfiler>>>,
        cpu_usage_profiler: Option<Arc<Mutex<CpuUsageProfiler>>>,
        fps_profiler: Option<Arc<Mutex<FpsProfiler>>>,
        hz: u64,
        tx: Sender<AggregatorEvent>,
    ) -> Result<Self, String> {
        let interval = Duration::from_secs_f64(1.0 / hz.max(1) as f64);
        let pause_flag = Arc::new(AtomicBool::new(false));
        let pause_flag_for_thread = Arc::clone(&pause_flag);
        let (stop_tx, stop_rx) = mpsc::channel();

        let join_handle =
            thread::spawn(move || {
                let mut next_tick = Instant::now();

                loop {
                    if stop_rx.try_recv().is_ok() {
                        break;
                    }

                    if pause_flag_for_thread.load(Ordering::Acquire) {
                        thread::sleep(Duration::from_millis(20));
                        continue;
                    }

                    if let Some(profiler) = cpu_clock_profiler.as_ref() {
                        let maybe_batch = profiler.lock().ok().and_then(|profiler| {
                            CpuClockDataPlane::build_metric_batch(&profiler).ok()
                        });

                        if let Some(batch) = maybe_batch {
                            let _ = tx.send(AggregatorEvent::MetricBatch(TimestampedBatch {
                                timestamp_ms: unix_timestamp_ms(),
                                batch,
                            }));
                        }
                    }

                    if let Some(profiler) = cpu_usage_profiler.as_ref() {
                        let maybe_batch = profiler.lock().ok().and_then(|profiler| {
                            CpuUsageDataPlane::build_metric_batch(&profiler).ok()
                        });

                        if let Some(batch) = maybe_batch {
                            let _ = tx.send(AggregatorEvent::MetricBatch(TimestampedBatch {
                                timestamp_ms: unix_timestamp_ms(),
                                batch,
                            }));
                        }
                    }

                    if let Some(profiler) = fps_profiler.as_ref() {
                        let maybe_batch = profiler
                            .lock()
                            .ok()
                            .and_then(|profiler| FpsDataPlane::build_metric_batch(&profiler).ok());

                        if let Some(batch) = maybe_batch {
                            let _ = tx.send(AggregatorEvent::MetricBatch(TimestampedBatch {
                                timestamp_ms: unix_timestamp_ms(),
                                batch,
                            }));
                        }
                    }

                    let now = Instant::now();
                    if next_tick > now {
                        thread::sleep(next_tick - now);
                    } else {
                        next_tick = now;
                    }
                    next_tick += interval;
                }
            });

        Ok(Self {
            stop_tx,
            pause_flag,
            join_handle: Some(join_handle),
        })
    }

    pub fn pause(&self) {
        self.pause_flag.store(true, Ordering::Release);
    }

    pub fn restart(&self) {
        self.pause_flag.store(false, Ordering::Release);
    }

    pub fn stop(&mut self) {
        let _ = self.stop_tx.send(());
        if let Some(join_handle) = self.join_handle.take() {
            let _ = join_handle.join();
        }
    }
}

fn unix_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
