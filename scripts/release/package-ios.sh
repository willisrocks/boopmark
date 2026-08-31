#!/usr/bin/env bash
set -euo pipefail

version="${1:?usage: package-ios.sh VERSION [OUTPUT_DIR]}"
output_dir="${2:-dist}"
repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
ios_dir="$repo_root/mobile/ios"
build_root="$repo_root/build/release-ios"

if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "invalid release version: $version" >&2
  exit 1
fi

mkdir -p "$repo_root/$output_dir"
rm -rf "$build_root"
(cd "$ios_dir" && ./generate-xcodeproj.sh)

xcodebuild -project "$ios_dir/Boopmark.xcodeproj" -scheme Boopmark \
  -configuration Release -sdk iphoneos -destination 'generic/platform=iOS' \
  -derivedDataPath "$build_root/device" CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO build

device_app="$build_root/device/Build/Products/Release-iphoneos/Boopmark.app"
test -d "$device_app/PlugIns/Boopmark Share Extension.appex"
test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$device_app/Info.plist")" = "$version"
payload_root="$build_root/ipa"
mkdir -p "$payload_root/Payload"
ditto "$device_app" "$payload_root/Payload/Boopmark.app"
(cd "$payload_root" && zip -X -q -r "$repo_root/$output_dir/boopmark-ios-unsigned-$version.ipa" Payload)

xcodebuild -project "$ios_dir/Boopmark.xcodeproj" -scheme Boopmark \
  -configuration Release -sdk iphonesimulator -destination 'generic/platform=iOS Simulator' \
  -derivedDataPath "$build_root/simulator" CODE_SIGNING_ALLOWED=NO build

simulator_app="$build_root/simulator/Build/Products/Release-iphonesimulator/Boopmark.app"
test -d "$simulator_app/PlugIns/Boopmark Share Extension.appex"
test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$simulator_app/Info.plist")" = "$version"
ditto -c -k --sequesterRsrc --keepParent \
  "$simulator_app" "$repo_root/$output_dir/boopmark-ios-simulator-$version.zip"

printf '%s\n' \
  "$repo_root/$output_dir/boopmark-ios-unsigned-$version.ipa" \
  "$repo_root/$output_dir/boopmark-ios-simulator-$version.zip"
