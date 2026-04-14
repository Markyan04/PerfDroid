use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::{collections::hash_map::DefaultHasher};

use gpui::{
    App, AppContext, Application, Bounds, Context, Entity, IntoElement, ParentElement, Render,
    SharedString, Styled, Subscription, Window, WindowBounds, WindowOptions, div, px, rgb, size,
    transparent_black,
};
use pdcore::types::ControlCommand;
use gpui_component::Root;
use gpui_component::StyledExt;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::chart::{AreaChart, LineChart};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::ScrollableElement;
use gpui_component::Disableable;

use crate::aggregation::AggregatorEvent;
use crate::device::{AdbDetectedDevice, DeviceDescriptor};
use crate::export::export_session_to_csv;
use crate::runtime::PerfDroidRuntime;
use crate::session::SessionState;
use crate::storage::SessionStore;

const WINDOW_WIDTH: f32 = 1440.0;
const WINDOW_HEIGHT: f32 = 960.0;
const CHART_HEIGHT: f32 = 250.0;
const Y_AXIS_WIDTH: f32 = 72.0;
const APP_PADDING_X: f32 = 48.0;
const CHART_SECTION_PADDING_X: f32 = 40.0;
const LINE_COLORS: [u32; 10] = [
    0x2563EB, 0xF97316, 0x10B981, 0xDB2777, 0x7C3AED, 0x0F766E, 0xDC2626, 0xCA8A04, 0x4F46E5,
    0x0891B2,
];

#[derive(Clone)]
struct PlotRow {
    time_label: SharedString,
    values: [f64; 10],
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
    selected_hz: u64,
    package_name: String,
    package_input: Entity<InputState>,
    hz_input: Entity<InputState>,
    _package_input_subscription: Subscription,
    _hz_input_subscription: Subscription,
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
            selected_hz: 4,
            package_name: initial_package_name,
            package_input,
            hz_input,
            _package_input_subscription: package_input_subscription,
            _hz_input_subscription: hz_input_subscription,
        }
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                AggregatorEvent::StateChanged(state) => {
                    self.state = state;
                    if state == SessionState::Connected {
                        self.session = SessionStore::default();
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
        let Some(first) = self.session.cpu_clock_frames().first() else {
            return Vec::new();
        };

        self.session
            .cpu_clock_frames()
            .iter()
            .map(|frame| {
                let elapsed_s =
                    (frame.timestamp_ms.saturating_sub(first.timestamp_ms)) as f64 / 1000.0;
                let mut values = [0.0; 10];
                for (idx, value) in frame.batch.values.iter().copied().enumerate().take(10) {
                    values[idx] = if value < 0 { 0.0 } else { value as f64 };
                }

                PlotRow {
                    time_label: format!("{elapsed_s:.1}s").into(),
                    values,
                }
            })
            .collect()
    }

    fn build_fps_rows(&self) -> Vec<PlotRow> {
        let Some(first) = self.session.fps_frames().first() else {
            return Vec::new();
        };

        self.session
            .fps_frames()
            .iter()
            .map(|frame| {
                let elapsed_s =
                    (frame.timestamp_ms.saturating_sub(first.timestamp_ms)) as f64 / 1000.0;
                let mut values = [0.0; 10];
                if let Some(value) = frame.batch.values.first().copied() {
                    values[0] = if value < 0 { 0.0 } else { value as f64 };
                }

                PlotRow {
                    time_label: format!("{elapsed_s:.1}s").into(),
                    values,
                }
            })
            .collect()
    }

    fn render_cpu_clock_chart(&self, chart_width: f32) -> impl IntoElement {
        let rows = self.build_plot_rows();
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
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .justify_center()
                    .gap_2()
                    .child(self.current_value_cards()),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(render_y_axis(max_value, "MHz"))
                    .child(
                        div()
                            .w(px(plot_width))
                            .h(px(CHART_HEIGHT))
                            .border_1()
                            .rounded_md()
                            .p_2()
                            .child(chart),
                    ),
            )
            .child(render_legend((0..line_count).map(|idx| {
                (format!("policy{idx}"), LINE_COLORS[idx % LINE_COLORS.len()])
            })))
            .child(chart_footer(
                "Metric: CPU_CLOCK | unit: MHz | fixed width values: 10",
            ))
    }

    fn render_fps_chart(&self, chart_width: f32) -> impl IntoElement {
        let rows = self.build_fps_rows();
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
                    .child(
                        div()
                            .w(px(plot_width))
                            .h(px(CHART_HEIGHT))
                            .border_1()
                            .rounded_md()
                            .p_2()
                            .child(chart),
                    ),
            )
            .child(render_legend(std::iter::once((
                "main".to_string(),
                0xDC2626,
            ))))
            .child(chart_footer("Metric: FPS | unit: FPS | collector: main"))
    }

    fn current_value_cards(&self) -> impl IntoElement {
        let latest = self.session.latest_values();
        if latest.is_empty() {
            return div().child(metric_card("CPU policy", "--"));
        }

        div().children(latest.into_iter().enumerate().filter_map(|(idx, value)| {
            value.map(|value| metric_card(format!("policy{idx}"), format!("{value} MHz")))
        }))
    }

    fn render_hz_input(&self) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .items_start()
            .child(form_label("Sampling Rate (1-10 Hz)"))
            .child(Input::new(&self.hz_input).cleanable(true))
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
                self.session.cpu_clock_frames().len() + self.session.fps_frames().len()
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
        let can_start = self.state.allows(ControlCommand::Start);
        let can_pause = self.state.allows(ControlCommand::Pause);
        let can_restart = self.state.allows(ControlCommand::Restart);
        let can_stop = self.state.allows(ControlCommand::Stop);

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

        let export_allowed = self.can_export_csv();
        let export_state = self.state;
        let export_hz = self.selected_hz;
        let export_session = self.session.clone();
        let export_runtime = Arc::clone(&self.runtime);
        let export_csv = Button::new("export-csv")
            .label("Export CSV")
            .on_click(move |_, _, cx| {
                if !matches!(export_state, SessionState::Paused | SessionState::Stopped) {
                    export_runtime.request_status(
                        "CSV export is only available in Paused or Stopped state.",
                    );
                    return;
                }

                let initial_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                let receiver =
                    cx.prompt_for_new_path(&initial_dir, Some("perfdroid_session.csv"));
                let export_runtime = Arc::clone(&export_runtime);
                let export_session = export_session.clone();
                cx.background_executor().spawn(async move {
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
                }).detach();
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
                        .child(Input::new(&self.package_input).cleanable(true))
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
                        .items_center()
                        .gap_2()
                        .p_2()
                        .rounded_md()
                        .bg(rgb(0xFAF3E8))
                        .border_1()
                        .child(export_csv)
                        .child(helper_text(
                            "CSV export is only enabled when session state is Paused or Stopped.",
                        )),
                ),
            panel_width,
        )
    }

    fn can_export_csv(&self) -> bool {
        matches!(self.state, SessionState::Paused | SessionState::Stopped)
    }

    fn render_adb_part(&self, panel_width: f32) -> impl IntoElement {
        let runtime = Arc::clone(&self.runtime);
        let detect = Button::new("detect-devices")
            .primary()
            .label("Detect ADB Devices")
            .on_click(move |_, _, _| runtime.request_refresh_devices());

        let content = if self.detected_devices.is_empty() {
            div()
                .flex()
                .flex_col()
                .gap_4()
                .child(detect)
                .child(helper_text(
                    "No ADB devices listed yet. Detect devices first, then choose USB or Wireless.",
                ))
                .child(helper_text(
                    "Wireless connection requires the PC and device to be on the same LAN.",
                ))
        } else {
            div()
                .flex()
                .flex_col()
                .gap_4()
                .child(detect)
                .child(helper_text(
                    "Wireless connection requires the PC and device to be on the same LAN.",
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
        let can_connect = self.state.allows(ControlCommand::Connect);
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
}

impl Render for PerfDroidDemo {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.drain_events();
        window.request_animation_frame();
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
            .child(chart_section(self.render_cpu_clock_chart(chart_width)))
            .child(div().h(px(12.0)))
            .child(chart_section(self.render_fps_chart(chart_width)))
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

fn ensure_csv_extension(path: PathBuf) -> PathBuf {
    if path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("csv"))
    {
        path
    } else {
        path.with_extension("csv")
    }
}

fn stable_u64(value: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}
