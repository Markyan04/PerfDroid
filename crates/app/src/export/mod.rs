use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use crate::storage::{SessionStore, TimestampedBatch};

pub fn export_session_to_csv(
    path: &Path,
    session: &SessionStore,
    sampling_hz: u64,
) -> Result<usize, String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            return Err(format!(
                "export directory does not exist: {}",
                parent.display()
            ));
        }
    }

    let file = File::create(path)
        .map_err(|err| format!("failed to create csv file `{}`: {err}", path.display()))?;
    let mut writer = BufWriter::new(file);
    let precision = time_precision_for_hz(sampling_hz);
    let header = format!(
        "time_s(dp={precision},hz={sampling_hz}),metric_key,unit,value_0,value_1,value_2,value_3,value_4,value_5,value_6,value_7,value_8,value_9\n"
    );
    writer
        .write_all(header.as_bytes())
        .map_err(|err| format!("failed to write csv header `{}`: {err}", path.display()))?;

    let mut frames = collect_frames(session);
    frames.sort_by_key(|frame| frame.timestamp_ms);
    let base_timestamp_ms = frames.first().map(|frame| frame.timestamp_ms).unwrap_or(0);

    for frame in &frames {
        write_csv_row(&mut writer, frame, base_timestamp_ms, precision)
            .map_err(|err| format!("failed to write csv row `{}`: {err}", path.display()))?;
    }

    writer
        .flush()
        .map_err(|err| format!("failed to flush csv file `{}`: {err}", path.display()))?;
    Ok(frames.len())
}

fn collect_frames(session: &SessionStore) -> Vec<&TimestampedBatch> {
    let mut frames = Vec::with_capacity(
        session.cpu_clock_frames().len() + session.cpu_usage_frames().len() + session.fps_frames().len(),
    );
    frames.extend(session.cpu_clock_frames().iter());
    frames.extend(session.cpu_usage_frames().iter());
    frames.extend(session.fps_frames().iter());
    frames
}

fn write_csv_row(
    writer: &mut impl Write,
    frame: &TimestampedBatch,
    base_timestamp_ms: u64,
    precision: usize,
) -> std::io::Result<()> {
    let elapsed_ms = frame.timestamp_ms.saturating_sub(base_timestamp_ms);
    let elapsed_seconds = elapsed_ms as f64 / 1000.0;
    let time_label = format!("{elapsed_seconds:.precision$}s");
    writer.write_all(time_label.as_bytes())?;
    writer.write_all(b",")?;
    write_csv_field(writer, &frame.batch.metric_key)?;
    writer.write_all(b",")?;
    write_csv_field(writer, &frame.batch.unit)?;

    for idx in 0..10 {
        writer.write_all(b",")?;
        let value = frame.batch.values.get(idx).copied().unwrap_or(-1);
        writer.write_all(value.to_string().as_bytes())?;
    }

    writer.write_all(b"\n")
}

fn write_csv_field(writer: &mut impl Write, field: &str) -> std::io::Result<()> {
    let escaped = field.replace('"', "\"\"");
    writer.write_all(b"\"")?;
    writer.write_all(escaped.as_bytes())?;
    writer.write_all(b"\"")
}

fn time_precision_for_hz(sampling_hz: u64) -> usize {
    let hz = sampling_hz.clamp(1, 10);
    let mut denominator = hz;
    let mut two_count = 0usize;
    let mut five_count = 0usize;
    while denominator % 2 == 0 {
        denominator /= 2;
        two_count += 1;
    }
    while denominator % 5 == 0 {
        denominator /= 5;
        five_count += 1;
    }

    // If denominator has factors other than 2/5 (e.g. 3, 7, 9), decimal is recurring.
    // Keep three decimals for readability and consistency with 1-10Hz range.
    if denominator > 1 {
        3
    } else {
        two_count.max(five_count).max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdcore::types::MetricBatch;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn export_generates_csv_with_header_and_rows() {
        let mut store = SessionStore::default();
        store.push(TimestampedBatch {
            timestamp_ms: 2000,
            batch: MetricBatch {
                metric_key: "FPS".to_string(),
                unit: "FPS".to_string(),
                values: vec![119],
            },
        });
        store.push(TimestampedBatch {
            timestamp_ms: 1000,
            batch: MetricBatch {
                metric_key: "CPU_CLOCK".to_string(),
                unit: "MHz".to_string(),
                values: vec![1400, 1700],
            },
        });

        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("perfdroid-export-{suffix}.csv"));
        let rows = export_session_to_csv(&path, &store, 4).expect("export should succeed");
        assert_eq!(rows, 2);

        let text = std::fs::read_to_string(&path).expect("csv should exist");
        assert!(text.starts_with("time_s(dp=2,hz=4),metric_key,unit,value_0"));
        assert!(text.contains("0.00s,\"CPU_CLOCK\",\"MHz\",1400,1700"));
        assert!(text.contains("1.00s,\"FPS\",\"FPS\",119,-1,-1"));

        let _ = std::fs::remove_file(path);
    }
}
