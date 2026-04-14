use adb_client::server::ADBServer;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::{Path, PathBuf};

const ADB_SERVER_PORT: u16 = 5037;

/// Returns the workspace root that contains the bundled `adb/` directory.
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("pdcore should live in <workspace>/crates/pdcore")
        .to_path_buf()
}

/// Returns the host-specific directory that contains the bundled `adb` binary.
pub fn workspace_adb_dir() -> PathBuf {
    workspace_root().join("adb").join(adb_platform_dir())
}

/// Returns the host-specific bundled `adb` executable path.
pub fn workspace_adb_path() -> PathBuf {
    workspace_adb_dir().join(adb_binary_name())
}

/// Creates an [`ADBServer`] configured to start from the bundled workspace-local `adb`.
pub fn workspace_adb_server() -> ADBServer {
    ADBServer::new_from_path(
        SocketAddrV4::new(Ipv4Addr::LOCALHOST, ADB_SERVER_PORT),
        Some(workspace_adb_path().to_string_lossy().into_owned()),
    )
}

#[cfg(target_os = "windows")]
fn adb_platform_dir() -> &'static str {
    "win"
}

#[cfg(target_os = "macos")]
fn adb_platform_dir() -> &'static str {
    "mac"
}

#[cfg(target_os = "linux")]
fn adb_platform_dir() -> &'static str {
    "linux"
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn adb_platform_dir() -> &'static str {
    panic!("PerfDroid does not bundle adb for this host OS");
}

#[cfg(target_os = "windows")]
fn adb_binary_name() -> &'static str {
    "adb.exe"
}

#[cfg(not(target_os = "windows"))]
fn adb_binary_name() -> &'static str {
    "adb"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_adb_path_ends_with_expected_host_binary() {
        let expected_suffix = Path::new("adb")
            .join(adb_platform_dir())
            .join(adb_binary_name());
        assert!(workspace_adb_path().ends_with(expected_suffix));
    }

    #[test]
    fn workspace_root_contains_bundled_adb_directory() {
        assert_eq!(workspace_adb_dir(), workspace_root().join("adb").join(adb_platform_dir()));
    }
}
