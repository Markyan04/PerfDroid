use adb_client::{ADBDeviceExt, server::ADBServer};
use std::process::Command;

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
    let mut server = ADBServer::default();
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
    let connect_output = run_adb(&["connect", &wifi_serial])?;
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
    if let Some(address) = extract_ip_address(&route) {
        return Ok(address);
    }

    let wlan0 = run_adb(&["-s", serial, "shell", "ip", "-f", "inet", "addr", "show", "wlan0"])?;
    if let Some(address) = extract_ip_address(&wlan0) {
        return Ok(address);
    }

    Err(format!(
        "failed to determine WiFi IP for `{serial}` before enabling wireless ADB. Ensure the device is still visible in `adb devices`, WiFi is enabled, and the phone is on the same LAN as this computer."
    ))
}

fn extract_ip_address(text: &str) -> Option<String> {
    for token in text.split(|ch: char| ch.is_whitespace() || ch == '/') {
        let candidate = token.trim();
        if is_ipv4_address(candidate) {
            return Some(candidate.to_string());
        }
    }

    None
}

fn is_ipv4_address(value: &str) -> bool {
    let octets = value
        .split('.')
        .map(|part| part.parse::<u8>())
        .collect::<Result<Vec<_>, _>>();

    matches!(octets, Ok(parts) if parts.len() == 4)
}

fn run_adb(args: &[&str]) -> Result<String, String> {
    let output = Command::new("adb")
        .args(args)
        .output()
        .map_err(|err| format!("failed to launch `adb {}`: {err}", args.join(" ")))?;

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
