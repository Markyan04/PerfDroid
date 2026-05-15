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

mkdir -p "$dist_dir"
rm -rf "$pkg_dir" "$tar_path"

cargo build --release -p "$app_crate" --target x86_64-unknown-linux-gnu

mkdir -p "$pkg_dir/adb"
cp "$target_bin" "$pkg_dir/$app_name"
cp "$repo_root/adb/linux/adb" "$pkg_dir/adb/adb"
chmod +x "$pkg_dir/$app_name" "$pkg_dir/adb/adb"
tar -C "$dist_dir" -czf "$tar_path" "$package_name"

echo "Created tar.gz package: $tar_path"
