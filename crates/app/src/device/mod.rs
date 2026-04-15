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

        devices.push(AdbDetectedDevice {
            serial: serial.to_string(),
            connection: infer_connection_kind(serial),
            adb_state,
            model,
        });
    }

    Ok(devices)
}

pub fn connect_wireless(serial: &str) -> Result<(String, String), String> {
    if serial.contains(':') {
        return Ok((
            serial.to_string(),
            format!("Wireless device `{serial}` is already reachable through ADB over WiFi."),
        ));
    }

    let ip_address = query_wireless_ip(serial)?;
    let tcpip_output = run_adb(&["-s", serial, "tcpip", "5555"])?;
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
    let route = run_adb(&["-s", serial, "shell", "ip", "route"])?;
    if let Some(address) = extract_route_src_ip(&route) {
        return Ok(address);
    }

    let addresses = run_adb(&[
        "-s", serial, "shell", "ip", "-f", "inet", "-o", "addr", "show", "up", "scope", "global",
    ])?;
    if let Some(address) = extract_inet_ip(&addresses) {
        return Ok(address);
    }

    let wlan0 = run_adb(&[
        "-s", serial, "shell", "ip", "-f", "inet", "addr", "show", "wlan0",
    ])?;
    if let Some(address) = extract_inet_ip(&wlan0) {
        return Ok(address);
    }

    Err(format!(
        "failed to determine WiFi IP for `{serial}` before enabling wireless ADB. Ensure the device is still visible in `adb devices`, WiFi is enabled, and the phone is on the same LAN as this computer."
    ))
}

fn connect_wireless_with_retry(wifi_serial: &str) -> Result<String, String> {
    const CONNECT_RETRIES: usize = 5;
    const CONNECT_RETRY_DELAY_MS: u64 = 300;
    let mut last_message = String::new();

    for attempt in 0..CONNECT_RETRIES {
        match run_adb(&["connect", wifi_serial]) {
            Ok(output) => {
                last_message = output;
                if is_adb_connect_success(&last_message) {
                    return Ok(last_message);
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
    fn inet_parser_extracts_host_address() {
        let addr = "3: wlan0    inet 192.168.1.48/24 brd 192.168.1.255 scope global wlan0";
        assert_eq!(extract_inet_ip(addr).as_deref(), Some("192.168.1.48"));
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
