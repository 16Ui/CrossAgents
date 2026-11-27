#!/usr/bin/env bash
set -euo pipefail

# Build Rust `crossagents-link-ffi` static libraries for iOS device + Apple Silicon iOS Simulator
# and bundle them into `apps/ios/Frameworks/CrossAgentsLink.xcframework`.

ROOT_DIR="$(cd "$(dirname "$0")/../../.." && pwd)"
IOS_DIR="$ROOT_DIR/apps/ios"
HEADERS_DIR="$ROOT_DIR/crates/link-ffi/include"
OUT_XCFRAMEWORK="$IOS_DIR/Frameworks/CrossAgentsLink.xcframework"

PROFILE="release"
CARGO_PROFILE_FLAG="--release"
if [[ "${1:-}" == "--debug" ]]; then
  PROFILE="debug"
  CARGO_PROFILE_FLAG=""
fi

export IPHONEOS_DEPLOYMENT_TARGET="17.0"

echo "==> Checking Rust iOS targets..."
rustup target add aarch64-apple-ios aarch64-apple-ios-sim >/dev/null

echo "==> Building crossagents-link-ffi ($PROFILE) for aarch64-apple-ios (iPhone)..."
(cd "$ROOT_DIR" && cargo build $CARGO_PROFILE_FLAG -p crossagents-link-ffi --target aarch64-apple-ios)

echo "==> Building crossagents-link-ffi ($PROFILE) for aarch64-apple-ios-sim (Simulator)..."
(cd "$ROOT_DIR" && cargo build $CARGO_PROFILE_FLAG -p crossagents-link-ffi --target aarch64-apple-ios-sim)

DEVICE_LIB="$ROOT_DIR/target/aarch64-apple-ios/$PROFILE/libcrossagents_link_ffi.a"
SIM_LIB="$ROOT_DIR/target/aarch64-apple-ios-sim/$PROFILE/libcrossagents_link_ffi.a"

if [[ ! -f "$DEVICE_LIB" || ! -f "$SIM_LIB" ]]; then
  echo "ERROR: Built static libraries not found." >&2
  exit 1
fi

echo "==> Packaging CrossAgentsLink.xcframework..."
rm -rf "$OUT_XCFRAMEWORK"
mkdir -p "$IOS_DIR/Frameworks"

xcodebuild -create-xcframework \
  -library "$DEVICE_LIB" -headers "$HEADERS_DIR" \
  -library "$SIM_LIB" -headers "$HEADERS_DIR" \
  -output "$OUT_XCFRAMEWORK"

echo "==> Ready: $OUT_XCFRAMEWORK"
