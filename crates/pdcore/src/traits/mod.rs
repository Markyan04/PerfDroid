use std::sync::atomic::Ordering;

use crate::errors::CoreError;
use crate::types::{CollectorMetadata, ControlCommand, MetricBatch, ProfilerMetadata};

pub trait Collector: Send + Sync {
    fn metadata(&self) -> &CollectorMetadata;
    fn read_buffer(&self, ordering: Ordering) -> i64;
}

pub trait Profiler: Send + Sync {
    type CollectorType: Collector;

    fn metadata(&self) -> &ProfilerMetadata;
    fn collectors(&self) -> &[Self::CollectorType];
}

pub trait DataPlane {
    fn build_metric_batch(&self) -> Result<MetricBatch, CoreError>;
}

pub trait ControlPlane {
    fn apply_control(&mut self, command: ControlCommand) -> Result<(), CoreError>;
}
