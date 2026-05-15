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
host_triple="$(rustc -vV | sed -n 's/^host: //p')"
target_triple="${MACOS_TARGET:-$host_triple}"

case "$target_triple" in
    aarch64-apple-darwin)
        arch_suffix="arm64"
        ;;
    x86_64-apple-darwin)
        arch_suffix="x86_64"
        ;;
    *)
        echo "unsupported macOS target triple: $target_triple" >&2
        echo "set MACOS_TARGET to a supported Apple target (aarch64-apple-darwin or x86_64-apple-darwin)" >&2
        exit 1
        ;;
esac

bundle_name="$app_name.app"
package_name="$app_name-$version-macos-$arch_suffix"
package_dir="$dist_dir/$package_name"
bundle_dir="$package_dir/$bundle_name"
contents_dir="$bundle_dir/Contents"
macos_dir="$contents_dir/MacOS"
resources_dir="$contents_dir/Resources"
archive_path="$dist_dir/$package_name.tar.gz"
target_bin="$repo_root/target/$target_triple/release/$app_crate"
icon_source="$repo_root/assets/icons/app.icns"
bundle_icon_name="$app_name.icns"

mkdir -p "$dist_dir"
rm -rf "$package_dir" "$archive_path"

cargo build --release -p "$app_crate" --target "$target_triple"

mkdir -p "$macos_dir" "$resources_dir/adb/mac"

cp "$target_bin" "$macos_dir/$app_name"
cp "$repo_root/adb/mac/adb" "$resources_dir/adb/mac/adb"
cp "$icon_source" "$resources_dir/$bundle_icon_name"
chmod +x "$macos_dir/$app_name" "$resources_dir/adb/mac/adb"

cat > "$contents_dir/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "https://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleExecutable</key>
    <string>$app_name</string>
    <key>CFBundleDisplayName</key>
    <string>PerfDroid</string>
    <key>CFBundleIdentifier</key>
    <string>com.perfdroid.$app_name</string>
    <key>CFBundleIconFile</key>
    <string>$bundle_icon_name</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>PerfDroid</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$version</string>
    <key>CFBundleVersion</key>
    <string>$version</string>
    <key>LSMinimumSystemVersion</key>
    <string>12.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
EOF

tar -C "$dist_dir" -czf "$archive_path" "$package_name"

echo "Created macOS app bundle: $bundle_dir"
echo "Created tar.gz package: $archive_path"
