use adb_client::server::ADBServer;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::{fs, os::unix::fs::PermissionsExt};

const ADB_SERVER_PORT: u16 = 5037;

/// Returns the host-specific bundled `adb` path relative to the app root.
pub fn bundled_adb_relative_path() -> PathBuf {
    PathBuf::from("adb")
        .join(adb_platform_dir())
        .join(adb_binary_name())
}

/// Returns the runtime root that contains the bundled `adb/` directory.
///
/// Packaged builds should resolve `adb` relative to the executable location.
/// Local development still falls back to the source workspace root.
pub fn workspace_root() -> PathBuf {
    let relative_adb_path = bundled_adb_relative_path();
    candidate_roots()
        .into_iter()
        .find(|root| root.join(&relative_adb_path).exists())
        .unwrap_or_else(source_workspace_root)
}

/// Returns the host-specific directory that contains the bundled `adb` binary.
pub fn workspace_adb_dir() -> PathBuf {
    workspace_root().join("adb").join(adb_platform_dir())
}

/// Returns the host-specific bundled `adb` executable path.
pub fn workspace_adb_path() -> PathBuf {
    let adb_path = workspace_root().join(bundled_adb_relative_path());
    if let Err(err) = ensure_bundled_adb_permissions(&adb_path) {
        eprintln!(
            "warning: failed to ensure executable permissions for `{}`: {err}",
            adb_path.display()
        );
    }
    adb_path
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

fn candidate_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    if let Ok(exe_path) = std::env::current_exe()
        && let Some(exe_dir) = exe_path.parent()
    {
        push_ancestors(exe_dir, &mut roots);
    }

    if let Ok(current_dir) = std::env::current_dir() {
        push_ancestors(&current_dir, &mut roots);
    }

    roots.push(source_workspace_root());
    roots
}

fn push_ancestors(path: &Path, roots: &mut Vec<PathBuf>) {
    for ancestor in path.ancestors() {
        let candidate = ancestor.to_path_buf();
        if !roots.contains(&candidate) {
            roots.push(candidate);
        }
    }
}

fn source_workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("pdcore should live in <workspace>/crates/pdcore")
        .to_path_buf()
}

#[cfg(unix)]
fn ensure_bundled_adb_permissions(adb_path: &Path) -> std::io::Result<()> {
    let metadata = fs::metadata(adb_path)?;
    let mode = metadata.permissions().mode();
    let executable_mode = mode | 0o111;
    if mode != executable_mode {
        let mut permissions = metadata.permissions();
        permissions.set_mode(executable_mode);
        fs::set_permissions(adb_path, permissions)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_bundled_adb_permissions(_adb_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_adb_path_ends_with_expected_host_binary() {
        assert!(workspace_adb_path().ends_with(bundled_adb_relative_path()));
    }

    #[test]
    fn workspace_root_contains_bundled_adb_directory() {
        assert_eq!(workspace_adb_dir(), workspace_root().join("adb").join(adb_platform_dir()));
    }

    #[test]
    fn bundled_adb_relative_path_is_stable() {
        let expected = Path::new("adb")
            .join(adb_platform_dir())
            .join(adb_binary_name());
        assert_eq!(bundled_adb_relative_path(), expected);
    }
}
