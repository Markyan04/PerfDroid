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
    fps: Vec<TimestampedBatch>,
}

impl SessionStore {
    pub fn push(&mut self, frame: TimestampedBatch) {
        match frame.batch.metric_key.as_str() {
            "CPU_CLOCK" => self.cpu_clock.push(frame),
            "FPS" => self.fps.push(frame),
            _ => {}
        }
    }

    pub fn cpu_clock_frames(&self) -> &[TimestampedBatch] {
        &self.cpu_clock
    }

    pub fn latest_cpu_clock(&self) -> Option<&TimestampedBatch> {
        self.cpu_clock.last()
    }

    pub fn fps_frames(&self) -> &[TimestampedBatch] {
        &self.fps
    }

    pub fn latest_fps(&self) -> Option<&TimestampedBatch> {
        self.fps.last()
    }

    pub fn delete_range(&mut self, start_ms: u64, end_ms: u64) {
        self.cpu_clock
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
}
