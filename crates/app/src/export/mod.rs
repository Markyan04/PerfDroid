use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use crate::storage::{SessionStore, TimestampedBatch};

pub fn export_session_to_csv(
    path: &Path,
    session: &SessionStore,
    sampling_hz: u64,
) -> Result<usize, String> {
    ensure_parent_exists(path)?;

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

pub fn export_session_to_json(
    path: &Path,
    session: &SessionStore,
    sampling_hz: u64,
) -> Result<usize, String> {
    ensure_parent_exists(path)?;
    let rows = build_json_rows(session);
    let precision = time_precision_for_hz(sampling_hz);

    let mut out = String::new();
    out.push_str("{\n");
    out.push_str(&format!("  \"sampling_hz\": {sampling_hz},\n"));
    out.push_str(&format!("  \"time_precision\": {precision},\n"));
    out.push_str("  \"rows\": [\n");

    for (idx, row) in rows.iter().enumerate() {
        out.push_str("    {\n");
        out.push_str(&format!("      \"time_s\": {:.precision$},\n", row.elapsed_s));
        out.push_str(&format!(
            "      \"metric_key\": \"{}\",\n",
            json_escape(&row.metric_key)
        ));
        out.push_str(&format!("      \"unit\": \"{}\",\n", json_escape(&row.unit)));
        out.push_str("      \"values\": [");
        for (value_idx, value) in row.values.iter().enumerate() {
            if value_idx > 0 {
                out.push_str(", ");
            }
            out.push_str(&value.to_string());
        }
        out.push_str("]\n");
        out.push_str("    }");
        if idx + 1 < rows.len() {
            out.push(',');
        }
        out.push('\n');
    }

    out.push_str("  ]\n}");

    std::fs::write(path, out)
        .map_err(|err| format!("failed to write json file `{}`: {err}", path.display()))?;
    Ok(rows.len())
}

pub fn export_session_to_html(
    _path: &Path,
    _session: &SessionStore,
    _sampling_hz: u64,
) -> Result<usize, String> {
    Err("HTML export renderer has been removed for full refactor.".to_string())
}

pub fn export_session_to_png(_path: &Path, _session: &SessionStore) -> Result<usize, String> {
    Err("PNG export renderer has been removed for full refactor.".to_string())
}

#[derive(Debug, Clone)]
struct JsonRow {
    metric_key: String,
    unit: String,
    elapsed_s: f64,
    values: [i64; 10],
}

fn collect_frames(session: &SessionStore) -> Vec<&TimestampedBatch> {
    let mut frames = Vec::with_capacity(
        session.cpu_clock_frames().len()
            + session.cpu_usage_frames().len()
            + session.fps_frames().len()
            + session.battery_temperature_frames().len()
            + session.battery_voltage_frames().len()
            + session.battery_current_frames().len()
            + session.battery_power_frames().len(),
    );
    frames.extend(session.cpu_clock_frames().iter());
    frames.extend(session.cpu_usage_frames().iter());
    frames.extend(session.fps_frames().iter());
    frames.extend(session.battery_temperature_frames().iter());
    frames.extend(session.battery_voltage_frames().iter());
    frames.extend(session.battery_current_frames().iter());
    frames.extend(session.battery_power_frames().iter());
    frames
}

fn build_json_rows(session: &SessionStore) -> Vec<JsonRow> {
    let mut frames = collect_frames(session);
    frames.sort_by_key(|frame| frame.timestamp_ms);
    let base_timestamp_ms = frames.first().map(|frame| frame.timestamp_ms).unwrap_or(0);

    frames
        .into_iter()
        .map(|frame| {
            let mut values = [-1; 10];
            for (idx, value) in frame.batch.values.iter().copied().enumerate().take(10) {
                values[idx] = value;
            }

            JsonRow {
                metric_key: frame.batch.metric_key.clone(),
                unit: frame.batch.unit.clone(),
                elapsed_s: frame.timestamp_ms.saturating_sub(base_timestamp_ms) as f64 / 1000.0,
                values,
            }
        })
        .collect()
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

    if denominator > 1 {
        3
    } else {
        two_count.max(five_count).max(1)
    }
}

fn ensure_parent_exists(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            return Err(format!(
                "export directory does not exist: {}",
                parent.display()
            ));
        }
    }
    Ok(())
}

fn json_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
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

    #[test]
    fn export_generates_json_and_reports_html_png_removed() {
        let mut store = SessionStore::default();
        store.push(TimestampedBatch {
            timestamp_ms: 1000,
            batch: MetricBatch {
                metric_key: "FPS".to_string(),
                unit: "FPS".to_string(),
                values: vec![90],
            },
        });

        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let base = std::env::temp_dir().join(format!("perfdroid-export-{suffix}"));
        let json_path = base.with_extension("json");

        let json_rows = export_session_to_json(&json_path, &store, 5).expect("json export");
        assert_eq!(json_rows, 1);
        let json_text = std::fs::read_to_string(&json_path).expect("json text");
        assert!(json_text.contains("\"rows\": ["));

        let html_err = export_session_to_html(&base.with_extension("html"), &store, 5)
            .expect_err("html removed");
        assert!(html_err.contains("removed"));

        let png_err = export_session_to_png(&base.with_extension("png"), &store)
            .expect_err("png removed");
        assert!(png_err.contains("removed"));

        let _ = std::fs::remove_file(json_path);
    }
}
