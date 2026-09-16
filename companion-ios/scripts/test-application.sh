#!/usr/bin/env bash
set -euo pipefail
repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_dir/companion-ios"
result="$repo_dir/.artifacts/ios-application.xcresult"
rm -rf "$result"
mkdir -p "$(dirname "$result")"
xcodegen generate
xcodebuild test -quiet \
  -project OBCCompanion.xcodeproj -scheme OBCCompanion \
  -destination "platform=iOS Simulator,name=${OBC_TEST_DEVICE:-iPhone 17 Pro}" \
  -derivedDataPath DerivedData -resultBundlePath "$result" \
  -skip-testing:OBCCompanionUITests/WebsiteScreenshotTests \
  -parallel-testing-enabled NO CODE_SIGNING_ALLOWED=NO
