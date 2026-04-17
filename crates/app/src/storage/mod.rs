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
}

impl SessionStore {
    pub fn push(&mut self, frame: TimestampedBatch) {
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

    pub fn delete_range(&mut self, start_ms: u64, end_ms: u64) {
        self.cpu_clock
            .retain(|frame| frame.timestamp_ms < start_ms || frame.timestamp_ms > end_ms);
        self.cpu_usage
            .retain(|frame| frame.timestamp_ms < start_ms || frame.timestamp_ms > end_ms);
        self.fps
            .retain(|frame| frame.timestamp_ms < start_ms || frame.timestamp_ms > end_ms);
        self.battery_temperature
            .retain(|frame| frame.timestamp_ms < start_ms || frame.timestamp_ms > end_ms);
        self.battery_voltage
            .retain(|frame| frame.timestamp_ms < start_ms || frame.timestamp_ms > end_ms);
        self.battery_current
            .retain(|frame| frame.timestamp_ms < start_ms || frame.timestamp_ms > end_ms);
        self.battery_power
            .retain(|frame| frame.timestamp_ms < start_ms || frame.timestamp_ms > end_ms);
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
}
