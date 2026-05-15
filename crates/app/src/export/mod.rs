use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use crate::storage::{SessionStore, TimestampedBatch};

const LINE_COLORS: [&str; 10] = [
    "#2563EB", "#F97316", "#10B981", "#DB2777", "#7C3AED", "#0F766E", "#DC2626", "#CA8A04",
    "#4F46E5", "#0891B2",
];

#[derive(Debug, Clone)]
struct JsonRow {
    metric_key: String,
    unit: String,
    elapsed_s: f64,
    values: [i64; 10],
}

#[derive(Debug, Clone)]
struct PlotRow {
    time_s: f64,
    values: [f64; 10],
}

#[derive(Debug, Clone)]
struct SeriesStats {
    series: String,
    avg: f64,
    min: f64,
    max: f64,
    median: f64,
    p95: f64,
    p99: f64,
    stddev: f64,
    cv: f64,
    range: f64,
}

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
    path: &Path,
    session: &SessionStore,
    sampling_hz: u64,
) -> Result<usize, String> {
    ensure_parent_exists(path)?;
    let precision = time_precision_for_hz(sampling_hz);

    let cpu_clock_rows = build_metric_rows(session.cpu_clock_frames(), 1.0);
    let cpu_usage_rows = build_metric_rows(session.cpu_usage_frames(), 1.0);
    let temp_rows = build_metric_rows(session.battery_temperature_frames(), 10.0);
    let fps_rows = build_metric_rows(session.fps_frames(), 1.0);
    let battery_rows = build_battery_power_rows(session);

    let cpu_clock_lines = detect_line_count(session.latest_cpu_clock().map(|f| &f.batch.values));
    let cpu_usage_lines = detect_line_count(session.latest_cpu_usage().map(|f| &f.batch.values));

    let mut html = String::new();
    html.push_str("<!doctype html><html><head><meta charset=\"utf-8\"><title>PerfDroid Report</title>");
    html.push_str("<style>body{font-family:Arial,sans-serif;background:#F3EBDD;color:#222;padding:24px}h1{margin:0 0 6px}h2{margin:0 0 10px;text-align:center}.panel{background:#FFF9F1;border:1px solid #ddd;border-radius:12px;padding:16px;margin-bottom:16px}.cards{display:flex;flex-wrap:wrap;gap:8px;justify-content:center;margin-bottom:10px}.card{background:#F7EEE0;border:1px solid #ddd;border-radius:8px;padding:8px 12px;min-width:120px;text-align:center}.legend{display:flex;flex-wrap:wrap;gap:8px;margin-top:8px}.legend i{display:inline-block;width:14px;height:3px;vertical-align:middle;margin-right:6px}table{width:100%;border-collapse:collapse;margin-top:10px;background:#fff}th,td{border:1px solid #ddd;padding:6px 8px;text-align:left;font-size:12px}.muted{color:#666}</style>");
    html.push_str("</head><body>");
    html.push_str("<h1>PerfDroid</h1>");
    html.push_str(&format!("<p class=\"muted\">sampling_hz: {} Hz | time_precision: {} decimal places (time label precision) | state: exported</p>", sampling_hz, precision));

    html.push_str(&render_panel(
        "CPU Clock",
        "Metric: CPU_CLOCK | unit: MHz | fixed width values: 10",
        &cpu_clock_rows,
        (0..cpu_clock_lines)
            .map(|i| (format!("policy{i}"), LINE_COLORS[i % LINE_COLORS.len()]))
            .collect(),
        "MHz",
        &build_latest_cards(session.latest_values(), "MHz", "policy"),
        &(0..cpu_clock_lines).collect::<Vec<_>>(),
    ));

    html.push_str(&render_panel(
        "CPU Usage",
        "Metric: CPU_USAGE | unit: % | fixed width values: 10",
        &cpu_usage_rows,
        (0..cpu_usage_lines)
            .map(|i| (format!("policy{i}"), LINE_COLORS[i % LINE_COLORS.len()]))
            .collect(),
        "%",
        &build_latest_cards(session.latest_cpu_usage_values(), "%", "policy"),
        &(0..cpu_usage_lines).collect::<Vec<_>>(),
    ));

    html.push_str(&render_panel(
        "Battery Temperature",
        "Metric: BATTERY_TEMP | unit: 0.1C | collector: battery",
        &temp_rows,
        vec![("battery".to_string(), "#D97706")],
        "C",
        &vec![(
            "Battery".to_string(),
            session
                .latest_battery_temperature_value()
                .map(|v| format!("{:.1} C", v as f64 / 10.0))
                .unwrap_or_else(|| "--".to_string()),
        )],
        &[0],
    ));

    html.push_str(&render_panel(
        "Battery Power Metrics",
        "Metrics: VOLTAGE / CURRENT / POWER | units: mV, mA, mW | collector: battery",
        &battery_rows,
        vec![
            ("voltage (mV)".to_string(), "#2563EB"),
            ("current (mA)".to_string(), "#10B981"),
            ("power (mW)".to_string(), "#DB2777"),
        ],
        "",
        &vec![
            (
                "Voltage".to_string(),
                session
                    .latest_battery_voltage_value()
                    .map(|v| format!("{v} mV"))
                    .unwrap_or_else(|| "--".to_string()),
            ),
            (
                "Current".to_string(),
                session
                    .latest_battery_current_value()
                    .map(|v| format!("{v} mA"))
                    .unwrap_or_else(|| "--".to_string()),
            ),
            (
                "Power".to_string(),
                session
                    .latest_battery_power_value()
                    .map(|v| format!("{v} mW"))
                    .unwrap_or_else(|| "--".to_string()),
            ),
        ],
        &[0, 1, 2],
    ));

    html.push_str(&render_panel(
        "FPS",
        "Metric: FPS | unit: FPS | collector: main",
        &fps_rows,
        vec![("main".to_string(), "#DC2626")],
        "FPS",
        &vec![(
            "FPS".to_string(),
            session
                .latest_fps_value()
                .map(|v| format!("{v} FPS"))
                .unwrap_or_else(|| "--".to_string()),
        )],
        &[0],
    ));

    html.push_str("</body></html>");
    std::fs::write(path, html)
        .map_err(|err| format!("failed to write html file `{}`: {err}", path.display()))?;

    Ok(collect_frames(session).len())
}

pub fn export_session_to_png(_path: &Path, _session: &SessionStore) -> Result<usize, String> {
    Err("PNG export renderer has been removed for full refactor.".to_string())
}

fn render_panel(
    title: &str,
    footer: &str,
    rows: &[PlotRow],
    legends: Vec<(String, &'static str)>,
    unit: &str,
    cards: &[(String, String)],
    series_indexes: &[usize],
) -> String {
    let svg = render_line_chart_svg(rows, &legends, series_indexes, 1080.0, 240.0, unit);
    let stats = build_series_stats(rows, &legends, series_indexes);

    let mut s = String::new();
    s.push_str("<section class=\"panel\">");
    s.push_str(&format!("<h2>{}</h2>", html_escape(title)));
    s.push_str("<div class=\"cards\">");
    for (k, v) in cards {
        s.push_str(&format!(
            "<div class=\"card\"><div>{}</div><div>{}</div></div>",
            html_escape(k),
            html_escape(v)
        ));
    }
    s.push_str("</div>");
    s.push_str(&svg);
    s.push_str("<div class=\"legend\">");
    for (label, color) in &legends {
        s.push_str(&format!(
            "<span><i style=\"background:{}\"></i>{}</span>",
            color,
            html_escape(label)
        ));
    }
    s.push_str("</div>");
    s.push_str(&format!(
        "<div class=\"muted\" style=\"margin-top:8px\">{}</div>",
        html_escape(footer)
    ));
    s.push_str("<table><thead><tr><th>series</th><th>avg</th><th>min</th><th>max</th><th>median</th><th>p95</th><th>p99</th><th>stddev</th><th>cv</th><th>range</th></tr></thead><tbody>");
    for stat in stats {
        s.push_str(&format!(
            "<tr><td>{}</td><td>{:.2}</td><td>{:.2}</td><td>{:.2}</td><td>{:.2}</td><td>{:.2}</td><td>{:.2}</td><td>{:.2}</td><td>{:.2}%</td><td>{:.2}</td></tr>",
            html_escape(&stat.series), stat.avg, stat.min, stat.max, stat.median, stat.p95, stat.p99, stat.stddev, stat.cv * 100.0, stat.range
        ));
    }
    s.push_str("</tbody></table></section>");
    s
}

fn render_line_chart_svg(
    rows: &[PlotRow],
    legends: &[(String, &'static str)],
    series_indexes: &[usize],
    width: f64,
    height: f64,
    unit: &str,
) -> String {
    let left = 56.0;
    let right = width - 16.0;
    let top = 16.0;
    let bottom = height - 26.0;
    let max_time = rows.last().map(|r| r.time_s).unwrap_or(1.0).max(1.0);
    let mut max_value: f64 = 0.0;
    for row in rows {
        for &idx in series_indexes {
            max_value = max_value.max(row.values[idx]);
        }
    }
    max_value = max_value.max(1.0);

    let mut s = format!(
        "<svg width=\"100%\" height=\"{}\" viewBox=\"0 0 {} {}\" xmlns=\"http://www.w3.org/2000/svg\"><rect x=\"0\" y=\"0\" width=\"{}\" height=\"{}\" fill=\"#fff\"/><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"none\" stroke=\"#ccc\"/><line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"#888\"/><line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"#888\"/>",
        height,
        width,
        height,
        width,
        height,
        left,
        top,
        right - left,
        bottom - top,
        left,
        bottom,
        right,
        bottom,
        left,
        top,
        left,
        bottom
    );

    s.push_str(&format!(
        "<text x=\"8\" y=\"{}\" font-size=\"11\" fill=\"#666\">{:.1} {}</text><text x=\"8\" y=\"{}\" font-size=\"11\" fill=\"#666\">{:.1} {}</text><text x=\"8\" y=\"{}\" font-size=\"11\" fill=\"#666\">0 {}</text>",
        top + 10.0,
        max_value,
        html_escape(unit),
        (top + bottom) / 2.0,
        max_value / 2.0,
        html_escape(unit),
        bottom,
        html_escape(unit)
    ));
    s.push_str(&format!(
        "<text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"#666\">0.0s</text><text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"#666\">{:.1}s</text>",
        left,
        bottom + 14.0,
        right - 32.0,
        bottom + 14.0,
        max_time
    ));

    for (series_idx, (_, color)) in legends.iter().enumerate() {
        if series_idx >= series_indexes.len() {
            break;
        }
        let value_index = series_indexes[series_idx];
        let mut points = String::new();
        for row in rows {
            let x = left + (row.time_s / max_time) * (right - left);
            let y = bottom - (row.values[value_index] / max_value) * (bottom - top);
            if !points.is_empty() {
                points.push(' ');
            }
            points.push_str(&format!("{x:.2},{y:.2}"));
        }
        if !points.is_empty() {
            s.push_str(&format!(
                "<polyline points=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"2\"/>",
                points, color
            ));
        }
    }

    s.push_str("</svg>");
    s
}

fn build_series_stats(
    rows: &[PlotRow],
    legends: &[(String, &'static str)],
    series_indexes: &[usize],
) -> Vec<SeriesStats> {
    let mut out = Vec::new();
    for (series_idx, (label, _)) in legends.iter().enumerate() {
        if series_idx >= series_indexes.len() {
            break;
        }
        let idx = series_indexes[series_idx];
        let values: Vec<f64> = rows
            .iter()
            .map(|row| row.values[idx])
            .filter(|v| *v > 0.0)
            .collect();
        if values.is_empty() {
            continue;
        }
        let n = values.len();
        let sum: f64 = values.iter().sum();
        let avg = sum / n as f64;
        let min = values.iter().copied().fold(f64::INFINITY, f64::min);
        let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let mut sorted = values.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = percentile(&sorted, 0.5);
        let p95 = percentile(&sorted, 0.95);
        let p99 = percentile(&sorted, 0.99);
        let variance = values
            .iter()
            .map(|v| {
                let d = *v - avg;
                d * d
            })
            .sum::<f64>()
            / n as f64;
        let stddev = variance.sqrt();
        let cv = if avg.abs() < f64::EPSILON {
            0.0
        } else {
            stddev / avg.abs()
        };
        let range = max - min;
        out.push(SeriesStats {
            series: label.clone(),
            avg,
            min,
            max,
            median,
            p95,
            p99,
            stddev,
            cv,
            range,
        });
    }
    out
}

fn percentile(sorted_values: &[f64], p: f64) -> f64 {
    if sorted_values.is_empty() {
        return 0.0;
    }
    let rank = (sorted_values.len() - 1) as f64 * p.clamp(0.0, 1.0);
    let low = rank.floor() as usize;
    let high = rank.ceil() as usize;
    if low == high {
        sorted_values[low]
    } else {
        let w = rank - low as f64;
        sorted_values[low] * (1.0 - w) + sorted_values[high] * w
    }
}

fn detect_line_count(values: Option<&Vec<i64>>) -> usize {
    values
        .map(|vals| vals.iter().take_while(|v| **v >= 0).count().max(1))
        .unwrap_or(1)
}

fn build_metric_rows(frames: &[TimestampedBatch], scale: f64) -> Vec<PlotRow> {
    let Some(first) = frames.first() else {
        return Vec::new();
    };
    frames
        .iter()
        .map(|frame| {
            let time_s = (frame.timestamp_ms.saturating_sub(first.timestamp_ms)) as f64 / 1000.0;
            let mut values = [0.0; 10];
            for (i, value) in frame.batch.values.iter().copied().enumerate().take(10) {
                if value >= 0 {
                    values[i] = value as f64 / scale.max(1.0);
                }
            }
            PlotRow { time_s, values }
        })
        .collect()
}

fn build_battery_power_rows(session: &SessionStore) -> Vec<PlotRow> {
    let voltage_frames = session.battery_voltage_frames();
    let current_frames = session.battery_current_frames();
    let power_frames = session.battery_power_frames();
    let total_rows = voltage_frames
        .len()
        .max(current_frames.len())
        .max(power_frames.len());
    if total_rows == 0 {
        return Vec::new();
    }
    let first_timestamp_ms = voltage_frames
        .first()
        .or_else(|| current_frames.first())
        .or_else(|| power_frames.first())
        .map(|f| f.timestamp_ms)
        .unwrap_or(0);

    (0..total_rows)
        .map(|idx| {
            let timestamp_ms = voltage_frames
                .get(idx)
                .or_else(|| current_frames.get(idx))
                .or_else(|| power_frames.get(idx))
                .map(|f| f.timestamp_ms)
                .unwrap_or(first_timestamp_ms);
            let mut values = [0.0; 10];
            values[0] = frame_scalar_value(voltage_frames.get(idx));
            values[1] = frame_scalar_value(current_frames.get(idx));
            values[2] = frame_scalar_value(power_frames.get(idx));
            PlotRow {
                time_s: (timestamp_ms.saturating_sub(first_timestamp_ms)) as f64 / 1000.0,
                values,
            }
        })
        .collect()
}

fn frame_scalar_value(frame: Option<&TimestampedBatch>) -> f64 {
    frame
        .and_then(|f| f.batch.values.first().copied())
        .filter(|v| *v >= 0)
        .map(|v| v as f64)
        .unwrap_or(0.0)
}

fn build_latest_cards(
    latest_values: Vec<Option<i64>>,
    unit: &str,
    prefix: &str,
) -> Vec<(String, String)> {
    let mut cards = Vec::new();
    for (idx, value) in latest_values.into_iter().enumerate() {
        if let Some(v) = value {
            cards.push((format!("{prefix}{idx}"), format!("{v} {unit}")));
        }
    }
    if cards.is_empty() {
        cards.push(("--".to_string(), "--".to_string()));
    }
    cards
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

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
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
    fn export_generates_json_and_html() {
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
        let html_path = base.with_extension("html");

        let json_rows = export_session_to_json(&json_path, &store, 5).expect("json export");
        assert_eq!(json_rows, 1);
        let json_text = std::fs::read_to_string(&json_path).expect("json text");
        assert!(json_text.contains("\"rows\": ["));

        let html_rows = export_session_to_html(&html_path, &store, 5).expect("html export");
        assert_eq!(html_rows, 1);
        let html_text = std::fs::read_to_string(&html_path).expect("html text");
        assert!(html_text.contains("CPU Clock"));
        assert!(html_text.contains("<th>median</th>"));

        let _ = std::fs::remove_file(json_path);
        let _ = std::fs::remove_file(html_path);
    }
}
