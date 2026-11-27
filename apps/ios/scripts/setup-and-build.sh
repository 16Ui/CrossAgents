#!/usr/bin/env bash
set -euo pipefail

# One-shot check + Rust XCFramework build + Xcode project generation + Simulator verification build.
# Run on macOS after installing Xcode from the Mac App Store.

ROOT_DIR="$(cd "$(dirname "$0")/../../.." && pwd)"
IOS_DIR="$ROOT_DIR/apps/ios"

# Ensure Homebrew & Cargo bins are in PATH when invoked from non-login shells.
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"

echo "==> 1/4 Checking Xcode..."
if [[ ! -d "/Applications/Xcode.app" ]]; then
  echo "ERROR: /Applications/Xcode.app not found." >&2
  echo "Please install Xcode from the Mac App Store first, then run:" >&2
  echo "  sudo xcode-select -s /Applications/Xcode.app/Contents/Developer" >&2
  echo "  sudo xcodebuild -license accept" >&2
  exit 1
fi

DEV_DIR="$(xcode-select -p 2>/dev/null || true)"
if [[ "$DEV_DIR" != *"/Xcode.app/"* ]]; then
  echo "ERROR: Active developer directory is '$DEV_DIR' (not Xcode.app)." >&2
  echo "Run: sudo xcode-select -s /Applications/Xcode.app/Contents/Developer" >&2
  exit 1
fi
xcodebuild -version

echo "==> 2/4 Checking Rust & XcodeGen..."
if ! command -v rustup >/dev/null 2>&1 || ! command -v cargo >/dev/null 2>&1; then
  echo "ERROR: rustup/cargo not found." >&2
  echo "Install Rust with:" >&2
  echo "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y" >&2
  echo "Then re-run this script." >&2
  exit 1
fi

if ! command -v xcodegen >/dev/null 2>&1; then
  echo "ERROR: xcodegen not found." >&2
  echo "Install it with: brew install xcodegen" >&2
  exit 1
fi

echo "==> 3/4 Building Rust CrossAgentsLink.xcframework..."
bash "$IOS_DIR/scripts/build-rust.sh"

echo "==> 4/4 Generating CrossAgents.xcodeproj and verifying iOS Simulator build..."
(cd "$IOS_DIR" && xcodegen generate)
xcodebuild \
  -project "$IOS_DIR/CrossAgents.xcodeproj" \
  -scheme CrossAgents \
  -configuration Debug \
  -destination "generic/platform=iOS Simulator" \
  CODE_SIGNING_ALLOWED=NO \
  build

echo ""
echo "==================================================================="
echo "CrossAgents iOS build succeeded!"
echo "Next steps:"
echo "  open $IOS_DIR/CrossAgents.xcodeproj"
echo "  1) Select target 'CrossAgents' -> Signing & Capabilities -> pick your Apple ID Team"
echo "     (if bundle ID conflicts, change 'dev.crossagents.ios' to e.g. 'dev.crossagents.ios.yourname')"
echo "  2) Pick an iPhone Simulator or your USB-connected iPhone and press Cmd+R!"
echo "==================================================================="
