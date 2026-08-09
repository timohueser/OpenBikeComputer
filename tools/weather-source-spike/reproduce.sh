#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 OUTPUT_DIRECTORY" >&2
  exit 64
fi

wx_output=$1
mkdir -p "$wx_output/dwd" "$wx_output/met" "$wx_output/gfs"
wx_script_dir=$(cd "$(dirname "$0")" && pwd)
wx_repository=$(cd "$wx_script_dir/../.." && pwd)

if ! command -v uv >/dev/null 2>&1; then
  echo "uv is required to run the pinned HDF5/GeoTIFF validator" >&2
  exit 69
fi

echo "Fetching DWD coverage discovery documents"
dwd_wcs='https://maps.dwd.de/geoserver/dwd/wcs'
curl -fsS --compressed --get "$dwd_wcs" \
  --data 'service=WCS' --data 'version=2.0.1' \
  --data 'request=GetCapabilities' \
  -D "$wx_output/dwd/capabilities.headers" \
  -o "$wx_output/dwd/capabilities.xml"
curl -fsS --compressed --get "$dwd_wcs" \
  --data 'service=WCS' --data 'version=2.0.1' \
  --data 'request=DescribeCoverage' \
  --data 'coverageId=dwd__Niederschlagsradar' \
  -D "$wx_output/dwd/describe.headers" \
  -o "$wx_output/dwd/describe.xml"

echo "Fetching the maintained Germany-wide RV bundle for numeric cross-checks"
curl -fsS --compressed \
  'https://opendata.dwd.de/weather/radar/composite/rv/composite_rv_LATEST.tar' \
  -D "$wx_output/dwd/raw-rv.headers" \
  -o "$wx_output/dwd/composite_rv_LATEST.tar" \
  -w 'raw-rv http=%{http_code} bytes=%{size_download} ttfb=%{time_starttransfer} total=%{time_total}\n' \
  | tee "$wx_output/dwd/metrics.txt"

python3 - "$wx_output/dwd/composite_rv_LATEST.tar" \
  "$wx_output/dwd/selected-reference.txt" \
  "$wx_output/dwd/valid-times.txt" <<'PY'
from datetime import datetime, timedelta
import re
import sys
import tarfile

with tarfile.open(sys.argv[1]) as archive:
    members = archive.getnames()
match = re.fullmatch(r"composite_rv_(\d{8})_(\d{4})_000-hd5", members[0])
if match is None:
    raise SystemExit("DWD RV archive does not start with a lead-000 HDF5 member")
run = datetime.strptime("".join(match.groups()), "%Y%m%d%H%M")
reference = run.isoformat(timespec="milliseconds") + "Z"
with open(sys.argv[2], "w", encoding="utf-8") as output:
    output.write(reference + "\n")
with open(sys.argv[3], "w", encoding="utf-8") as output:
    for minutes in range(0, 121, 15):
        output.write(
            (run + timedelta(minutes=minutes)).isoformat(timespec="milliseconds")
            + "Z\n"
        )
PY

dwd_reference=$(sed -n '1p' "$wx_output/dwd/selected-reference.txt")
while IFS= read -r dwd_valid; do
  dwd_token=$(printf '%s' "$dwd_valid" | tr -cd '0-9')
  curl -fsS --compressed --get "$dwd_wcs" \
    --data 'service=WCS' --data 'version=2.0.1' \
    --data 'request=GetCoverage' \
    --data 'coverageId=dwd__Niederschlagsradar' \
    --data-urlencode 'subset=Lat(52.5016,53.3656)' \
    --data-urlencode 'subset=Long(6.8560,8.2932)' \
    --data-urlencode "subset=time(\"$dwd_valid\")" \
    --data-urlencode "subset=REFERENCE_TIME(\"$dwd_reference\")" \
    --data-urlencode 'format=image/tiff;application=geotiff' \
    -D "$wx_output/dwd/$dwd_token.headers" \
    -o "$wx_output/dwd/$dwd_token.tif" \
    -w "$dwd_valid http=%{http_code} bytes=%{size_download} ttfb=%{time_starttransfer} total=%{time_total}\n" \
    | tee -a "$wx_output/dwd/metrics.txt"
done < "$wx_output/dwd/valid-times.txt"

echo "Fetching one-off MET evidence; this does not authorize a direct native production client"
met_user_agent='OpenBikeComputer/WX1 https://github.com/timohueser/OpenBikeComputer'
met_url='https://api.met.no/weatherapi/locationforecast/2.0/complete'
curl -fsS --compressed --get "$met_url" \
  -H "User-Agent: $met_user_agent" \
  --data 'lat=59.9139' --data 'lon=10.7522' --data 'altitude=23' \
  -D "$wx_output/met/oslo.headers" \
  -o "$wx_output/met/oslo.json" \
  -w 'met-oslo http=%{http_code} wire_bytes=%{size_download} ttfb=%{time_starttransfer} total=%{time_total}\n' \
  | tee "$wx_output/met/metrics.txt"

curl -fsS --compressed --get "$met_url" \
  -H "User-Agent: $met_user_agent" \
  --data 'lat=14.5995' --data 'lon=120.9842' --data 'altitude=16' \
  -D "$wx_output/met/manila.headers" \
  -o "$wx_output/met/manila.json" \
  -w 'met-manila http=%{http_code} wire_bytes=%{size_download} ttfb=%{time_starttransfer} total=%{time_total}\n' \
  | tee -a "$wx_output/met/metrics.txt"

met_last_modified=$(awk '
  BEGIN { IGNORECASE=1 }
  /^last-modified:/ {
    sub(/^last-modified:[[:space:]]*/, "")
    sub(/\r$/, "")
    print
  }
 ' "$wx_output/met/oslo.headers")
curl -sS --compressed --get "$met_url" \
  -H "User-Agent: $met_user_agent" \
  -H "If-Modified-Since: $met_last_modified" \
  --data 'lat=59.9139' --data 'lon=10.7522' --data 'altitude=23' \
  -D "$wx_output/met/conditional.headers" \
  -o "$wx_output/met/conditional.body" \
  -w 'met-conditional http=%{http_code} wire_bytes=%{size_download} total=%{time_total}\n' \
  | tee -a "$wx_output/met/metrics.txt"

echo "Selecting the latest complete GFS cycle (f024 must exist)"
python3 - <<'PY' > "$wx_output/gfs/candidates.txt"
from datetime import datetime, timedelta, timezone

now = datetime.now(timezone.utc)
for days_back in range(0, 3):
    day = (now - timedelta(days=days_back)).date()
    for cycle in (18, 12, 6, 0):
        candidate = datetime(day.year, day.month, day.day, cycle, tzinfo=timezone.utc)
        if candidate <= now:
            print(candidate.strftime("%Y%m%d %H"))
PY

gfs_date=''
gfs_cycle=''
while read -r candidate_date candidate_cycle; do
  idx="https://nomads.ncep.noaa.gov/pub/data/nccf/com/gfs/prod/gfs.$candidate_date/$candidate_cycle/atmos/gfs.t${candidate_cycle}z.pgrb2.0p25.f024.idx"
  gfs_http=$(curl -sS -o /dev/null -w '%{http_code}' "$idx")
  if [[ "$gfs_http" == '200' ]]; then
    gfs_date=$candidate_date
    gfs_cycle=$candidate_cycle
    break
  fi
done < "$wx_output/gfs/candidates.txt"
if [[ -z "$gfs_date" ]]; then
  echo "No complete GFS f024 cycle was available" >&2
  exit 69
fi
printf '%s %s\n' "$gfs_date" "$gfs_cycle" > "$wx_output/gfs/selected-cycle.txt"

for gfs_hour in $(seq 1 24); do
  gfs_fh=$(printf '%03d' "$gfs_hour")
  curl -fsS --compressed --get \
    'https://nomads.ncep.noaa.gov/cgi-bin/filter_gfs_0p25.pl' \
    --data "file=gfs.t${gfs_cycle}z.pgrb2.0p25.f${gfs_fh}" \
    --data 'lev_surface=on' --data 'var_APCP=on' --data 'subregion=' \
    --data 'leftlon=120.53' --data 'rightlon=121.43' \
    --data 'toplat=15.03' --data 'bottomlat=14.17' \
    --data-urlencode "dir=/gfs.$gfs_date/$gfs_cycle/atmos" \
    -D "$wx_output/gfs/f${gfs_fh}.headers" \
    -o "$wx_output/gfs/f${gfs_fh}.grib2" \
    -w "f${gfs_fh} http=%{http_code} bytes=%{size_download} ttfb=%{time_starttransfer} total=%{time_total}\n" \
    | tee -a "$wx_output/gfs/metrics.txt"
done

echo "Validating MIME, grids, timestamps, provider fields, raw/WCS cells, and hashes"
uv run --script "$wx_script_dir/validate_sources.py" "$wx_output"

echo "Decoding every live GFS crop through the production Swift decoder"
(
  cd "$wx_repository/companion-ios/Packages/OBCKit"
  OBC_WEATHER_LIVE_GFS_DIRECTORY="$wx_output/gfs" \
    swift test --filter decodesOptInLiveCapture
)

echo "Validated evidence written to $wx_output"
