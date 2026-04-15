set shell := ["bash", "-eu", "-o", "pipefail", "-c"]
set windows-shell := ["powershell.exe", "-NoLogo", "-NoProfile", "-Command"]

app_name := "perfdroid"
app_crate := "app"
version := "0.1.0"

default:
    just --list

# Development basics
build:
    cargo build --workspace

check:
    cargo check --workspace

test:
    cargo test --workspace

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

clippy:
    cargo clippy --workspace --all-targets -- -D warnings

run:
    cargo run -p {{app_crate}}

clean:
    cargo clean

# Optional: install targets needed for cross-platform packaging
install-targets:
    rustup target add x86_64-pc-windows-gnu x86_64-unknown-linux-gnu x86_64-apple-darwin

# Internal helpers
prepare-dist:
    {{ if os_family() == "windows" { "if (-not (Test-Path 'dist')) { New-Item -ItemType Directory -Path 'dist' | Out-Null }" } else { "mkdir -p dist" } }}

clean-dist:
    {{ if os_family() == "windows" { "if (Test-Path 'dist') { Remove-Item -LiteralPath 'dist' -Recurse -Force }" } else { "rm -rf dist" } }}

# Package for Linux, includes adb/linux/adb in release
package-linux: prepare-dist
    ./scripts/package-linux.sh '{{app_name}}' '{{version}}' '{{app_crate}}'

# Package for macOS, includes adb/mac/adb in release
package-macos: prepare-dist
    pkg_dir="dist/{{app_name}}-{{version}}-macos-x86_64"; \
    rm -rf "$pkg_dir" "dist/{{app_name}}-{{version}}-macos-x86_64.tar.gz"; \
    cargo build --release -p {{app_crate}} --target x86_64-apple-darwin; \
    mkdir -p "$pkg_dir/adb"; \
    cp target/x86_64-apple-darwin/release/{{app_crate}} "$pkg_dir/{{app_name}}"; \
    cp adb/mac/adb "$pkg_dir/adb/adb"; \
    chmod +x "$pkg_dir/{{app_name}}" "$pkg_dir/adb/adb"; \
    tar -C dist -czf "dist/{{app_name}}-{{version}}-macos-x86_64.tar.gz" "{{app_name}}-{{version}}-macos-x86_64"

# Package for Windows, includes adb/win binaries in release
package-windows: prepare-dist
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File '.\scripts\package-windows.ps1' -AppName '{{app_name}}' -Version '{{version}}' -AppCrate '{{app_crate}}'

# Build all platform packages (requires all targets/toolchains available)
package-all: clean-dist package-linux package-macos package-windows
