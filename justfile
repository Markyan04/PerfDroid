set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

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
    mkdir -p dist

clean-dist:
    rm -rf dist

# Package for Linux, includes adb/linux/adb in release
package-linux: prepare-dist
    pkg_dir="dist/{{app_name}}-{{version}}-linux-x86_64"; \
    rm -rf "$pkg_dir" "dist/{{app_name}}-{{version}}-linux-x86_64.tar.gz"; \
    cargo build --release -p {{app_crate}} --target x86_64-unknown-linux-gnu; \
    mkdir -p "$pkg_dir/adb"; \
    cp target/x86_64-unknown-linux-gnu/release/{{app_crate}} "$pkg_dir/{{app_name}}"; \
    cp adb/linux/adb "$pkg_dir/adb/adb"; \
    chmod +x "$pkg_dir/{{app_name}}" "$pkg_dir/adb/adb"; \
    tar -C dist -czf "dist/{{app_name}}-{{version}}-linux-x86_64.tar.gz" "{{app_name}}-{{version}}-linux-x86_64"

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
    pkg_dir="dist/{{app_name}}-{{version}}-windows-x86_64"; \
    rm -rf "$pkg_dir" "dist/{{app_name}}-{{version}}-windows-x86_64.zip"; \
    cargo build --release -p {{app_crate}} --target x86_64-pc-windows-gnu; \
    mkdir -p "$pkg_dir/adb"; \
    cp target/x86_64-pc-windows-gnu/release/{{app_crate}}.exe "$pkg_dir/{{app_name}}.exe"; \
    cp adb/win/adb.exe "$pkg_dir/adb/adb.exe"; \
    cp adb/win/AdbWinApi.dll "$pkg_dir/adb/AdbWinApi.dll"; \
    cp adb/win/AdbWinUsbApi.dll "$pkg_dir/adb/AdbWinUsbApi.dll"; \
    (cd dist && zip -r "{{app_name}}-{{version}}-windows-x86_64.zip" "{{app_name}}-{{version}}-windows-x86_64")

# Build all platform packages (requires all targets/toolchains available)
package-all: clean-dist package-linux package-macos package-windows
