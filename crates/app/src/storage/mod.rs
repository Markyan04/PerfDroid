use pdcore::INVALID_METRIC_VALUE;
use pdcore::types::MetricBatch;

#[derive(Debug, Clone)]
pub struct TimestampedBatch {
    pub timestamp_ms: u64,
    pub batch: MetricBatch,
}

#[derive(Debug, Clone, Default)]
pub struct SessionStore {
    cpu_clock: Vec<TimestampedBatch>,
    cpu_usage: Vec<TimestampedBatch>,
    fps: Vec<TimestampedBatch>,
    battery_temperature: Vec<TimestampedBatch>,
    battery_voltage: Vec<TimestampedBatch>,
    battery_current: Vec<TimestampedBatch>,
    battery_power: Vec<TimestampedBatch>,
    timeline_compaction_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdcore::types::MetricBatch;

    #[test]
    fn stores_latest_battery_temperature_value() {
        let mut store = SessionStore::default();
        store.push(TimestampedBatch {
            timestamp_ms: 1234,
            batch: MetricBatch {
                metric_key: "BATTERY_TEMP".to_string(),
                unit: "0.1C".to_string(),
                values: vec![315],
            },
        });

        assert_eq!(store.latest_battery_temperature_value(), Some(315));
        assert_eq!(store.battery_temperature_frames().len(), 1);
    }

    #[test]
    fn stores_latest_power_related_values() {
        let mut store = SessionStore::default();
        store.push(TimestampedBatch {
            timestamp_ms: 1234,
            batch: MetricBatch {
                metric_key: "VOLTAGE".to_string(),
                unit: "mV".to_string(),
                values: vec![4321],
            },
        });
        store.push(TimestampedBatch {
            timestamp_ms: 1235,
            batch: MetricBatch {
                metric_key: "CURRENT".to_string(),
                unit: "mA".to_string(),
                values: vec![512],
            },
        });
        store.push(TimestampedBatch {
            timestamp_ms: 1236,
            batch: MetricBatch {
                metric_key: "POWER".to_string(),
                unit: "mW".to_string(),
                values: vec![2212],
            },
        });

        assert_eq!(store.latest_battery_voltage_value(), Some(4321));
        assert_eq!(store.latest_battery_current_value(), Some(512));
        assert_eq!(store.latest_battery_power_value(), Some(2212));
    }

    #[test]
    fn delete_range_compacts_timeline_for_all_metrics() {
        let mut store = SessionStore::default();
        for ts in [0_u64, 5_000, 10_000, 15_000, 20_000] {
            store.push(TimestampedBatch {
                timestamp_ms: ts,
                batch: MetricBatch {
                    metric_key: "CPU_CLOCK".to_string(),
                    unit: "MHz".to_string(),
                    values: vec![1000],
                },
            });
            store.push(TimestampedBatch {
                timestamp_ms: ts,
                batch: MetricBatch {
                    metric_key: "FPS".to_string(),
                    unit: "FPS".to_string(),
                    values: vec![60],
                },
            });
        }

        store.delete_range(5_000, 15_000);

        let cpu_ts: Vec<u64> = store
            .cpu_clock_frames()
            .iter()
            .map(|f| f.timestamp_ms)
            .collect();
        let fps_ts: Vec<u64> = store.fps_frames().iter().map(|f| f.timestamp_ms).collect();
        assert_eq!(cpu_ts, vec![0, 10_000]);
        assert_eq!(fps_ts, vec![0, 10_000]);
    }

    #[test]
    fn delete_range_also_compacts_future_incoming_frames() {
        let mut store = SessionStore::default();
        for ts in [0_u64, 5_000, 10_000, 15_000, 20_000] {
            store.push(TimestampedBatch {
                timestamp_ms: ts,
                batch: MetricBatch {
                    metric_key: "CPU_CLOCK".to_string(),
                    unit: "MHz".to_string(),
                    values: vec![1000],
                },
            });
        }

        store.delete_range(5_000, 15_000);

        store.push(TimestampedBatch {
            timestamp_ms: 25_000,
            batch: MetricBatch {
                metric_key: "CPU_CLOCK".to_string(),
                unit: "MHz".to_string(),
                values: vec![1100],
            },
        });

        let cpu_ts: Vec<u64> = store
            .cpu_clock_frames()
            .iter()
            .map(|f| f.timestamp_ms)
            .collect();
        assert_eq!(cpu_ts, vec![0, 10_000, 15_000]);
    }

    #[test]
    fn delete_single_point_compacts_by_sampling_step() {
        let mut store = SessionStore::default();
        for ts in [0_u64, 1_000, 2_000, 3_000] {
            store.push(TimestampedBatch {
                timestamp_ms: ts,
                batch: MetricBatch {
                    metric_key: "CPU_CLOCK".to_string(),
                    unit: "MHz".to_string(),
                    values: vec![1000],
                },
            });
        }

        store.delete_range(0, 0);
        let cpu_ts: Vec<u64> = store
            .cpu_clock_frames()
            .iter()
            .map(|f| f.timestamp_ms)
            .collect();
        assert_eq!(cpu_ts, vec![0, 1_000, 2_000]);
    }
}

impl SessionStore {
    pub fn push(&mut self, mut frame: TimestampedBatch) {
        frame.timestamp_ms = frame
            .timestamp_ms
            .saturating_sub(self.timeline_compaction_ms);
        match frame.batch.metric_key.as_str() {
            "CPU_CLOCK" => self.cpu_clock.push(frame),
            "CPU_USAGE" => self.cpu_usage.push(frame),
            "FPS" => self.fps.push(frame),
            "BATTERY_TEMP" => self.battery_temperature.push(frame),
            "VOLTAGE" => self.battery_voltage.push(frame),
            "CURRENT" => self.battery_current.push(frame),
            "POWER" => self.battery_power.push(frame),
            _ => {}
        }
    }

    pub fn cpu_clock_frames(&self) -> &[TimestampedBatch] {
        &self.cpu_clock
    }

    pub fn latest_cpu_clock(&self) -> Option<&TimestampedBatch> {
        self.cpu_clock.last()
    }

    pub fn cpu_usage_frames(&self) -> &[TimestampedBatch] {
        &self.cpu_usage
    }

    pub fn latest_cpu_usage(&self) -> Option<&TimestampedBatch> {
        self.cpu_usage.last()
    }

    pub fn fps_frames(&self) -> &[TimestampedBatch] {
        &self.fps
    }

    pub fn latest_fps(&self) -> Option<&TimestampedBatch> {
        self.fps.last()
    }

    pub fn battery_temperature_frames(&self) -> &[TimestampedBatch] {
        &self.battery_temperature
    }

    pub fn latest_battery_temperature(&self) -> Option<&TimestampedBatch> {
        self.battery_temperature.last()
    }

    pub fn battery_voltage_frames(&self) -> &[TimestampedBatch] {
        &self.battery_voltage
    }

    pub fn latest_battery_voltage(&self) -> Option<&TimestampedBatch> {
        self.battery_voltage.last()
    }

    pub fn battery_current_frames(&self) -> &[TimestampedBatch] {
        &self.battery_current
    }

    pub fn latest_battery_current(&self) -> Option<&TimestampedBatch> {
        self.battery_current.last()
    }

    pub fn battery_power_frames(&self) -> &[TimestampedBatch] {
        &self.battery_power
    }

    pub fn latest_battery_power(&self) -> Option<&TimestampedBatch> {
        self.battery_power.last()
    }

    pub fn global_start_timestamp_ms(&self) -> Option<u64> {
        [
            self.cpu_clock.first(),
            self.cpu_usage.first(),
            self.fps.first(),
            self.battery_temperature.first(),
            self.battery_voltage.first(),
            self.battery_current.first(),
            self.battery_power.first(),
        ]
        .into_iter()
        .flatten()
        .map(|f| f.timestamp_ms)
        .min()
    }

    pub fn delete_range(&mut self, start_ms: u64, end_ms: u64) {
        let shift_ms = if start_ms == end_ms {
            self.estimate_sampling_step_ms().unwrap_or(0)
        } else {
            end_ms.saturating_sub(start_ms)
        };
        delete_and_compact_frames(&mut self.cpu_clock, start_ms, end_ms, shift_ms);
        delete_and_compact_frames(&mut self.cpu_usage, start_ms, end_ms, shift_ms);
        delete_and_compact_frames(&mut self.fps, start_ms, end_ms, shift_ms);
        delete_and_compact_frames(&mut self.battery_temperature, start_ms, end_ms, shift_ms);
        delete_and_compact_frames(&mut self.battery_voltage, start_ms, end_ms, shift_ms);
        delete_and_compact_frames(&mut self.battery_current, start_ms, end_ms, shift_ms);
        delete_and_compact_frames(&mut self.battery_power, start_ms, end_ms, shift_ms);
        self.timeline_compaction_ms = self.timeline_compaction_ms.saturating_add(shift_ms);
    }

    pub fn latest_values(&self) -> Vec<Option<i64>> {
        self.latest_cpu_clock()
            .map(|frame| {
                frame
                    .batch
                    .values
                    .iter()
                    .map(|value| (*value != INVALID_METRIC_VALUE).then_some(*value))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn latest_fps_value(&self) -> Option<i64> {
        self.latest_fps().and_then(|frame| {
            frame
                .batch
                .values
                .first()
                .copied()
                .filter(|value| *value != INVALID_METRIC_VALUE)
        })
    }

    pub fn latest_cpu_usage_values(&self) -> Vec<Option<i64>> {
        self.latest_cpu_usage()
            .map(|frame| {
                frame
                    .batch
                    .values
                    .iter()
                    .map(|value| (*value != INVALID_METRIC_VALUE).then_some(*value))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn latest_battery_temperature_value(&self) -> Option<i64> {
        self.latest_scalar_value(self.latest_battery_temperature())
    }

    pub fn latest_battery_voltage_value(&self) -> Option<i64> {
        self.latest_scalar_value(self.latest_battery_voltage())
    }

    pub fn latest_battery_current_value(&self) -> Option<i64> {
        self.latest_scalar_value(self.latest_battery_current())
    }

    pub fn latest_battery_power_value(&self) -> Option<i64> {
        self.latest_scalar_value(self.latest_battery_power())
    }

    fn latest_scalar_value(&self, frame: Option<&TimestampedBatch>) -> Option<i64> {
        frame.and_then(|frame| {
            frame
                .batch
                .values
                .first()
                .copied()
                .filter(|value| *value != INVALID_METRIC_VALUE)
        })
    }

    fn estimate_sampling_step_ms(&self) -> Option<u64> {
        let mut timestamps = Vec::new();
        timestamps.extend(self.cpu_clock.iter().map(|f| f.timestamp_ms));
        timestamps.extend(self.cpu_usage.iter().map(|f| f.timestamp_ms));
        timestamps.extend(self.fps.iter().map(|f| f.timestamp_ms));
        timestamps.extend(self.battery_temperature.iter().map(|f| f.timestamp_ms));
        timestamps.extend(self.battery_voltage.iter().map(|f| f.timestamp_ms));
        timestamps.extend(self.battery_current.iter().map(|f| f.timestamp_ms));
        timestamps.extend(self.battery_power.iter().map(|f| f.timestamp_ms));
        timestamps.sort_unstable();
        timestamps.dedup();
        let mut min_delta: Option<u64> = None;
        for pair in timestamps.windows(2) {
            let delta = pair[1].saturating_sub(pair[0]);
            if delta == 0 {
                continue;
            }
            min_delta = Some(min_delta.map_or(delta, |curr| curr.min(delta)));
        }
        min_delta
    }
}

fn delete_and_compact_frames(
    frames: &mut Vec<TimestampedBatch>,
    start_ms: u64,
    end_ms: u64,
    shift_ms: u64,
) {
    frames.retain(|frame| frame.timestamp_ms < start_ms || frame.timestamp_ms > end_ms);
    if shift_ms == 0 {
        return;
    }
    for frame in frames.iter_mut() {
        if frame.timestamp_ms > end_ms {
            frame.timestamp_ms = frame.timestamp_ms.saturating_sub(shift_ms);
        }
    }
}
