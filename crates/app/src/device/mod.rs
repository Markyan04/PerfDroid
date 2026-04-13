use adb_client::{ADBDeviceExt, server::ADBServer};

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
