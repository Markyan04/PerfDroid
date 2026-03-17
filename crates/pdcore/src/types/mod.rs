use std::str::FromStr;

use crate::errors::CoreError;
use crate::utils::{normalize_metric_values, validate_collectors, validate_non_empty};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectorMetadata {
    pub collector_key: String,
    pub unit: String,
    pub order: usize,
}

impl CollectorMetadata {
    pub fn new(
        collector_key: impl Into<String>,
        unit: impl Into<String>,
        order: usize,
    ) -> Result<Self, CoreError> {
        let collector_key = collector_key.into();
        let unit = unit.into();

        validate_non_empty("collector_key", &collector_key)?;
        validate_non_empty("unit", &unit)?;

        Ok(Self {
            collector_key,
            unit,
            order,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfilerMetadata {
    pub profiler_key: String,
    pub collector: Vec<CollectorMetadata>,
}

impl ProfilerMetadata {
    pub fn new(
        profiler_key: impl Into<String>,
        collector: Vec<CollectorMetadata>,
    ) -> Result<Self, CoreError> {
        let profiler_key = profiler_key.into();
        validate_non_empty("profiler_key", &profiler_key)?;
        validate_collectors(&collector)?;

        Ok(Self {
            profiler_key,
            collector,
        })
    }

    pub fn ordered_collectors(&self) -> Vec<&CollectorMetadata> {
        let mut collectors = self.collector.iter().collect::<Vec<_>>();
        collectors.sort_by_key(|collector| collector.order);
        collectors
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricBatch {
    pub metric_key: String,
    pub unit: String,
    pub values: Vec<i64>,
}

impl MetricBatch {
    pub fn new(
        metric_key: impl Into<String>,
        unit: impl Into<String>,
        values: Vec<i64>,
    ) -> Result<Self, CoreError> {
        let metric_key = metric_key.into();
        let unit = unit.into();

        validate_non_empty("metric_key", &metric_key)?;
        validate_non_empty("unit", &unit)?;

        Ok(Self {
            metric_key,
            unit,
            values: normalize_metric_values(values)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlCommand {
    Connect,
    Start,
    Pause,
    Restart,
    Stop,
}

impl ControlCommand {
    pub const ALL: [Self; 5] = [
        Self::Connect,
        Self::Start,
        Self::Pause,
        Self::Restart,
        Self::Stop,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::Start => "start",
            Self::Pause => "pause",
            Self::Restart => "restart",
            Self::Stop => "stop",
        }
    }
}

impl FromStr for ControlCommand {
    type Err = CoreError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "connect" => Ok(Self::Connect),
            "start" => Ok(Self::Start),
            "pause" => Ok(Self::Pause),
            "restart" => Ok(Self::Restart),
            "stop" => Ok(Self::Stop),
            other => Err(CoreError::InvalidControlCommand(other.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::constants::{INVALID_METRIC_VALUE, METRIC_VALUES_CAPACITY};

    use super::{CollectorMetadata, MetricBatch, ProfilerMetadata};

    #[test]
    fn metric_batch_is_padded_to_fixed_size() {
        let batch = MetricBatch::new("FPS", "FPS", vec![118]).expect("batch should be created");
        assert_eq!(batch.values.len(), METRIC_VALUES_CAPACITY);
        assert_eq!(batch.values[0], 118);
        assert_eq!(batch.values[1], INVALID_METRIC_VALUE);
    }

    #[test]
    fn collector_metadata_enforces_required_fields() {
        assert!(CollectorMetadata::new("", "Mhz", 0).is_err());
        assert!(CollectorMetadata::new("cpu0", "", 0).is_err());
    }

    #[test]
    fn profiler_metadata_rejects_duplicate_order() {
        let c1 = CollectorMetadata::new("cpu_l", "Mhz", 0).expect("collector 1");
        let c2 = CollectorMetadata::new("cpu_b", "Mhz", 0).expect("collector 2");

        assert!(ProfilerMetadata::new("CPU_CLOCK", vec![c1, c2]).is_err());
    }
}
