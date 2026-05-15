use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};

use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, Application, Bounds, Context, Entity, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Render, SharedString,
    Styled, Subscription, Window, WindowBounds, WindowOptions, canvas, div, hsla, px, rgb, size,
    transparent_black,
};
use gpui_component::Disableable;
use gpui_component::Root;
use gpui_component::Sizable;
use gpui_component::Size as ComponentSize;
use gpui_component::StyledExt;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::chart::{AreaChart, LineChart};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::ScrollableElement;
use gpui_component::spinner::Spinner;
use pdcore::types::ControlCommand;

use crate::aggregation::AggregatorEvent;
use crate::device::{AdbDetectedDevice, DeviceDescriptor};
use crate::export::{
    export_session_to_csv, export_session_to_html, export_session_to_json, export_session_to_png,
};
use crate::runtime::PerfDroidRuntime;
use crate::session::SessionState;
use crate::storage::{SessionStore, TimestampedBatch};

const WINDOW_WIDTH: f32 = 1440.0;
const WINDOW_HEIGHT: f32 = 960.0;
const CHART_HEIGHT: f32 = 250.0;
const Y_AXIS_WIDTH: f32 = 72.0;
const APP_PADDING_X: f32 = 48.0;
const CHART_SECTION_PADDING_X: f32 = 40.0;
const CHART_PLOT_INNER_PADDING: f32 = 0.0;
const LINE_COLORS: [u32; 10] = [
    0x2563EB, 0xF97316, 0x10B981, 0xDB2777, 0x7C3AED, 0x0F766E, 0xDC2626, 0xCA8A04, 0x4F46E5,
    0x0891B2,
];

#[derive(Clone)]
struct PlotRow {
    timestamp_ms: u64,
    time_label: SharedString,
    values: [f64; 10],
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ChartKey {
    CpuClock,
    CpuUsage,
    BatteryTemp,
    BatteryPowerMetrics,
    Fps,
}

#[derive(Clone, Copy)]
enum SelectionRange {
    Point(u64),
    Span { start_ms: u64, end_ms: u64 },
}

#[derive(Clone, Copy)]
struct DragSelection {
    start_ms: u64,
    current_ms: u64,
}

pub fn run_demo() {
    let (tx, rx) = mpsc::channel();
    let runtime = Arc::new(PerfDroidRuntime::new(tx));

    Application::new().run(move |cx: &mut App| {
        gpui_component::init(cx);

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)),
                cx,
            ))),
            ..Default::default()
        };

        let runtime = Arc::clone(&runtime);
        cx.open_window(options, move |window, cx| {
            let runtime = Arc::clone(&runtime);
            let view = cx.new(|cx| PerfDroidDemo::new(runtime, rx, window, cx));
            cx.new(|cx| Root::new(view, window, cx))
        })
        .expect("open window failed");
    });
}

struct PerfDroidDemo {
    runtime: Arc<PerfDroidRuntime>,
    rx: Receiver<AggregatorEvent>,
    session: SessionStore,
    state: SessionState,
    device: Option<DeviceDescriptor>,
    detected_devices: Vec<AdbDetectedDevice>,
    status_line: String,
    is_busy: bool,
    busy_message: String,
    selected_hz: u64,
    package_name: String,
    package_input: Entity<InputState>,
    hz_input: Entity<InputState>,
    _package_input_subscription: Subscription,
    _hz_input_subscription: Subscription,
    selection: Option<SelectionRange>,
    drag_selection: Option<DragSelection>,
    ignore_next_left_click: bool,
}

impl PerfDroidDemo {
    fn new(
        runtime: Arc<PerfDroidRuntime>,
        rx: Receiver<AggregatorEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let initial_package_name = runtime.package_name();
        let package_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("com.example.game")
                .default_value(initial_package_name.clone())
        });
        let package_runtime = Arc::clone(&runtime);
        let package_input_subscription = cx.subscribe(
            &package_input,
            move |_, input, event: &InputEvent, cx| match event {
                InputEvent::Change => {
                    let value = input.read(cx).value().to_string();
                    let _ = package_runtime.request_set_package_name(value);
                }
                _ => {}
            },
        );
        let hz_input = cx.new(|cx| InputState::new(window, cx).default_value("4"));
        let hz_runtime = Arc::clone(&runtime);
        let hz_input_subscription = cx.subscribe(
            &hz_input,
            move |_, input, event: &InputEvent, cx| match event {
                InputEvent::Change => {
                    let raw = input.read(cx).value().to_string();
                    if let Ok(hz) = raw.trim().parse::<u64>() {
                        let _ = hz_runtime.request_set_hz(hz);
                    }
                }
                _ => {}
            },
        );

        Self {
            runtime,
            rx,
            session: SessionStore::default(),
            state: SessionState::Disconnected,
            device: None,
            detected_devices: Vec::new(),
            status_line: "Waiting for Connect.".to_string(),
            is_busy: false,
            busy_message: String::new(),
            selected_hz: 4,
            package_name: initial_package_name,
            package_input,
            hz_input,
            _package_input_subscription: package_input_subscription,
            _hz_input_subscription: hz_input_subscription,
            selection: None,
            drag_selection: None,
            ignore_next_left_click: false,
        }
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                AggregatorEvent::BusyStateChanged(is_busy, message) => {
                    self.is_busy = is_busy;
                    self.busy_message = message;
                }
                AggregatorEvent::StateChanged(state) => {
                    self.state = state;
                    if state == SessionState::Connected {
                        self.session = SessionStore::default();
                        self.selection = None;
                        self.drag_selection = None;
                        self.ignore_next_left_click = false;
                    }
                    self.status_line = format!("Session state => {}", state.as_str());
                }
                AggregatorEvent::DeviceUpdated(device) => {
                    self.device = Some(device);
                }
                AggregatorEvent::DeviceDiscoveryUpdated(devices) => {
                    self.detected_devices = devices;
                }
                AggregatorEvent::MetadataRegistered(_) => {}
                AggregatorEvent::MetricBatch(batch) => {
                    self.session.push(batch);
                }
                AggregatorEvent::SamplingRateChanged(hz) => {
                    self.selected_hz = hz;
                }
                AggregatorEvent::PackageNameChanged(package_name) => {
                    self.package_name = package_name;
                }
                AggregatorEvent::Status(message) => {
                    self.status_line = message;
                }
            }
        }
    }

    fn build_plot_rows(&self) -> Vec<PlotRow> {
        let Some(base_ts) = self.session.global_start_timestamp_ms() else {
            return Vec::new();
        };

        self.session
            .cpu_clock_frames()
            .iter()
            .map(|frame| {
                let elapsed_s = (frame.timestamp_ms.saturating_sub(base_ts)) as f64 / 1000.0;
                let mut values = [0.0; 10];
                for (idx, value) in frame.batch.values.iter().copied().enumerate().take(10) {
                    values[idx] = if value < 0 { 0.0 } else { value as f64 };
                }

                PlotRow {
                    timestamp_ms: frame.timestamp_ms,
                    time_label: format!("{elapsed_s:.3}s").into(),
                    values,
                }
            })
            .collect()
    }

    fn build_fps_rows(&self) -> Vec<PlotRow> {
        let Some(base_ts) = self.session.global_start_timestamp_ms() else {
            return Vec::new();
        };

        self.session
            .fps_frames()
            .iter()
            .map(|frame| {
                let elapsed_s = (frame.timestamp_ms.saturating_sub(base_ts)) as f64 / 1000.0;
                let mut values = [0.0; 10];
                if let Some(value) = frame.batch.values.first().copied() {
                    values[0] = if value < 0 { 0.0 } else { value as f64 };
                }

                PlotRow {
                    timestamp_ms: frame.timestamp_ms,
                    time_label: format!("{elapsed_s:.3}s").into(),
                    values,
                }
            })
            .collect()
    }

    fn build_battery_temperature_rows(&self) -> Vec<PlotRow> {
        self.build_single_metric_rows(self.session.battery_temperature_frames(), 10.0)
    }

    fn build_single_metric_rows(&self, frames: &[TimestampedBatch], scale: f64) -> Vec<PlotRow> {
        let Some(base_ts) = self.session.global_start_timestamp_ms() else {
            return Vec::new();
        };

        frames
            .iter()
            .map(|frame| {
                let elapsed_s = (frame.timestamp_ms.saturating_sub(base_ts)) as f64 / 1000.0;
                let mut values = [0.0; 10];
                if let Some(value) = frame.batch.values.first().copied() {
                    values[0] = if value < 0 {
                        0.0
                    } else {
                        value as f64 / scale.max(1.0)
                    };
                }

                PlotRow {
                    timestamp_ms: frame.timestamp_ms,
                    time_label: format!("{elapsed_s:.3}s").into(),
                    values,
                }
            })
            .collect()
    }

    fn build_battery_power_metric_rows(&self) -> Vec<PlotRow> {
        let voltage_frames = self.session.battery_voltage_frames();
        let current_frames = self.session.battery_current_frames();
        let power_frames = self.session.battery_power_frames();
        let total_rows = voltage_frames
            .len()
            .max(current_frames.len())
            .max(power_frames.len());

        if total_rows == 0 {
            return Vec::new();
        }

        let first_timestamp_ms = self.session.global_start_timestamp_ms().unwrap_or(0);

        (0..total_rows)
            .map(|idx| {
                let timestamp_ms = voltage_frames
                    .get(idx)
                    .or_else(|| current_frames.get(idx))
                    .or_else(|| power_frames.get(idx))
                    .map(|frame| frame.timestamp_ms)
                    .unwrap_or(first_timestamp_ms);
                let elapsed_s = (timestamp_ms.saturating_sub(first_timestamp_ms)) as f64 / 1000.0;
                let mut values = [0.0; 10];
                values[0] = voltage_frames
                    .get(idx)
                    .and_then(frame_scalar_value)
                    .unwrap_or(0.0);
                values[1] = current_frames
                    .get(idx)
                    .and_then(frame_scalar_value)
                    .unwrap_or(0.0);
                values[2] = power_frames
                    .get(idx)
                    .and_then(frame_scalar_value)
                    .unwrap_or(0.0);

                PlotRow {
                    timestamp_ms,
                    time_label: format!("{elapsed_s:.3}s").into(),
                    values,
                }
            })
            .collect()
    }

    fn selection_bounds_ms(&self) -> Option<(u64, u64)> {
        match self.selection {
            Some(SelectionRange::Point(ts)) => Some((ts, ts)),
            Some(SelectionRange::Span { start_ms, end_ms }) => Some((start_ms, end_ms)),
            None => None,
        }
    }

    fn nearest_timestamp_for_rows(rows: &[PlotRow], x_ratio: f32) -> Option<u64> {
        if rows.is_empty() {
            return None;
        }
        let ratio = x_ratio.clamp(0.0, 1.0);
        let idx = ((rows.len().saturating_sub(1) as f32) * ratio).round() as usize;
        rows.get(idx).map(|row| row.timestamp_ms)
    }

    fn nearest_x_ratio_for_selected(&self, rows: &[PlotRow]) -> Option<f32> {
        let selected_ts = match self.selection {
            Some(SelectionRange::Point(ts)) => ts,
            _ => return None,
        };
        let (idx, _) = rows
            .iter()
            .enumerate()
            .min_by_key(|(_, row)| row.timestamp_ms.abs_diff(selected_ts))?;
        let denom = rows.len().saturating_sub(1).max(1) as f32;
        Some((idx as f32 / denom).clamp(0.0, 1.0))
    }

    fn render_selection_summary(&self, view: Entity<Self>, chart_width: f32) -> impl IntoElement {
        let base_ts = self.session.global_start_timestamp_ms().unwrap_or(0);
        let text = if let Some((start_ms, end_ms)) = self.selection_bounds_ms() {
            format!(
                "Selected range: [{:.3}s, {:.3}s] (inclusive). Long-press left mouse to select range, then delete in Paused state.",
                start_ms.saturating_sub(base_ts) as f64 / 1000.0,
                end_ms.saturating_sub(base_ts) as f64 / 1000.0
            )
        } else {
            "No selection. Left click selects nearest point. Left long-press drag selects a range. Deletion is only allowed in Paused state."
                .to_string()
        };

        let can_delete = self.selection.is_some()
            && matches!(self.state, SessionState::Paused | SessionState::Stopped);
        let delete_view = view.clone();
        let delete_button = Button::new("delete-selection-top")
            .label("Delete Selection")
            .on_click(move |_, _, cx| {
                let _ = delete_view.update(cx, |this, cx| {
                    if this.delete_current_selection() {
                        cx.notify();
                    } else {
                        this.status_line = "No active selection to delete.".to_string();
                        cx.notify();
                    }
                });
            })
            .disabled(!can_delete);

        div()
            .w(px(chart_width))
            .p_3()
            .rounded_md()
            .border_1()
            .bg(rgb(0xE8F3FF))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_3()
            .child(div().flex_1().child(text))
            .child(div().w(px(180.0)).child(delete_button))
    }

    fn render_selection_details(&self, chart_width: f32) -> impl IntoElement {
        let base_ts = self.session.global_start_timestamp_ms().unwrap_or(0);
        let mut lines = Vec::new();
        match self.selection {
            Some(SelectionRange::Point(ts)) => {
                lines.push(format!(
                    "Point @ {:.3}s",
                    ts.saturating_sub(base_ts) as f64 / 1000.0
                ));
                lines.extend(self.describe_point(
                    "CPU_CLOCK",
                    self.session.cpu_clock_frames(),
                    ts,
                    1.0,
                ));
                lines.extend(self.describe_point(
                    "CPU_USAGE",
                    self.session.cpu_usage_frames(),
                    ts,
                    1.0,
                ));
                lines.extend(self.describe_point("FPS", self.session.fps_frames(), ts, 1.0));
                lines.extend(self.describe_point(
                    "BATTERY_TEMP",
                    self.session.battery_temperature_frames(),
                    ts,
                    10.0,
                ));
                lines.extend(self.describe_point(
                    "VOLTAGE",
                    self.session.battery_voltage_frames(),
                    ts,
                    1.0,
                ));
                lines.extend(self.describe_point(
                    "CURRENT",
                    self.session.battery_current_frames(),
                    ts,
                    1.0,
                ));
                lines.extend(self.describe_point(
                    "POWER",
                    self.session.battery_power_frames(),
                    ts,
                    1.0,
                ));
            }
            Some(SelectionRange::Span { start_ms, end_ms }) => {
                lines.push(format!(
                    "Range [{:.3}s, {:.3}s] avg/min/max",
                    start_ms.saturating_sub(base_ts) as f64 / 1000.0,
                    end_ms.saturating_sub(base_ts) as f64 / 1000.0
                ));
                lines.extend(self.describe_range(
                    "CPU_CLOCK",
                    self.session.cpu_clock_frames(),
                    start_ms,
                    end_ms,
                    1.0,
                ));
                lines.extend(self.describe_range(
                    "CPU_USAGE",
                    self.session.cpu_usage_frames(),
                    start_ms,
                    end_ms,
                    1.0,
                ));
                lines.extend(self.describe_range(
                    "FPS",
                    self.session.fps_frames(),
                    start_ms,
                    end_ms,
                    1.0,
                ));
                lines.extend(self.describe_range(
                    "BATTERY_TEMP",
                    self.session.battery_temperature_frames(),
                    start_ms,
                    end_ms,
                    10.0,
                ));
                lines.extend(self.describe_range(
                    "VOLTAGE",
                    self.session.battery_voltage_frames(),
                    start_ms,
                    end_ms,
                    1.0,
                ));
                lines.extend(self.describe_range(
                    "CURRENT",
                    self.session.battery_current_frames(),
                    start_ms,
                    end_ms,
                    1.0,
                ));
                lines.extend(self.describe_range(
                    "POWER",
                    self.session.battery_power_frames(),
                    start_ms,
                    end_ms,
                    1.0,
                ));
            }
            None => lines.push("Selection details will appear here.".to_string()),
        }

        div()
            .w(px(chart_width))
            .p_3()
            .rounded_md()
            .border_1()
            .bg(rgb(0xF2FAFF))
            .flex()
            .flex_col()
            .gap_1()
            .children(lines.into_iter().map(|line| div().text_sm().child(line)))
    }

    fn describe_point(
        &self,
        name: &str,
        frames: &[TimestampedBatch],
        ts: u64,
        scale: f64,
    ) -> Vec<String> {
        let Some(frame) = frames.iter().min_by_key(|f| f.timestamp_ms.abs_diff(ts)) else {
            return vec![format!("{name}: --")];
        };
        let mut parts = Vec::new();
        for (idx, value) in frame.batch.values.iter().copied().enumerate() {
            if value < 0 {
                continue;
            }
            parts.push(format!("v{idx}={:.2}", value as f64 / scale.max(1.0)));
        }
        if parts.is_empty() {
            vec![format!("{name}: --")]
        } else {
            vec![format!("{name}: {}", parts.join(", "))]
        }
    }

    fn describe_range(
        &self,
        name: &str,
        frames: &[TimestampedBatch],
        start_ms: u64,
        end_ms: u64,
        scale: f64,
    ) -> Vec<String> {
        let in_range: Vec<&TimestampedBatch> = frames
            .iter()
            .filter(|f| f.timestamp_ms >= start_ms && f.timestamp_ms <= end_ms)
            .collect();
        if in_range.is_empty() {
            return vec![format!("{name}: --")];
        }
        let mut lines = Vec::new();
        for idx in 0..10 {
            let mut vals = Vec::new();
            for frame in &in_range {
                if let Some(value) = frame.batch.values.get(idx).copied() {
                    if value >= 0 {
                        vals.push(value as f64 / scale.max(1.0));
                    }
                }
            }
            if vals.is_empty() {
                continue;
            }
            let sum: f64 = vals.iter().sum();
            let avg = sum / vals.len() as f64;
            let min = vals.iter().copied().fold(f64::INFINITY, f64::min);
            let max = vals.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            lines.push(format!(
                "{name}[v{idx}] avg={avg:.2} min={min:.2} max={max:.2}"
            ));
        }
        if lines.is_empty() {
            vec![format!("{name}: --")]
        } else {
            lines
        }
    }

    fn delete_current_selection(&mut self) -> bool {
        if !matches!(self.state, SessionState::Paused | SessionState::Stopped) {
            self.status_line = "Delete is only allowed in Paused or Stopped state.".to_string();
            return false;
        }
        let Some((start_ms, end_ms)) = self.selection_bounds_ms() else {
            return false;
        };
        let base_ts = self.session.global_start_timestamp_ms().unwrap_or(0);
        self.session.delete_range(start_ms, end_ms);
        self.selection = None;
        self.drag_selection = None;
        self.status_line = format!(
            "Deleted selected range [{:.3}s, {:.3}s] across all profilers.",
            start_ms.saturating_sub(base_ts) as f64 / 1000.0,
            end_ms.saturating_sub(base_ts) as f64 / 1000.0
        );
        true
    }

    fn selection_overlay_position(&self, rows: &[PlotRow]) -> Option<(f32, f32, bool)> {
        let (start, end, is_point) = match self.selection {
            Some(SelectionRange::Point(ts)) => (ts, ts, true),
            Some(SelectionRange::Span { start_ms, end_ms }) => (start_ms, end_ms, false),
            None => return None,
        };
        if is_point {
            let x = self.nearest_x_ratio_for_selected(rows).unwrap_or(0.0);
            return Some((x, 0.0, true));
        }
        let first = rows.first()?.timestamp_ms;
        let last = rows.last()?.timestamp_ms.max(first + 1);
        let total = (last - first) as f32;
        let left = ((start.saturating_sub(first)) as f32 / total).clamp(0.0, 1.0);
        let right = ((end.saturating_sub(first)) as f32 / total).clamp(0.0, 1.0);
        Some((left, (right - left).max(0.0), false))
    }

    fn interactive_plot(
        &self,
        view: Entity<Self>,
        chart_key: ChartKey,
        rows: Vec<PlotRow>,
        plot_width: f32,
        plot: impl IntoElement,
    ) -> impl IntoElement {
        let down_rows = rows.clone();
        let move_rows = rows.clone();
        let up_rows = rows.clone();
        let overlay = self.selection_overlay_position(&rows);
        div()
            .relative()
            .w(px(plot_width))
            .h(px(CHART_HEIGHT))
            .border_1()
            .rounded_md()
            .p(px(CHART_PLOT_INNER_PADDING))
            .child(plot)
            .when_some(overlay, |this, (left, width, is_point): (f32, f32, bool)| {
                if is_point {
                    this.child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(px((plot_width * left - 1.0).max(0.0)))
                            .w(px(2.0))
                            .bg(hsla(0.58, 0.75, 0.65, 0.65)),
                    )
                } else {
                    this.child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(px(plot_width * left))
                            .w(px((plot_width * width).max(2.0)))
                            .bg(hsla(0.58, 0.75, 0.65, 0.22)),
                    )
                }
            })
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let view_down = view.clone();
                        let rows_down = down_rows.clone();
                        window.on_mouse_event(move |ev: &MouseDownEvent, _, _, cx| {
                            if !bounds.contains(&ev.position) {
                                return;
                            }
                            let ratio = ((ev.position.x - bounds.origin.x) / bounds.size.width)
                                .clamp(0.0, 1.0);
                            match ev.button {
                                MouseButton::Left => {
                                    if let Some(ts) =
                                        PerfDroidDemo::nearest_timestamp_for_rows(&rows_down, ratio)
                                    {
                                        let _ = view_down.update(cx, |this, _| {
                                            if this.ignore_next_left_click {
                                                this.ignore_next_left_click = false;
                                                return;
                                            }
                                            this.selection = Some(SelectionRange::Point(ts));
                                            this.drag_selection = Some(DragSelection {
                                                start_ms: ts,
                                                current_ms: ts,
                                            });
                                        });
                                    }
                                }
                                MouseButton::Right => {
                                    let _ = view_down.update(cx, |this, _| {
                                        this.ignore_next_left_click = true;
                                        this.status_line = "Right-click menu opened. Choose Delete Selected Range."
                                            .to_string();
                                    });
                                }
                                _ => {}
                            }
                        });

                        let view_move = view.clone();
                        let rows_move = move_rows.clone();
                        window.on_mouse_event(move |ev: &MouseMoveEvent, _, _, cx| {
                            if !ev.dragging() {
                                return;
                            }
                            let ratio = ((ev.position.x - bounds.origin.x) / bounds.size.width)
                                .clamp(0.0, 1.0);
                            if let Some(ts) =
                                PerfDroidDemo::nearest_timestamp_for_rows(&rows_move, ratio)
                            {
                                let _ = view_move.update(cx, |this, _| {
                                    if let Some(drag) = this.drag_selection.as_mut() {
                                        drag.current_ms = ts;
                                    }
                                });
                            }
                        });

                        let view_up = view.clone();
                        let rows_up = up_rows.clone();
                        window.on_mouse_event(move |ev: &MouseUpEvent, _, _, cx| {
                            if ev.button != MouseButton::Left {
                                return;
                            }
                            let ratio = ((ev.position.x - bounds.origin.x) / bounds.size.width)
                                .clamp(0.0, 1.0);
                            let Some(ts) =
                                PerfDroidDemo::nearest_timestamp_for_rows(&rows_up, ratio)
                            else {
                                return;
                            };
                            let _ = view_up.update(cx, |this, _| {
                                let Some(drag) = this.drag_selection.take() else {
                                    return;
                                };
                                let start = drag.start_ms.min(ts).min(drag.current_ms);
                                let end = drag.start_ms.max(ts).max(drag.current_ms);
                                if start == end {
                                    this.selection = Some(SelectionRange::Point(start));
                                } else {
                                    this.selection = Some(SelectionRange::Span {
                                        start_ms: start,
                                        end_ms: end,
                                    });
                                }
                                this.status_line = format!(
                                    "Selection updated on {:?}: [{:.3}s, {:.3}s]",
                                    chart_key,
                                    start as f64 / 1000.0,
                                    end as f64 / 1000.0
                                );
                            });
                        });
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom_0(),
            )
    }

    fn render_cpu_clock_chart(&self, view: Entity<Self>, chart_width: f32) -> impl IntoElement {
        let rows = self.build_plot_rows();
        let chart_rows = rows.clone();
        let max_value = rows
            .iter()
            .flat_map(|row| row.values.iter().copied())
            .fold(0.0_f64, f64::max);
        let line_count = self
            .session
            .latest_cpu_clock()
            .map(|frame| {
                frame
                    .batch
                    .values
                    .iter()
                    .take_while(|value| **value >= 0)
                    .count()
                    .max(1)
            })
            .unwrap_or(1);

        let tick_margin = (rows.len() / 12).max(1);
        let mut chart = AreaChart::new(rows)
            .x(|row: &PlotRow| row.time_label.clone())
            .tick_margin(tick_margin);
        let plot_width = (chart_width - Y_AXIS_WIDTH).max(320.0);

        for line_idx in 0..line_count {
            let color = LINE_COLORS[line_idx % LINE_COLORS.len()];
            chart = chart
                .y(move |row: &PlotRow| row.values[line_idx])
                .stroke(rgb(color))
                .linear()
                .fill(transparent_black());
        }

        div()
            .w(px(chart_width))
            .flex()
            .flex_col()
            .gap_3()
            .child(section_title("CPU Clock"))
            .child(self.current_value_cards(chart_width))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(render_y_axis(max_value, "MHz"))
                    .child(self.interactive_plot(
                        view,
                        ChartKey::CpuClock,
                        chart_rows,
                        plot_width,
                        chart,
                    )),
            )
            .child(render_legend((0..line_count).map(|idx| {
                (format!("policy{idx}"), LINE_COLORS[idx % LINE_COLORS.len()])
            })))
            .child(chart_footer(
                "Metric: CPU_CLOCK | unit: MHz | fixed width values: 10",
            ))
    }

    fn render_fps_chart(&self, view: Entity<Self>, chart_width: f32) -> impl IntoElement {
        let rows = self.build_fps_rows();
        let chart_rows = rows.clone();
        let max_value = rows.iter().map(|row| row.values[0]).fold(0.0_f64, f64::max);
        let tick_margin = (rows.len() / 12).max(1);
        let plot_width = (chart_width - Y_AXIS_WIDTH).max(320.0);
        let chart = LineChart::new(rows)
            .x(|row: &PlotRow| row.time_label.clone())
            .y(|row: &PlotRow| row.values[0])
            .stroke(rgb(0xDC2626))
            .linear()
            .tick_margin(tick_margin);

        div()
            .w(px(chart_width))
            .flex()
            .flex_col()
            .gap_3()
            .child(section_title("FPS"))
            .child(
                div().flex().flex_row().justify_center().child(metric_card(
                    "FPS",
                    self.session
                        .latest_fps_value()
                        .map(|value| format!("{value} FPS"))
                        .unwrap_or_else(|| "--".to_string()),
                )),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(render_y_axis(max_value, "FPS"))
                    .child(self.interactive_plot(
                        view,
                        ChartKey::Fps,
                        chart_rows,
                        plot_width,
                        chart,
                    )),
            )
            .child(render_legend(std::iter::once((
                "main".to_string(),
                0xDC2626,
            ))))
            .child(chart_footer("Metric: FPS | unit: FPS | collector: main"))
    }

    fn render_battery_temperature_chart(
        &self,
        view: Entity<Self>,
        chart_width: f32,
    ) -> impl IntoElement {
        let rows = self.build_battery_temperature_rows();
        let chart_rows = rows.clone();
        let max_value = rows
            .iter()
            .map(|row| row.values[0])
            .fold(0.0_f64, f64::max)
            .max(30.0);
        let tick_margin = (rows.len() / 12).max(1);
        let plot_width = (chart_width - Y_AXIS_WIDTH).max(320.0);
        let chart = LineChart::new(rows)
            .x(|row: &PlotRow| row.time_label.clone())
            .y(|row: &PlotRow| row.values[0])
            .stroke(rgb(0xD97706))
            .linear()
            .tick_margin(tick_margin);

        div()
            .w(px(chart_width))
            .flex()
            .flex_col()
            .gap_3()
            .child(section_title("Battery Temperature"))
            .child(
                div().flex().flex_row().justify_center().child(metric_card(
                    "Battery",
                    self.session
                        .latest_battery_temperature_value()
                        .map(format_temperature_value)
                        .unwrap_or_else(|| "--".to_string()),
                )),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(render_decimal_y_axis(max_value, "C"))
                    .child(self.interactive_plot(
                        view,
                        ChartKey::BatteryTemp,
                        chart_rows,
                        plot_width,
                        chart,
                    )),
            )
            .child(render_legend(std::iter::once((
                "battery".to_string(),
                0xD97706,
            ))))
            .child(chart_footer(
                "Metric: BATTERY_TEMP | unit: 0.1C | collector: battery",
            ))
    }

    fn render_battery_power_metrics_chart(
        &self,
        view: Entity<Self>,
        chart_width: f32,
    ) -> impl IntoElement {
        let rows = self.build_battery_power_metric_rows();
        let chart_rows = rows.clone();
        let voltage_enabled = !self.session.battery_voltage_frames().is_empty();
        let current_enabled = !self.session.battery_current_frames().is_empty();
        let power_enabled = !self.session.battery_power_frames().is_empty();
        let max_value = rows
            .iter()
            .flat_map(|row| row.values.iter().take(3).copied())
            .fold(0.0_f64, f64::max)
            .max(100.0);
        let tick_margin = (rows.len() / 12).max(1);
        let plot_width = (chart_width - Y_AXIS_WIDTH).max(320.0);
        let mut chart = AreaChart::new(rows)
            .x(|row: &PlotRow| row.time_label.clone())
            .tick_margin(tick_margin);

        let mut legend_items = Vec::new();

        for (enabled, label, value_index, color) in [
            (voltage_enabled, "voltage (mV)", 0usize, 0x2563EB),
            (current_enabled, "current (mA)", 1usize, 0x10B981),
            (power_enabled, "power (mW)", 2usize, 0xDB2777),
        ] {
            if !enabled {
                continue;
            }

            legend_items.push((label.to_string(), color));
            chart = chart
                .y(move |row: &PlotRow| row.values[value_index])
                .stroke(rgb(color))
                .linear()
                .fill(transparent_black());
        }

        div()
            .w(px(chart_width))
            .flex()
            .flex_col()
            .gap_3()
            .child(section_title("Battery Power Metrics"))
            .child(
                div()
                    .w(px(chart_width))
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .justify_center()
                    .gap_2()
                    .child(metric_card(
                        "Voltage",
                        self.session
                            .latest_battery_voltage_value()
                            .map(format_voltage_value)
                            .unwrap_or_else(|| "--".to_string()),
                    ))
                    .child(metric_card(
                        "Current",
                        self.session
                            .latest_battery_current_value()
                            .map(format_current_value)
                            .unwrap_or_else(|| "--".to_string()),
                    ))
                    .child(metric_card(
                        "Power",
                        self.session
                            .latest_battery_power_value()
                            .map(format_power_value)
                            .unwrap_or_else(|| "--".to_string()),
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(render_numeric_y_axis(max_value))
                    .child(self.interactive_plot(
                        view,
                        ChartKey::BatteryPowerMetrics,
                        chart_rows,
                        plot_width,
                        chart,
                    )),
            )
            .child(render_legend(legend_items))
            .child(chart_footer(
                "Metrics: VOLTAGE / CURRENT / POWER | units: mV, mA, mW | collector: battery",
            ))
    }

    fn build_cpu_usage_rows(&self) -> Vec<PlotRow> {
        let Some(base_ts) = self.session.global_start_timestamp_ms() else {
            return Vec::new();
        };

        self.session
            .cpu_usage_frames()
            .iter()
            .map(|frame| {
                let elapsed_s = (frame.timestamp_ms.saturating_sub(base_ts)) as f64 / 1000.0;
                let mut values = [0.0; 10];
                for (idx, value) in frame.batch.values.iter().copied().enumerate().take(10) {
                    values[idx] = if value < 0 { 0.0 } else { value as f64 };
                }

                PlotRow {
                    timestamp_ms: frame.timestamp_ms,
                    time_label: format!("{elapsed_s:.3}s").into(),
                    values,
                }
            })
            .collect()
    }

    fn render_cpu_usage_chart(&self, view: Entity<Self>, chart_width: f32) -> impl IntoElement {
        let rows = self.build_cpu_usage_rows();
        let chart_rows = rows.clone();
        let max_value = rows
            .iter()
            .flat_map(|row| row.values.iter().copied())
            .fold(0.0_f64, f64::max)
            .max(100.0);
        let line_count = self
            .session
            .latest_cpu_usage()
            .map(|frame| {
                frame
                    .batch
                    .values
                    .iter()
                    .take_while(|value| **value >= 0)
                    .count()
                    .max(1)
            })
            .unwrap_or(1);

        let tick_margin = (rows.len() / 12).max(1);
        let mut chart = AreaChart::new(rows)
            .x(|row: &PlotRow| row.time_label.clone())
            .tick_margin(tick_margin);
        let plot_width = (chart_width - Y_AXIS_WIDTH).max(320.0);

        for line_idx in 0..line_count {
            let color = LINE_COLORS[line_idx % LINE_COLORS.len()];
            chart = chart
                .y(move |row: &PlotRow| row.values[line_idx])
                .stroke(rgb(color))
                .linear()
                .fill(transparent_black());
        }

        div()
            .w(px(chart_width))
            .flex()
            .flex_col()
            .gap_3()
            .child(section_title("CPU Usage"))
            .child(self.current_cpu_usage_cards(chart_width))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(render_y_axis(max_value, "%"))
                    .child(self.interactive_plot(
                        view,
                        ChartKey::CpuUsage,
                        chart_rows,
                        plot_width,
                        chart,
                    )),
            )
            .child(render_legend((0..line_count).map(|idx| {
                (format!("policy{idx}"), LINE_COLORS[idx % LINE_COLORS.len()])
            })))
            .child(chart_footer(
                "Metric: CPU_USAGE | unit: % | fixed width values: 10",
            ))
    }

    fn current_value_cards(&self, chart_width: f32) -> impl IntoElement {
        let latest = self.session.latest_values();
        if latest.is_empty() {
            return div()
                .w(px(chart_width))
                .flex()
                .flex_row()
                .flex_wrap()
                .justify_center()
                .gap_2()
                .child(metric_card("CPU policy", "--"));
        }

        div()
            .w(px(chart_width))
            .flex()
            .flex_row()
            .flex_wrap()
            .justify_center()
            .gap_2()
            .children(latest.into_iter().enumerate().filter_map(|(idx, value)| {
                value.map(|value| metric_card(format!("policy{idx}"), format!("{value} MHz")))
            }))
    }

    fn current_cpu_usage_cards(&self, chart_width: f32) -> impl IntoElement {
        let latest = self.session.latest_cpu_usage_values();
        if latest.is_empty() {
            return div()
                .w(px(chart_width))
                .flex()
                .flex_row()
                .flex_wrap()
                .justify_center()
                .gap_2()
                .child(metric_card("CPU policy", "--"));
        }

        div()
            .w(px(chart_width))
            .flex()
            .flex_row()
            .flex_wrap()
            .justify_center()
            .gap_2()
            .children(latest.into_iter().enumerate().filter_map(|(idx, value)| {
                value.map(|value| metric_card(format!("policy{idx}"), format!("{value} %")))
            }))
    }

    fn render_hz_input(&self) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .items_start()
            .child(form_label("Sampling Rate (1-10 Hz)"))
            .child(
                Input::new(&self.hz_input)
                    .cleanable(true)
                    .disabled(self.is_busy),
            )
            .child(helper_text(format!(
                "Selected sampling rate: {} Hz",
                self.selected_hz
            )))
            .child(helper_text("Allowed range: 1-10 Hz"))
    }

    fn render_device_part(&self, panel_width: f32) -> impl IntoElement {
        let mut lines = if let Some(device) = &self.device {
            vec![
                format!("Model: {}", device.model),
                format!("Serial: {}", device.serial),
                format!("Connection: {}", device.connection.as_str()),
                format!("Android: {}", device.android_version),
                format!("SoC: {}", device.soc_model),
            ]
        } else {
            vec![
                "Model: --".to_string(),
                "Serial: --".to_string(),
                "Connection: --".to_string(),
                "Android: --".to_string(),
                "SoC: --".to_string(),
            ]
        };
        lines.extend([
            format!(
                "Frames cached: {}",
                self.session.cpu_clock_frames().len()
                    + self.session.cpu_usage_frames().len()
                    + self.session.fps_frames().len()
                    + self.session.battery_temperature_frames().len()
                    + self.session.battery_voltage_frames().len()
                    + self.session.battery_current_frames().len()
                    + self.session.battery_power_frames().len()
            ),
            format!("Runtime snapshot: {}", self.runtime.state().as_str()),
        ]);

        section_card(
            "Device Part",
            div()
                .flex()
                .flex_col()
                .gap_3()
                .children(lines.into_iter().map(info_row)),
            panel_width,
        )
    }

    fn render_control_part(&self, panel_width: f32) -> impl IntoElement {
        let can_start = !self.is_busy && self.state.allows(ControlCommand::Start);
        let can_pause = !self.is_busy && self.state.allows(ControlCommand::Pause);
        let can_restart = !self.is_busy && self.state.allows(ControlCommand::Restart);
        let can_stop = !self.is_busy && self.state.allows(ControlCommand::Stop);

        let runtime = Arc::clone(&self.runtime);
        let start = Button::new("start")
            .label(format!("Start {}Hz", self.selected_hz))
            .on_click(move |_, _, _| runtime.request_start(runtime.selected_hz()))
            .disabled(!can_start);

        let runtime = Arc::clone(&self.runtime);
        let pause = Button::new("pause")
            .label("Pause")
            .on_click(move |_, _, _| runtime.request_pause())
            .disabled(!can_pause);

        let runtime = Arc::clone(&self.runtime);
        let restart = Button::new("continue")
            .label("Continue")
            .on_click(move |_, _, _| runtime.request_restart())
            .disabled(!can_restart);

        let runtime = Arc::clone(&self.runtime);
        let stop = Button::new("stop")
            .label("Stop")
            .on_click(move |_, _, _| runtime.request_stop())
            .disabled(!can_stop);

        let export_allowed = self.can_export() && !self.is_busy;
        let export_state = self.state;
        let export_hz = self.selected_hz;
        let export_session = self.session.clone();
        let export_runtime = Arc::clone(&self.runtime);
        let export_csv = Button::new("export-csv")
            .label("Export CSV")
            .on_click(move |_, _, cx| {
                if !matches!(export_state, SessionState::Paused | SessionState::Stopped) {
                    export_runtime
                        .request_status("CSV export is only available in Paused or Stopped state.");
                    return;
                }

                let initial_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                let receiver = cx.prompt_for_new_path(&initial_dir, Some("perfdroid_session.csv"));
                let export_runtime = Arc::clone(&export_runtime);
                let export_session = export_session.clone();
                cx.background_executor()
                    .spawn(async move {
                        let selected_path = match receiver.await {
                            Ok(Ok(Some(path))) => path,
                            Ok(Ok(None)) => {
                                export_runtime.request_status("CSV export canceled.");
                                return;
                            }
                            Ok(Err(err)) => {
                                export_runtime.request_status(format!(
                                    "failed to open save dialog for CSV export: {err}"
                                ));
                                return;
                            }
                            Err(err) => {
                                export_runtime.request_status(format!(
                                    "failed while waiting for CSV save dialog result: {err}"
                                ));
                                return;
                            }
                        };

                        let output_path = ensure_csv_extension(selected_path);
                        match export_session_to_csv(&output_path, &export_session, export_hz) {
                            Ok(rows) => export_runtime.request_status(format!(
                                "CSV exported: {} row(s) -> {}",
                                rows,
                                output_path.display()
                            )),
                            Err(err) => {
                                export_runtime.request_status(format!("CSV export failed: {err}"))
                            }
                        }
                    })
                    .detach();
            })
            .disabled(!export_allowed);
        let export_state = self.state;
        let export_hz = self.selected_hz;
        let export_session = self.session.clone();
        let export_runtime = Arc::clone(&self.runtime);
        let export_json = Button::new("export-json")
            .label("Export JSON")
            .on_click(move |_, _, cx| {
                if !matches!(export_state, SessionState::Paused | SessionState::Stopped) {
                    export_runtime.request_status(
                        "JSON export is only available in Paused or Stopped state.",
                    );
                    return;
                }
                let initial_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                let receiver = cx.prompt_for_new_path(&initial_dir, Some("perfdroid_session.json"));
                let export_runtime = Arc::clone(&export_runtime);
                let export_session = export_session.clone();
                cx.background_executor()
                    .spawn(async move {
                        let selected_path = match receiver.await {
                            Ok(Ok(Some(path))) => path,
                            Ok(Ok(None)) => {
                                export_runtime.request_status("JSON export canceled.");
                                return;
                            }
                            Ok(Err(err)) => {
                                export_runtime.request_status(format!(
                                    "failed to open save dialog for JSON export: {err}"
                                ));
                                return;
                            }
                            Err(err) => {
                                export_runtime.request_status(format!(
                                    "failed while waiting for JSON save dialog result: {err}"
                                ));
                                return;
                            }
                        };
                        let output_path = ensure_extension(selected_path, "json");
                        match export_session_to_json(&output_path, &export_session, export_hz) {
                            Ok(rows) => export_runtime.request_status(format!(
                                "JSON exported: {} row(s) -> {}",
                                rows,
                                output_path.display()
                            )),
                            Err(err) => {
                                export_runtime.request_status(format!("JSON export failed: {err}"))
                            }
                        }
                    })
                    .detach();
            })
            .disabled(!export_allowed);
        let export_state = self.state;
        let export_hz = self.selected_hz;
        let export_session = self.session.clone();
        let export_runtime = Arc::clone(&self.runtime);
        let export_html = Button::new("export-html")
            .label("Export HTML")
            .on_click(move |_, _, cx| {
                if !matches!(export_state, SessionState::Paused | SessionState::Stopped) {
                    export_runtime.request_status(
                        "HTML export is only available in Paused or Stopped state.",
                    );
                    return;
                }
                let initial_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                let receiver = cx.prompt_for_new_path(&initial_dir, Some("perfdroid_report.html"));
                let export_runtime = Arc::clone(&export_runtime);
                let export_session = export_session.clone();
                cx.background_executor()
                    .spawn(async move {
                        let selected_path = match receiver.await {
                            Ok(Ok(Some(path))) => path,
                            Ok(Ok(None)) => {
                                export_runtime.request_status("HTML export canceled.");
                                return;
                            }
                            Ok(Err(err)) => {
                                export_runtime.request_status(format!(
                                    "failed to open save dialog for HTML export: {err}"
                                ));
                                return;
                            }
                            Err(err) => {
                                export_runtime.request_status(format!(
                                    "failed while waiting for HTML save dialog result: {err}"
                                ));
                                return;
                            }
                        };
                        let output_path = ensure_extension(selected_path, "html");
                        match export_session_to_html(&output_path, &export_session, export_hz) {
                            Ok(rows) => export_runtime.request_status(format!(
                                "HTML report exported: {} row(s) -> {}",
                                rows,
                                output_path.display()
                            )),
                            Err(err) => {
                                export_runtime.request_status(format!("HTML export failed: {err}"))
                            }
                        }
                    })
                    .detach();
            })
            .disabled(!export_allowed);
        let export_state = self.state;
        let export_session = self.session.clone();
        let export_runtime = Arc::clone(&self.runtime);
        let export_png = Button::new("export-png")
            .label("Export PNG")
            .on_click(move |_, _, cx| {
                if !matches!(export_state, SessionState::Paused | SessionState::Stopped) {
                    export_runtime
                        .request_status("PNG export is only available in Paused or Stopped state.");
                    return;
                }
                let initial_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                let receiver = cx.prompt_for_new_path(&initial_dir, Some("perfdroid_report.png"));
                let export_runtime = Arc::clone(&export_runtime);
                let export_session = export_session.clone();
                cx.background_executor()
                    .spawn(async move {
                        let selected_path = match receiver.await {
                            Ok(Ok(Some(path))) => path,
                            Ok(Ok(None)) => {
                                export_runtime.request_status("PNG export canceled.");
                                return;
                            }
                            Ok(Err(err)) => {
                                export_runtime.request_status(format!(
                                    "failed to open save dialog for PNG export: {err}"
                                ));
                                return;
                            }
                            Err(err) => {
                                export_runtime.request_status(format!(
                                    "failed while waiting for PNG save dialog result: {err}"
                                ));
                                return;
                            }
                        };
                        let output_path = ensure_extension(selected_path, "png");
                        match export_session_to_png(&output_path, &export_session) {
                            Ok(rows) => export_runtime.request_status(format!(
                                "PNG report exported: {} row(s) -> {}",
                                rows,
                                output_path.display()
                            )),
                            Err(err) => {
                                export_runtime.request_status(format!("PNG export failed: {err}"))
                            }
                        }
                    })
                    .detach();
            })
            .disabled(!export_allowed);

        section_card(
            "Control Part",
            div()
                .flex()
                .flex_col()
                .gap_4()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_between()
                        .items_center()
                        .child(form_label("Session State"))
                        .child(status_pill(self.state.as_str())),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(form_label("Target Package For FPS"))
                        .child(
                            Input::new(&self.package_input)
                                .cleanable(true)
                                .disabled(self.is_busy),
                        )
                        .child(helper_text(format!(
                            "Current package: {}",
                            if self.package_name.trim().is_empty() {
                                "--"
                            } else {
                                self.package_name.as_str()
                            }
                        ))),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(self.render_hz_input()),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .gap_2()
                        .justify_center()
                        .child(start)
                        .child(pause)
                        .child(restart)
                        .child(stop),
                )
                .child(form_label("Status"))
                .child(
                    div()
                        .w_full()
                        .p_3()
                        .rounded_md()
                        .bg(rgb(0xF4ECE0))
                        .border_1()
                        .child(
                            div()
                                .w_full()
                                .whitespace_normal()
                                .text_center()
                                .child(self.status_line.clone()),
                        ),
                )
                .child(form_label("Export"))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .p_2()
                        .rounded_md()
                        .bg(rgb(0xFAF3E8))
                        .border_1()
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .justify_center()
                                .gap_2()
                                .child(div().w(px(150.0)).child(export_csv))
                                .child(div().w(px(150.0)).child(export_json)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .justify_center()
                                .gap_2()
                                .child(div().w(px(150.0)).child(export_html))
                                .child(div().w(px(150.0)).child(export_png)),
                        )
                        .child(helper_text(
                            "Export is only enabled when session state is Paused or Stopped.",
                        )),
                ),
            panel_width,
        )
    }

    fn can_export(&self) -> bool {
        matches!(self.state, SessionState::Paused | SessionState::Stopped)
    }

    fn render_adb_part(&self, panel_width: f32) -> impl IntoElement {
        let runtime = Arc::clone(&self.runtime);
        let detect = Button::new("detect-devices")
            .primary()
            .label("Detect USB Devices")
            .on_click(move |_, _, _| runtime.request_refresh_devices())
            .disabled(self.is_busy);

        let content = if self.detected_devices.is_empty() {
            div()
                .flex()
                .flex_col()
                .gap_4()
                .child(detect)
                .child(helper_text(
                    "No USB-connected ADB devices listed yet. Detect devices first, then choose Wired or Wireless.",
                ))
                .child(helper_text(
                    "Wireless uses the selected USB device to switch ADB onto WiFi. After it connects, the USB cable can be removed.",
                ))
        } else {
            div()
                .flex()
                .flex_col()
                .gap_4()
                .child(detect)
                .child(helper_text(
                    "Wireless uses the selected USB device to switch ADB onto WiFi. After it connects, the USB cable can be removed.",
                ))
                .children(
                    self.detected_devices
                        .iter()
                        .cloned()
                        .map(|device| self.render_detected_device_card(device)),
                )
        };

        section_card("ADB Device Part", content, panel_width)
    }

    fn render_detected_device_card(&self, device: AdbDetectedDevice) -> impl IntoElement {
        let can_connect = !self.is_busy && self.state.allows(ControlCommand::Connect);
        let serial_id = stable_u64(&device.serial);
        let usb_runtime = Arc::clone(&self.runtime);
        let usb_serial = device.serial.clone();
        let connect_usb = Button::new(("usb", serial_id))
            .label("Wired")
            .on_click(move |_, _, _| usb_runtime.request_connect_usb(usb_serial.clone()))
            .disabled(!can_connect);

        let wifi_runtime = Arc::clone(&self.runtime);
        let wifi_serial = device.serial.clone();
        let connect_wifi = Button::new(("wifi", serial_id))
            .label("Wireless")
            .on_click(move |_, _, _| wifi_runtime.request_connect_wireless(wifi_serial.clone()))
            .disabled(!can_connect);

        div()
            .w_full()
            .p_4()
            .rounded_md()
            .border_1()
            .bg(rgb(0xF7EEE0))
            .flex()
            .flex_col()
            .gap_3()
            .child(form_label(format!("{} ({})", device.model, device.serial)))
            .child(info_row(format!("ADB state: {}", device.adb_state)))
            .child(info_row(format!(
                "Detected transport: {}",
                device.connection.as_str()
            )))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap_2()
                    .child(connect_usb)
                    .child(connect_wifi),
            )
    }

    fn render_connection_overlay(&self) -> impl IntoElement {
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .occlude()
            .flex()
            .justify_center()
            .items_center()
            .bg(hsla(0.0, 0.0, 0.0, 0.38))
            .child(
                div()
                    .w(px(320.0))
                    .p_6()
                    .rounded_lg()
                    .border_1()
                    .bg(rgb(0xFFF9F1))
                    .flex()
                    .flex_col()
                    .justify_center()
                    .items_center()
                    .gap_3()
                    .child(
                        Spinner::new()
                            .with_size(ComponentSize::Large)
                            .color(hsla(0.07, 0.75, 0.45, 1.0)),
                    )
                    .child(
                        div()
                            .text_lg()
                            .font_semibold()
                            .text_center()
                            .child("Working"),
                    )
                    .child(div().text_sm().text_center().whitespace_normal().child(
                        if self.busy_message.trim().is_empty() {
                            "Please wait while PerfDroid finishes the current device operation."
                                .to_string()
                        } else {
                            self.busy_message.clone()
                        },
                    ))
                    .child(helper_text("Other page actions are temporarily disabled.")),
            )
    }
}

impl Render for PerfDroidDemo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.drain_events();
        window.request_animation_frame();
        let view = cx.entity().clone();
        let window_width = f32::from(window.bounds().size.width);
        let content_width = (window_width - APP_PADDING_X).max(360.0);
        let panel_width = if content_width >= 1100.0 {
            ((content_width - 24.0) / 3.0).max(320.0)
        } else if content_width >= 720.0 {
            ((content_width - 12.0) / 2.0).max(320.0)
        } else {
            content_width
        };
        let chart_width = (content_width - CHART_SECTION_PADDING_X).max(320.0);

        let mut root = div().relative().size_full().child(
            div()
                .size_full()
                .flex()
                .flex_col()
                .overflow_y_scrollbar()
                .gap_5()
                .p_5()
                .bg(rgb(0xF3EBDD))
                .child(self.render_header())
                .child(div().h(px(16.0)))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .gap_4()
                        .justify_center()
                        .child(self.render_adb_part(panel_width))
                        .child(self.render_device_part(panel_width))
                        .child(self.render_control_part(panel_width)),
                )
                .child(div().h(px(20.0)))
                .child(chart_section(
                    self.render_selection_summary(view.clone(), chart_width),
                ))
                .child(div().h(px(8.0)))
                .child(chart_section(self.render_selection_details(chart_width)))
                .child(div().h(px(8.0)))
                .child(chart_section(
                    self.render_cpu_clock_chart(view.clone(), chart_width),
                ))
                .child(div().h(px(12.0)))
                .child(chart_section(
                    self.render_cpu_usage_chart(view.clone(), chart_width),
                ))
                .child(div().h(px(12.0)))
                .child(chart_section(
                    self.render_battery_temperature_chart(view.clone(), chart_width),
                ))
                .child(div().h(px(12.0)))
                .child(chart_section(
                    self.render_battery_power_metrics_chart(view.clone(), chart_width),
                ))
                .child(div().h(px(12.0)))
                .child(chart_section(self.render_fps_chart(view, chart_width))),
        );

        if self.is_busy {
            root = root.child(self.render_connection_overlay());
        }

        root
    }
}

impl PerfDroidDemo {
    fn render_header(&self) -> impl IntoElement {
        div()
            .w_full()
            .p_4()
            .rounded_lg()
            .border_1()
            .bg(rgb(0xFFF8EE))
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .child(section_title("PerfDroid"))
            .child(
                div()
                    .text_center()
                    .child("ADB-based Android performance collection."),
            )
    }
}

fn section_card(
    title: impl Into<String>,
    content: impl IntoElement,
    panel_width: f32,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_4()
        .w(px(panel_width))
        .min_h(px(240.0))
        .p_5()
        .rounded_lg()
        .border_1()
        .bg(rgb(0xFFF9F1))
        .child(section_title(title.into()))
        .child(content)
}

fn metric_card(label: impl Into<String>, value: impl Into<String>) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap_2()
        .min_w(px(140.0))
        .p_4()
        .rounded_md()
        .border_1()
        .bg(rgb(0xF7EEE0))
        .child(form_label(label.into()))
        .child(div().text_center().child(value.into()))
}

fn render_y_axis(max_value: f64, unit: &str) -> impl IntoElement {
    let top = max_value.ceil() as i64;
    let middle = (max_value / 2.0).ceil() as i64;
    div()
        .w(px(Y_AXIS_WIDTH))
        .h(px(CHART_HEIGHT))
        .flex()
        .flex_col()
        .justify_between()
        .items_end()
        .pr_2()
        .text_sm()
        .child(format!("{top} {unit}"))
        .child(format!("{middle} {unit}"))
        .child(format!("0 {unit}"))
}

fn render_decimal_y_axis(max_value: f64, unit: &str) -> impl IntoElement {
    let top = max_value.max(1.0);
    let middle = top / 2.0;
    div()
        .w(px(Y_AXIS_WIDTH))
        .h(px(CHART_HEIGHT))
        .flex()
        .flex_col()
        .justify_between()
        .items_end()
        .pr_2()
        .text_sm()
        .child(format!("{top:.1} {unit}"))
        .child(format!("{middle:.1} {unit}"))
        .child(format!("0.0 {unit}"))
}

fn render_numeric_y_axis(max_value: f64) -> impl IntoElement {
    let top = max_value.max(1.0);
    let middle = top / 2.0;
    div()
        .w(px(Y_AXIS_WIDTH))
        .h(px(CHART_HEIGHT))
        .flex()
        .flex_col()
        .justify_between()
        .items_end()
        .pr_2()
        .text_sm()
        .child(format!("{top:.0}"))
        .child(format!("{middle:.0}"))
        .child("0")
}

fn render_legend(items: impl IntoIterator<Item = (String, u32)>) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .gap_2()
        .children(items.into_iter().map(|(label, color)| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .px_2()
                .py_1()
                .rounded_sm()
                .border_1()
                .child(div().w(px(12.0)).h(px(3.0)).bg(rgb(color)))
                .child(label)
        }))
}

fn section_title(title: impl Into<String>) -> impl IntoElement {
    div()
        .w_full()
        .text_center()
        .text_xl()
        .font_semibold()
        .child(title.into())
}

fn form_label(label: impl Into<String>) -> impl IntoElement {
    div().font_semibold().child(label.into())
}

fn helper_text(text: impl Into<String>) -> impl IntoElement {
    div().text_sm().whitespace_normal().child(text.into())
}

fn info_row(text: impl Into<String>) -> impl IntoElement {
    div()
        .w_full()
        .p_2()
        .rounded_md()
        .bg(rgb(0xF7EEE0))
        .border_1()
        .child(text.into())
}

fn status_pill(text: impl Into<String>) -> impl IntoElement {
    div()
        .px_3()
        .py_1()
        .rounded_full()
        .bg(rgb(0xE6D5BD))
        .font_semibold()
        .child(text.into())
}

fn chart_footer(text: impl Into<String>) -> impl IntoElement {
    div().text_center().child(text.into())
}

fn chart_section(content: impl IntoElement) -> impl IntoElement {
    div()
        .w_full()
        .p_4()
        .rounded_lg()
        .border_1()
        .bg(rgb(0xFFF9F1))
        .child(content)
}

fn format_temperature_value(raw_deci_c: i64) -> String {
    format!("{:.1} C", raw_deci_c as f64 / 10.0)
}

fn format_voltage_value(raw_mv: i64) -> String {
    format!("{raw_mv} mV")
}

fn format_current_value(raw_ma: i64) -> String {
    format!("{raw_ma} mA")
}

fn format_power_value(raw_mw: i64) -> String {
    format!("{raw_mw} mW")
}

fn frame_scalar_value(frame: &TimestampedBatch) -> Option<f64> {
    frame
        .batch
        .values
        .first()
        .copied()
        .filter(|value| *value >= 0)
        .map(|value| value as f64)
}

fn ensure_csv_extension(path: PathBuf) -> PathBuf {
    ensure_extension(path, "csv")
}

fn ensure_extension(path: PathBuf, ext: &str) -> PathBuf {
    if path
        .extension()
        .and_then(|current| current.to_str())
        .is_some_and(|current| current.eq_ignore_ascii_case(ext))
    {
        path
    } else {
        path.with_extension(ext)
    }
}

fn stable_u64(value: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}
