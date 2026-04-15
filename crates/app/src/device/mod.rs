use adb_client::ADBDeviceExt;
use pdcore::adb::{workspace_adb_path, workspace_adb_server};
use std::process::Command;
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionKind {
    Usb,
    Wifi,
    Unknown,
}

impl ConnectionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Usb => "USB",
            Self::Wifi => "WiFi",
            Self::Unknown => "Unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceDescriptor {
    pub serial: String,
    pub connection: ConnectionKind,
    pub model: String,
    pub android_version: String,
    pub soc_model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdbDetectedDevice {
    pub serial: String,
    pub connection: ConnectionKind,
    pub adb_state: String,
    pub model: String,
}

pub fn query_device_descriptor(serial: Option<&str>) -> Result<DeviceDescriptor, String> {
    let mut server = workspace_adb_server();
    let target_serial = serial.unwrap_or("default").to_string();
    let mut device = match serial {
        Some(serial) => server
            .get_device_by_name(serial)
            .map_err(|err| format!("failed to get adb device `{serial}`: {err}"))?,
        None => server
            .get_device()
            .map_err(|err| format!("failed to get adb device: {err}"))?,
    };

    let actual_serial = if serial.is_some() {
        target_serial
    } else {
        read_prop(&mut device, "ro.serialno").unwrap_or_else(|| "default".to_string())
    };

    Ok(DeviceDescriptor {
        connection: infer_connection_kind(&actual_serial),
        serial: actual_serial,
        model: read_prop(&mut device, "ro.product.model").unwrap_or_else(|| "Unknown".to_string()),
        android_version: read_prop(&mut device, "ro.build.version.release")
            .unwrap_or_else(|| "Unknown".to_string()),
        soc_model: read_prop(&mut device, "ro.soc.model")
            .or_else(|| read_prop(&mut device, "ro.board.platform"))
            .unwrap_or_else(|| "Unknown".to_string()),
    })
}

pub fn list_adb_devices() -> Result<Vec<AdbDetectedDevice>, String> {
    let output = run_adb(&["devices", "-l"])?;
    let mut devices = Vec::new();

    for line in output.lines().skip(1) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let mut parts = trimmed.split_whitespace();
        let Some(serial) = parts.next() else {
            continue;
        };
        let adb_state = parts.next().unwrap_or("unknown").to_string();
        let model = trimmed
            .split_whitespace()
            .find_map(|part| part.strip_prefix("model:"))
            .unwrap_or("Unknown")
            .replace('_', " ");

        let detected = AdbDetectedDevice {
            serial: serial.to_string(),
            connection: infer_connection_kind(serial),
            adb_state,
            model,
        };

        if detected.connection == ConnectionKind::Usb {
            devices.push(detected);
        }
    }

    Ok(devices)
}

pub fn connect_wireless(serial: &str) -> Result<(String, String), String> {
    if serial.contains(':') {
        let connect_output = connect_wireless_with_retry(serial)?;
        return Ok((
            serial.to_string(),
            format!(
                "Wireless device `{serial}` is reachable through ADB over WiFi. {}",
                connect_output.trim()
            )
            .trim()
            .to_string(),
        ));
    }

    let ip_address = query_wireless_ip(serial)?;
    let tcpip_output = run_adb(&["-s", serial, "tcpip", "5555"])?;
    thread::sleep(Duration::from_millis(1_200));
    let wifi_serial = format!("{ip_address}:5555");
    let connect_output = connect_wireless_with_retry(&wifi_serial)?;
    let message = format!(
        "Wireless ADB enabled for `{serial}` at `{wifi_serial}`. {} {}",
        tcpip_output.trim(),
        connect_output.trim()
    )
    .trim()
    .to_string();

    Ok((wifi_serial, message))
}

fn read_prop(device: &mut impl ADBDeviceExt, key: &str) -> Option<String> {
    let command = format!("getprop {key}");
    let mut out = Vec::with_capacity(64);
    let mut err = Vec::with_capacity(64);
    let status = device
        .shell_command(&command, Some(&mut out), Some(&mut err))
        .ok()?;

    if status.is_some_and(|code| code != 0) {
        return None;
    }

    let value = String::from_utf8_lossy(&out).trim().to_string();
    if value.is_empty() { None } else { Some(value) }
}

fn infer_connection_kind(serial: &str) -> ConnectionKind {
    if serial.contains(':') {
        ConnectionKind::Wifi
    } else if serial == "default" {
        ConnectionKind::Unknown
    } else {
        ConnectionKind::Usb
    }
}

fn query_wireless_ip(serial: &str) -> Result<String, String> {
    let mut failures = Vec::new();

    if let Some(address) = query_wireless_ip_from_command(
        serial,
        &["shell", "ip", "-f", "inet", "-o", "addr", "show", "up", "scope", "global"],
        extract_preferred_wireless_inet_ip,
        &mut failures,
    ) {
        return Ok(address);
    }

    if let Some(address) = query_wireless_ip_from_command(
        serial,
        &["shell", "ip", "-f", "inet", "addr", "show", "wlan0"],
        extract_inet_ip,
        &mut failures,
    ) {
        return Ok(address);
    }

    if let Some(address) = query_wireless_ip_from_command(
        serial,
        &["shell", "ip", "-f", "inet", "addr", "show", "wlan1"],
        extract_inet_ip,
        &mut failures,
    ) {
        return Ok(address);
    }

    if let Some(address) = query_wireless_ip_from_command(
        serial,
        &["shell", "ip", "-f", "inet", "addr", "show", "wifi0"],
        extract_inet_ip,
        &mut failures,
    ) {
        return Ok(address);
    }

    if let Some(address) = query_wireless_ip_from_command(
        serial,
        &["shell", "ip", "route"],
        extract_wireless_route_src_ip,
        &mut failures,
    ) {
        return Ok(address);
    }

    for property in ["dhcp.wlan0.ipaddress", "dhcp.wlan1.ipaddress", "dhcp.eth0.ipaddress"] {
        let command = ["shell", "getprop", property];
        if let Some(address) =
            query_wireless_ip_from_command(serial, &command, extract_first_ipv4, &mut failures)
        {
            return Ok(address);
        }
    }

    Err(format!(
        "failed to determine WiFi IP for `{serial}` before enabling wireless ADB. Ensure the device is still visible in `adb devices`, WiFi is enabled, and the phone is on the same LAN as this computer. Attempts: {}",
        failures.join(" | ")
    ))
}

fn connect_wireless_with_retry(wifi_serial: &str) -> Result<String, String> {
    const CONNECT_RETRIES: usize = 8;
    const CONNECT_RETRY_DELAY_MS: u64 = 900;
    let mut last_message = String::new();

    for attempt in 0..CONNECT_RETRIES {
        let _ = run_adb(&["disconnect", wifi_serial]);

        match run_adb(&["connect", wifi_serial]) {
            Ok(output) => {
                last_message = output;
                if is_adb_connect_success(&last_message) {
                    match probe_wireless_transport_ready(wifi_serial) {
                        Ok(()) => return Ok(last_message),
                        Err(err) => {
                            last_message = format!("{}; {}", last_message.trim(), err);
                        }
                    }
                }
            }
            Err(err) => {
                last_message = err;
            }
        }

        if attempt + 1 < CONNECT_RETRIES {
            thread::sleep(Duration::from_millis(CONNECT_RETRY_DELAY_MS));
        }
    }

    Err(format!(
        "failed to connect to `{wifi_serial}` after {CONNECT_RETRIES} attempts: {last_message}"
    ))
}

fn probe_wireless_transport_ready(serial: &str) -> Result<(), String> {
    const READY_RETRIES: usize = 12;
    const READY_RETRY_DELAY_MS: u64 = 350;
    let mut last_message = String::new();

    for attempt in 0..READY_RETRIES {
        match adb_target_state(serial) {
            Ok(state) if state == "device" => match query_device_descriptor(Some(serial)) {
                Ok(_) => return Ok(()),
                Err(err) => {
                    last_message = format!(
                        "wireless ADB state is `device` but the Rust ADB client cannot open `{serial}` yet: {err}"
                    );
                }
            },
            Ok(state) => {
                last_message = format!(
                    "ADB reported `{serial}` as `{state}` after connecting"
                );
            }
            Err(err) => last_message = err,
        }

        if attempt + 1 < READY_RETRIES {
            thread::sleep(Duration::from_millis(READY_RETRY_DELAY_MS));
        }
    }

    Err(format!("{last_message}; the wireless transport did not become ready in time"))
}

fn adb_target_state(serial: &str) -> Result<String, String> {
    let output = run_adb(&["-s", serial, "get-state"])?;
    Ok(output.trim().to_string())
}

fn query_wireless_ip_from_command(
    serial: &str,
    command: &[&str],
    parser: fn(&str) -> Option<String>,
    failures: &mut Vec<String>,
) -> Option<String> {
    let mut adb_args = Vec::with_capacity(command.len() + 2);
    adb_args.push("-s");
    adb_args.push(serial);
    adb_args.extend_from_slice(command);

    match run_adb(&adb_args) {
        Ok(output) => match parser(&output) {
            Some(address) => Some(address),
            None => {
                failures.push(format!(
                    "`adb {}` did not expose a usable IPv4 address",
                    adb_args.join(" ")
                ));
                None
            }
        },
        Err(err) => {
            failures.push(err);
            None
        }
    }
}

fn is_ipv4_address(value: &str) -> bool {
    parse_ipv4_address(value).is_some()
}

fn parse_ipv4_address(value: &str) -> Option<[u8; 4]> {
    let octets = value
        .split('.')
        .map(|part| part.parse::<u8>())
        .collect::<Result<Vec<_>, _>>()
        .ok()?;

    if octets.len() != 4 {
        return None;
    }

    Some([octets[0], octets[1], octets[2], octets[3]])
}

#[cfg(test)]
fn extract_route_src_ip(text: &str) -> Option<String> {
    for line in text.lines() {
        let tokens = line.split_whitespace().collect::<Vec<_>>();
        for window in tokens.windows(2) {
            if window[0] == "src" && is_ipv4_address(window[1]) {
                return Some(window[1].to_string());
            }
        }
    }

    None
}

fn extract_wireless_route_src_ip(text: &str) -> Option<String> {
    for line in text.lines() {
        let mut route_interface = None;
        let mut route_src = None;
        let tokens = line.split_whitespace().collect::<Vec<_>>();

        for window in tokens.windows(2) {
            if window[0] == "dev" {
                route_interface = Some(window[1]);
            } else if window[0] == "src" && is_ipv4_address(window[1]) {
                route_src = Some(window[1]);
            }
        }

        if let (Some(interface), Some(address)) = (route_interface, route_src)
            && is_wireless_interface(interface)
        {
            return Some(address.to_string());
        }
    }

    None
}

fn extract_inet_ip(text: &str) -> Option<String> {
    for line in text.lines() {
        let tokens = line.split_whitespace().collect::<Vec<_>>();
        for window in tokens.windows(2) {
            if window[0] == "inet" {
                let candidate = window[1].split('/').next().unwrap_or("");
                if is_ipv4_address(candidate) {
                    return Some(candidate.to_string());
                }
            }
        }
    }

    None
}

fn extract_preferred_wireless_inet_ip(text: &str) -> Option<String> {
    for line in text.lines() {
        let tokens = line.split_whitespace().collect::<Vec<_>>();
        if tokens.len() < 4 {
            continue;
        }

        let interface = tokens[1];
        if !is_wireless_interface(interface) {
            continue;
        }

        for window in tokens.windows(2) {
            if window[0] == "inet" {
                let candidate = window[1].split('/').next().unwrap_or("");
                if is_ipv4_address(candidate) {
                    return Some(candidate.to_string());
                }
            }
        }
    }

    None
}

fn extract_first_ipv4(text: &str) -> Option<String> {
    text.split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .find(|token| is_ipv4_address(token))
        .map(str::to_string)
}

fn is_wireless_interface(interface: &str) -> bool {
    let normalized = interface.trim_end_matches(':').to_ascii_lowercase();
    normalized.starts_with("wlan")
        || normalized.starts_with("wifi")
        || normalized.starts_with("wl")
        || normalized.starts_with("ap")
        || normalized.starts_with("p2p")
}

fn is_adb_connect_success(output: &str) -> bool {
    let normalized = output.to_ascii_lowercase();
    normalized.contains("connected to") || normalized.contains("already connected to")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_parser_uses_src_ip_instead_of_network_prefix() {
        let route = "10.0.0.0/24 dev wlan0 proto kernel scope link src 10.0.0.21\n";
        assert_eq!(extract_route_src_ip(route).as_deref(), Some("10.0.0.21"));
    }

    #[test]
    fn wireless_route_parser_ignores_cellular_and_prefers_wifi() {
        let route = "\
default via 10.0.252.217 dev rmnet_data0 proto static src 10.0.252.219
192.168.1.0/24 dev wlan0 proto kernel scope link src 192.168.1.48";
        assert_eq!(
            extract_wireless_route_src_ip(route).as_deref(),
            Some("192.168.1.48")
        );
    }

    #[test]
    fn inet_parser_extracts_host_address() {
        let addr = "3: wlan0    inet 192.168.1.48/24 brd 192.168.1.255 scope global wlan0";
        assert_eq!(extract_inet_ip(addr).as_deref(), Some("192.168.1.48"));
    }

    #[test]
    fn preferred_inet_parser_ignores_non_wifi_interfaces() {
        let addr = "\
4: rmnet_data0    inet 10.0.252.219/29 brd 10.0.252.223 scope global rmnet_data0
5: wlan0    inet 192.168.1.48/24 brd 192.168.1.255 scope global wlan0";
        assert_eq!(
            extract_preferred_wireless_inet_ip(addr).as_deref(),
            Some("192.168.1.48")
        );
    }

    #[test]
    fn first_ipv4_parser_handles_property_and_ifconfig_output() {
        assert_eq!(
            extract_first_ipv4("192.168.1.48").as_deref(),
            Some("192.168.1.48")
        );
        assert_eq!(
            extract_first_ipv4("inet addr:192.168.1.48  Bcast:192.168.1.255  Mask:255.255.255.0")
                .as_deref(),
            Some("192.168.1.48")
        );
    }

    #[test]
    fn usb_only_device_listing_filters_out_wifi_entries() {
        let devices = "List of devices attached\n192.168.1.48:5555 device product:demo model:Demo_Phone\nFA79X1A00000 device product:demo model:Demo_Phone\n";
        let parsed = devices
            .lines()
            .skip(1)
            .filter_map(|line| {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    return None;
                }

                let mut parts = trimmed.split_whitespace();
                let serial = parts.next()?;
                let adb_state = parts.next().unwrap_or("unknown").to_string();
                let model = trimmed
                    .split_whitespace()
                    .find_map(|part| part.strip_prefix("model:"))
                    .unwrap_or("Unknown")
                    .replace('_', " ");

                let detected = AdbDetectedDevice {
                    serial: serial.to_string(),
                    connection: infer_connection_kind(serial),
                    adb_state,
                    model,
                };

                (detected.connection == ConnectionKind::Usb).then_some(detected)
            })
            .collect::<Vec<_>>();

        assert_eq!(parsed.len(), 1);
        assert_eq!(
            parsed[0].serial,
            "FA79X1A00000"
        );
    }

    #[test]
    fn connect_success_recognizes_adb_connect_messages() {
        assert!(is_adb_connect_success("connected to 192.168.1.48:5555"));
        assert!(is_adb_connect_success(
            "already connected to 192.168.1.48:5555"
        ));
        assert!(!is_adb_connect_success(
            "failed to connect to 192.168.1.48:5555"
        ));
    }
}

fn run_adb(args: &[&str]) -> Result<String, String> {
    let adb_path = workspace_adb_path();
    let output = Command::new(&adb_path).args(args).output().map_err(|err| {
        format!(
            "failed to launch `{}` for `adb {}`: {err}",
            adb_path.display(),
            args.join(" ")
        )
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if output.status.success() {
        if stdout.is_empty() {
            Ok(stderr)
        } else {
            Ok(stdout)
        }
    } else {
        let detail = if stderr.is_empty() { stdout } else { stderr };
        Err(format!("`adb {}` failed: {detail}", args.join(" ")))
    }
}
