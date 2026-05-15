#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]]; then
    echo "usage: $0 <app-name> <version> <app-crate>" >&2
    exit 1
fi

app_name="$1"
version="$2"
app_crate="$3"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
dist_dir="$repo_root/dist"
package_name="$app_name-$version-linux-x86_64"
pkg_dir="$dist_dir/$package_name"
tar_path="$dist_dir/$package_name.tar.gz"
target_bin="$repo_root/target/x86_64-unknown-linux-gnu/release/$app_crate"
icon_dir="$repo_root/assets/icons"
desktop_entry_path="$pkg_dir/share/applications/$app_name.desktop"

mkdir -p "$dist_dir"
rm -rf "$pkg_dir" "$tar_path"

cargo build --release -p "$app_crate" --target x86_64-unknown-linux-gnu

mkdir -p "$pkg_dir/adb" "$pkg_dir/share/applications"
cp "$target_bin" "$pkg_dir/$app_name"
cp "$repo_root/adb/linux/adb" "$pkg_dir/adb/adb"
chmod +x "$pkg_dir/$app_name" "$pkg_dir/adb/adb"

cat > "$desktop_entry_path" <<EOF
[Desktop Entry]
Type=Application
Version=1.0
Name=PerfDroid
Comment=Android performance profiling desktop tool
Exec=$app_name
Icon=$app_name
Terminal=false
Categories=Development;Utility;
Keywords=android;adb;performance;profiling;
StartupWMClass=$app_name
EOF

for icon_size in 16 24 32 48 64 128 256 512 1024; do
    source_icon="$icon_dir/icon_${icon_size}.png"
    target_icon_dir="$pkg_dir/share/icons/hicolor/${icon_size}x${icon_size}/apps"

    if [[ ! -f "$source_icon" ]]; then
        echo "missing icon asset: $source_icon" >&2
        exit 1
    fi

    mkdir -p "$target_icon_dir"
    cp "$source_icon" "$target_icon_dir/$app_name.png"
done

tar -C "$dist_dir" -czf "$tar_path" "$package_name"

echo "Created tar.gz package: $tar_path"
