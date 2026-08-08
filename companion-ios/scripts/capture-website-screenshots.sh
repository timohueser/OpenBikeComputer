#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
companion_dir="$(cd "$script_dir/.." && pwd)"
repo_dir="$(cd "$companion_dir/.." && pwd)"
output_dir="$repo_dir/docs/assets/companion"
device_name="${OBC_SCREENSHOT_DEVICE:-iPhone 17 Pro}"
derived_data="${OBC_DERIVED_DATA_PATH:-$companion_dir/DerivedData}"
mode="update"

if [[ "${1:-}" == "--check" ]]; then
  mode="check"
  shift
fi
if [[ $# -ne 0 ]]; then
  echo "usage: $0 [--check]" >&2
  exit 2
fi

for tool in xcodegen xcodebuild xcrun python3 cwebp dwebp; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "missing $tool (install local prerequisites with: brew install xcodegen webp)" >&2
    exit 1
  fi
done

# One Grimsel fixture family owns all three surfaces: full planned route on upload, then the shorter
# browser replay as the finished ride. Update mode refreshes the derived SwiftPM resources; CI
# check mode reports drift without touching the tree.
if [[ "$mode" == "check" ]]; then
  python3 "$script_dir/generate-website-fixture.py" --check
else
  python3 "$script_dir/generate-website-fixture.py"
fi

work_dir="$(mktemp -d "${TMPDIR:-/tmp}/obc-companion-shots.XXXXXX")"
simulator_id=""
cleanup() {
  if [[ -n "$simulator_id" ]]; then
    xcrun simctl status_bar "$simulator_id" clear >/dev/null 2>&1 || true
  fi
  rm -rf "$work_dir"
}
trap cleanup EXIT

simulator_id="$(xcrun simctl list devices available -j | OBC_CAPTURE_DEVICE_NAME="$device_name" python3 -c '
import json, os, sys
data = json.load(sys.stdin)
wanted = os.environ["OBC_CAPTURE_DEVICE_NAME"]
for runtime, devices in sorted(data["devices"].items(), reverse=True):
    if "iOS" not in runtime:
        continue
    for device in devices:
        if device.get("isAvailable") and device.get("name") == wanted:
            print(device["udid"])
            raise SystemExit(0)
raise SystemExit(f"no available iOS simulator named {wanted!r}")
')"

xcrun simctl boot "$simulator_id" >/dev/null 2>&1 || true
xcrun simctl bootstatus "$simulator_id" -b
xcrun simctl ui "$simulator_id" appearance light
xcrun simctl ui "$simulator_id" content_size large
xcrun simctl status_bar "$simulator_id" override \
  --time 9:41 --dataNetwork wifi --wifiMode active --wifiBars 3 \
  --cellularMode active --cellularBars 4 --operatorName '' \
  --batteryState charged --batteryLevel 100

(
  cd "$companion_dir"
  xcodegen generate
  xcodebuild test \
    -quiet \
    -project OBCCompanion.xcodeproj \
    -scheme OBCCompanion \
    -destination "platform=iOS Simulator,id=$simulator_id" \
    -derivedDataPath "$derived_data" \
    -resultBundlePath "$work_dir/WebsiteScreenshots.xcresult" \
    -only-testing:OBCCompanionUITests/WebsiteScreenshotTests \
    CODE_SIGNING_ALLOWED=NO
)

xcrun xcresulttool export attachments \
  --path "$work_dir/WebsiteScreenshots.xcresult" \
  --output-path "$work_dir/attachments"

python3 - "$work_dir/attachments" "$work_dir/rendered" <<'PY'
import json
import pathlib
import shutil
import subprocess
import sys

attachments_dir = pathlib.Path(sys.argv[1])
rendered_dir = pathlib.Path(sys.argv[2])
rendered_dir.mkdir()

wanted = {
    "website-route-imported": "route-imported.webp",
    "website-route-on-device": "route-on-device.webp",
    "website-rides-before-sync": "rides-before-sync.webp",
    "website-rides-synced": "rides-synced.webp",
    "website-ride-detail": "ride-detail.webp",
}
found = {}
manifest = json.loads((attachments_dir / "manifest.json").read_text())
for test in manifest:
    for attachment in test.get("attachments", []):
        suggested = pathlib.Path(attachment["suggestedHumanReadableName"]).stem
        for attachment_name, output_name in wanted.items():
            if suggested == attachment_name or suggested.startswith(attachment_name + "_"):
                found[attachment_name] = attachments_dir / attachment["exportedFileName"]
                subprocess.run(
                    [
                        "cwebp", "-quiet", "-q", "86", "-m", "6",
                        "-resize", "402", "0", str(found[attachment_name]),
                        "-o", str(rendered_dir / output_name),
                    ],
                    check=True,
                )

missing = sorted(set(wanted) - set(found))
if missing:
    raise SystemExit("missing website screenshot attachments: " + ", ".join(missing))
PY

mkdir -p "$output_dir"
stale=0
for image in route-imported.webp route-on-device.webp rides-before-sync.webp rides-synced.webp ride-detail.webp; do
  generated="$work_dir/rendered/$image"
  committed="$output_dir/$image"
  if [[ "$mode" == "check" ]]; then
    current="$work_dir/rendered/current-$image.ppm"
    baseline="$work_dir/rendered/baseline-$image.ppm"
    if [[ -f "$committed" ]]; then
      # cwebp's RIFF container bytes can vary while decoding to the exact same pixels. Compare the
      # rendered image instead, which is both strict about UI drift and immune to container noise.
      dwebp -quiet -ppm "$generated" -o "$current"
      dwebp -quiet -ppm "$committed" -o "$baseline"
    fi
    if [[ -f "$committed" ]] && ! cmp -s "$current" "$baseline"; then
      python3 - "$current" "$baseline" <<'PY'
import pathlib
import sys

WHITESPACE = b" \t\r\n"

def read_ppm(path: str) -> tuple[int, int, bytes]:
    raw = pathlib.Path(path).read_bytes()
    offset = 0
    tokens = []
    while len(tokens) < 4:
        while offset < len(raw) and raw[offset] in WHITESPACE:
            offset += 1
        if offset < len(raw) and raw[offset] == ord("#"):
            while offset < len(raw) and raw[offset] not in b"\r\n":
                offset += 1
            continue
        start = offset
        while offset < len(raw) and raw[offset] not in WHITESPACE:
            offset += 1
        tokens.append(raw[start:offset])
    if tokens[0] != b"P6" or tokens[3] != b"255":
        raise SystemExit(f"unsupported PPM header in {path}")
    if raw[offset:offset + 2] == b"\r\n":
        offset += 2
    elif offset < len(raw) and raw[offset] in WHITESPACE:
        offset += 1
    width, height = map(int, tokens[1:3])
    pixels = raw[offset:]
    if len(pixels) != width * height * 3:
        raise SystemExit(f"truncated PPM in {path}")
    return width, height, pixels

width, height, current = read_ppm(sys.argv[1])
base_width, base_height, baseline = read_ppm(sys.argv[2])
if (width, height) != (base_width, base_height):
    print(f"screenshot dimensions differ: {width}x{height} vs {base_width}x{base_height}", file=sys.stderr)
    raise SystemExit(1)

changed = significant = large = total_delta = maximum = 0
for offset in range(0, len(current), 3):
    delta = max(abs(current[offset + channel] - baseline[offset + channel]) for channel in range(3))
    changed += delta > 0
    significant += delta > 8
    large += delta > 32
    maximum = max(maximum, delta)
    total_delta += sum(abs(current[offset + channel] - baseline[offset + channel]) for channel in range(3))
pixels = width * height
print(
    "screenshot pixel drift: "
    f"changed={changed}/{pixels} ({changed / pixels:.3%}), "
    f">8={significant}/{pixels} ({significant / pixels:.3%}), "
    f">32={large}/{pixels} ({large / pixels:.3%}), "
    f"mean-channel-delta={total_delta / (pixels * 3):.4f}, max={maximum}",
    file=sys.stderr,
)
raise SystemExit(1)
PY
    fi
    if [[ ! -f "$committed" ]] || ! cmp -s "$current" "$baseline"; then
      echo "stale companion screenshot: docs/assets/companion/$image" >&2
      stale=1
    fi
  else
    cp "$generated" "$committed"
    echo "updated docs/assets/companion/$image"
  fi
done

if [[ "$stale" -ne 0 ]]; then
  echo "run companion-ios/scripts/capture-website-screenshots.sh and commit the results" >&2
  exit 1
fi

if [[ "$mode" == "check" ]]; then
  echo "companion website screenshots are current"
fi
