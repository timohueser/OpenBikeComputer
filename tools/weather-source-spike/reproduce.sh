#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 OUTPUT_DIRECTORY" >&2
  exit 64
fi

for wx_command in awk cargo cat curl date head od sed seq sort tail tar tee tr uname wc; do
  if ! command -v "$wx_command" >/dev/null 2>&1; then
    echo "$wx_command is required" >&2
    exit 69
  fi
done
if [[ ! -x /usr/bin/time ]]; then
  echo "/usr/bin/time is required for CPU/RSS evidence" >&2
  exit 69
fi

wx_output=$1
wx_script_dir=$(cd "$(dirname "$0")" && pwd)
wx_repository=$(cd "$wx_script_dir/../.." && pwd)
wx_tool="$wx_repository/target/release/obc-wx-source-spike"
mkdir -p "$wx_output"/{dwd,met,mrms,icon-eu,hrrr,gfs}
: > "$wx_output/resource-metrics.txt"

readonly wx_max_compressed_bytes=$((16 * 1024 * 1024))
readonly wx_max_index_bytes=$((2 * 1024 * 1024))
readonly wx_max_noaa_object_bytes=$((2 * 1024 * 1024 * 1024))
readonly wx_gfs_selection_budget_bytes=15500000

header_value() {
  local headers=$1
  local name=$2
  tr -d '\r' < "$headers" \
    | awk -v wanted="$name:" 'tolower($1) == wanted { $1 = ""; sub(/^ /, ""); value = $0 } END { print value }'
}

assert_status() {
  local headers=$1
  local expected=$2
  local actual
  actual=$(tr -d '\r' < "$headers" | awk '/^HTTP\// { value = $2 } END { print value }')
  if [[ "$actual" != "$expected" ]]; then
    echo "$headers records HTTP $actual; expected $expected" >&2
    return 1
  fi
}

assert_header() {
  local headers=$1
  local name=$2
  local pattern=$3
  local value
  value=$(header_value "$headers" "$name")
  if [[ ! "$value" =~ $pattern ]]; then
    echo "$headers has invalid/missing $name: $value" >&2
    return 1
  fi
}

assert_size_at_most() {
  local path=$1
  local maximum=$2
  local size
  size=$(wc -c < "$path" | tr -d ' ')
  if [[ ! "$size" =~ ^[0-9]+$ ]] || [[ "$size" -eq 0 ]] || [[ "$size" -gt "$maximum" ]]; then
    echo "$path has invalid size $size (limit $maximum)" >&2
    return 1
  fi
}

assert_content_length_matches() {
  local headers=$1
  local path=$2
  local advertised actual
  advertised=$(header_value "$headers" content-length)
  actual=$(wc -c < "$path" | tr -d ' ')
  if [[ ! "$advertised" =~ ^[0-9]+$ ]] || [[ "$advertised" -ne "$actual" ]]; then
    echo "$path has $actual bytes but $headers advertised $advertised" >&2
    return 1
  fi
}

assert_magic() {
  local path=$1
  local expected_hex=$2
  local actual_hex
  actual_hex=$(od -An -tx1 -N4 "$path" | tr -d ' \n')
  if [[ ! "$actual_hex" =~ ^$expected_hex ]]; then
    echo "$path has magic $actual_hex; expected $expected_hex" >&2
    return 1
  fi
}

object_length() {
  local url=$1
  local maximum=$2
  local headers=$3
  curl -fsSI "$url" -D "$headers" -o /dev/null
  assert_status "$headers" 200
  assert_header "$headers" content-type '^(application|binary)/octet-stream$'
  assert_header "$headers" accept-ranges '^bytes$'
  assert_header "$headers" last-modified '.+'
  assert_header "$headers" etag '.+'
  local length
  length=$(header_value "$headers" content-length)
  if [[ ! "$length" =~ ^[0-9]+$ ]] || [[ "$length" -eq 0 ]] || [[ "$length" -gt "$maximum" ]]; then
    echo "Invalid/budget-exceeding Content-Length $length for $url (limit $maximum)" >&2
    return 1
  fi
  printf '%s\n' "$length"
}

run_measured() {
  local label=$1
  local validation_output=$2
  shift 2
  local raw_metrics="$validation_output.resource.raw.txt"
  if [[ "$(uname -s)" == Darwin ]]; then
    /usr/bin/time -l "$@" > "$validation_output" 2> "$raw_metrics"
    awk -v label="$label" '
      $2 == "real" && $4 == "user" { user_time = $3; system_time = $5 }
      $2 == "maximum" && $3 == "resident" { rss = $1 }
      END {
        if (user_time == "" || system_time == "" || rss == "") exit 1
        printf "%s user_seconds=%s system_seconds=%s max_rss_bytes=%s\n", label, user_time, system_time, rss
      }
    ' "$raw_metrics" | tee -a "$wx_output/resource-metrics.txt"
  else
    /usr/bin/time -v -o "$raw_metrics" "$@" > "$validation_output"
    awk -v label="$label" '
      /User time \(seconds\)/ { user_time = $NF }
      /System time \(seconds\)/ { system_time = $NF }
      /Maximum resident set size/ { rss = $NF * 1024 }
      END {
        if (user_time == "" || system_time == "" || rss == "") exit 1
        printf "%s user_seconds=%s system_seconds=%s max_rss_bytes=%.0f\n", label, user_time, system_time, rss
      }
    ' "$raw_metrics" | tee -a "$wx_output/resource-metrics.txt"
  fi
}

utc_day() {
  local days_back=$1
  if [[ "$days_back" -eq 0 ]]; then
    date -u +%Y%m%d
  elif date -u -v-"${days_back}"d +%Y%m%d >/dev/null 2>&1; then
    date -u -v-"${days_back}"d +%Y%m%d
  else
    date -u -d "$days_back days ago" +%Y%m%d
  fi
}

capture_range() {
  local object_url=$1
  local range=$2
  local expected_bytes=$3
  local object_bytes=$4
  local destination=$5
  local headers="$destination.headers"
  curl -fsS --max-filesize "$expected_bytes" --range "$range" "$object_url" -D "$headers" -o "$destination"
  assert_status "$headers" 206
  assert_header "$headers" content-range "^bytes $range/$object_bytes$"
  assert_header "$headers" content-type '^(application|binary)/octet-stream$'
  assert_header "$headers" accept-ranges '^bytes$'
  assert_header "$headers" last-modified '.+'
  assert_header "$headers" etag '.+'
  local actual_bytes
  actual_bytes=$(wc -c < "$destination" | tr -d ' ')
  if [[ "$actual_bytes" -ne "$expected_bytes" ]]; then
    echo "$destination has $actual_bytes bytes; expected $expected_bytes from range $range" >&2
    return 1
  fi
  assert_content_length_matches "$headers" "$destination"
  assert_magic "$destination" 47524942
}

echo "Building the Rust-only source validator"
(cd "$wx_repository" && cargo build --release -p obc-wx-source-spike)

echo "Fetching and validating the complete DWD RV raw tar"
dwd_url='https://opendata.dwd.de/weather/radar/composite/rv/composite_rv_LATEST.tar'
dwd_length=$(object_length "$dwd_url" "$wx_max_compressed_bytes" "$wx_output/dwd/preflight.headers")
curl -fsS --max-filesize "$dwd_length" "$dwd_url" \
  -D "$wx_output/dwd/response.headers" \
  -o "$wx_output/dwd/composite_rv_LATEST.tar" \
  -w 'http=%{http_code} bytes=%{size_download} ttfb=%{time_starttransfer} total=%{time_total}\n' \
  | tee "$wx_output/dwd/metrics.txt"
assert_status "$wx_output/dwd/response.headers" 200
assert_header "$wx_output/dwd/response.headers" content-type '^application/octet-stream$'
assert_header "$wx_output/dwd/response.headers" accept-ranges '^bytes$'
assert_header "$wx_output/dwd/response.headers" last-modified '.+'
assert_header "$wx_output/dwd/response.headers" etag '.+'
assert_content_length_matches "$wx_output/dwd/response.headers" "$wx_output/dwd/composite_rv_LATEST.tar"
assert_size_at_most "$wx_output/dwd/composite_rv_LATEST.tar" "$wx_max_compressed_bytes"
tar -tf "$wx_output/dwd/composite_rv_LATEST.tar" >/dev/null
run_measured dwd-rv-tar "$wx_output/dwd/validation.txt" \
  "$wx_tool" dwd-rv-tar "$wx_output/dwd/composite_rv_LATEST.tar"
cat "$wx_output/dwd/validation.txt"

echo "Fetching and validating the phone-only MET schema at Nordic and non-Nordic points"
met_user_agent='OpenBikeComputer/WX1 https://github.com/timohueser/OpenBikeComputer'
met_url='https://api.met.no/weatherapi/locationforecast/2.0/complete'
for met_case in oslo manila; do
  if [[ "$met_case" == 'oslo' ]]; then
    met_lat='59.9139'; met_lon='10.7522'; met_altitude='23'
  else
    met_lat='14.5995'; met_lon='120.9842'; met_altitude='16'
  fi
  curl -fsS --compressed --max-filesize "$wx_max_compressed_bytes" --get "$met_url" \
    -H "User-Agent: $met_user_agent" \
    --data "lat=$met_lat" --data "lon=$met_lon" --data "altitude=$met_altitude" \
    -D "$wx_output/met/$met_case.headers" \
    -o "$wx_output/met/$met_case.json" \
    -w "$met_case http=%{http_code} wire_bytes=%{size_download} ttfb=%{time_starttransfer} total=%{time_total}\n" \
    | tee -a "$wx_output/met/metrics.txt"
  assert_status "$wx_output/met/$met_case.headers" 200
  assert_header "$wx_output/met/$met_case.headers" content-type '^application/json$'
  assert_header "$wx_output/met/$met_case.headers" last-modified '.+'
  assert_header "$wx_output/met/$met_case.headers" expires '.+'
  assert_header "$wx_output/met/$met_case.headers" accept-ranges '^bytes$'
  assert_size_at_most "$wx_output/met/$met_case.json" "$wx_max_compressed_bytes"
  if [[ "$(head -c 1 "$wx_output/met/$met_case.json")" != '{' ]]; then
    echo "MET $met_case response is not a JSON object" >&2
    exit 65
  fi
  met_last_modified=$(header_value "$wx_output/met/$met_case.headers" last-modified)
  curl -sS --compressed --max-filesize "$wx_max_compressed_bytes" --get "$met_url" \
    -H "User-Agent: $met_user_agent" -H "If-Modified-Since: $met_last_modified" \
    --data "lat=$met_lat" --data "lon=$met_lon" --data "altitude=$met_altitude" \
    -D "$wx_output/met/$met_case.conditional.headers" -o /dev/null
  assert_status "$wx_output/met/$met_case.conditional.headers" 304
  run_measured "met-$met_case" "$wx_output/met/$met_case.validation.txt" \
    "$wx_tool" met-response "$wx_output/met/$met_case.json"
  cat "$wx_output/met/$met_case.validation.txt"
done

echo "Discovering and validating the newest NOAA MRMS CONUS observation"
mrms_key=''
for wx_days_back in 0 1; do
  wx_day=$(utc_day "$wx_days_back")
  mrms_listing="https://noaa-mrms-pds.s3.amazonaws.com/?list-type=2&prefix=CONUS/PrecipRate_00.00/$wx_day/"
  mrms_listing_file="$wx_output/mrms/listing-$wx_day.xml"
  curl -fsS --max-filesize "$wx_max_index_bytes" "$mrms_listing" \
    -D "$mrms_listing_file.headers" -o "$mrms_listing_file"
  assert_status "$mrms_listing_file.headers" 200
  assert_header "$mrms_listing_file.headers" content-type '^application/xml$'
  assert_size_at_most "$mrms_listing_file" "$wx_max_index_bytes"
  if [[ "$(head -c 1 "$mrms_listing_file")" != '<' ]]; then
    echo "$mrms_listing_file is not XML" >&2
    exit 65
  fi
  mrms_key=$(tr '<' '\n' < "$mrms_listing_file" \
    | sed -n 's#^Key>\([^<]*\.grib2\.gz\).*#\1#p' \
    | sort \
    | tail -n 1)
  [[ -n "$mrms_key" ]] && break
done
if [[ -z "$mrms_key" ]]; then
  echo "No MRMS PrecipRate object was discoverable for today or yesterday" >&2
  exit 69
fi
printf '%s\n' "$mrms_key" > "$wx_output/mrms/selected-object.txt"
mrms_url="https://noaa-mrms-pds.s3.amazonaws.com/$mrms_key"
mrms_length=$(object_length "$mrms_url" "$wx_max_compressed_bytes" "$wx_output/mrms/preflight.headers")
curl -fsS --max-filesize "$mrms_length" "$mrms_url" \
  -D "$wx_output/mrms/response.headers" \
  -o "$wx_output/mrms/precip-rate.grib2.gz" \
  -w 'http=%{http_code} bytes=%{size_download} ttfb=%{time_starttransfer} total=%{time_total}\n' \
  | tee "$wx_output/mrms/metrics.txt"
assert_status "$wx_output/mrms/response.headers" 200
assert_header "$wx_output/mrms/response.headers" content-type '^application/octet-stream$'
assert_header "$wx_output/mrms/response.headers" accept-ranges '^bytes$'
assert_header "$wx_output/mrms/response.headers" last-modified '.+'
assert_header "$wx_output/mrms/response.headers" etag '.+'
assert_content_length_matches "$wx_output/mrms/response.headers" "$wx_output/mrms/precip-rate.grib2.gz"
assert_magic "$wx_output/mrms/precip-rate.grib2.gz" 1f8b08
run_measured mrms "$wx_output/mrms/validation.txt" \
  "$wx_tool" mrms "$wx_output/mrms/precip-rate.grib2.gz"
cat "$wx_output/mrms/validation.txt"

echo "Selecting and validating the newest complete ICON-EU f000..f011 set"
icon_date=''
icon_cycle=''
for wx_days_back in 0 1; do
  wx_day=$(utc_day "$wx_days_back")
  for wx_cycle_number in 18 12 6 0; do
    printf -v wx_cycle '%02d' "$wx_cycle_number"
    icon_probe="https://opendata.dwd.de/weather/nwp/icon-eu/grib/$wx_cycle/tot_prec/icon-eu_europe_regular-lat-lon_single-level_${wx_day}${wx_cycle}_011_TOT_PREC.grib2.bz2"
    if curl -fIs "$icon_probe" >/dev/null; then
      icon_date=$wx_day
      icon_cycle=$wx_cycle
      break 2
    fi
  done
done
if [[ -z "$icon_date" ]]; then
  echo "No complete ICON-EU f011 run was available" >&2
  exit 69
fi
printf '%s %s\n' "$icon_date" "$icon_cycle" > "$wx_output/icon-eu/selected-cycle.txt"
icon_previous=''
for icon_hour in $(seq 0 11); do
  printf -v icon_fh '%03d' "$icon_hour"
  icon_file="$wx_output/icon-eu/f$icon_fh.grib2.bz2"
  icon_url="https://opendata.dwd.de/weather/nwp/icon-eu/grib/$icon_cycle/tot_prec/icon-eu_europe_regular-lat-lon_single-level_${icon_date}${icon_cycle}_${icon_fh}_TOT_PREC.grib2.bz2"
  icon_length=$(object_length "$icon_url" "$wx_max_compressed_bytes" "$wx_output/icon-eu/f$icon_fh.preflight.headers")
  curl -fsS --max-filesize "$icon_length" "$icon_url" \
    -D "$wx_output/icon-eu/f$icon_fh.headers" -o "$icon_file"
  assert_status "$wx_output/icon-eu/f$icon_fh.headers" 200
  assert_header "$wx_output/icon-eu/f$icon_fh.headers" content-type '^application/octet-stream$'
  assert_header "$wx_output/icon-eu/f$icon_fh.headers" accept-ranges '^bytes$'
  assert_header "$wx_output/icon-eu/f$icon_fh.headers" last-modified '.+'
  assert_header "$wx_output/icon-eu/f$icon_fh.headers" etag '.+'
  assert_content_length_matches "$wx_output/icon-eu/f$icon_fh.headers" "$icon_file"
  assert_magic "$icon_file" 425a68
  run_measured "icon-eu-f$icon_fh" "$wx_output/icon-eu/f$icon_fh.validation.txt" \
    "$wx_tool" icon-eu "$icon_file" "$icon_hour"
  if [[ -n "$icon_previous" ]]; then
    run_measured "icon-eu-delta-f$icon_fh" "$wx_output/icon-eu/f$icon_fh.delta-validation.txt" \
      "$wx_tool" icon-eu-delta "$icon_previous" "$icon_file"
  fi
  icon_previous=$icon_file
done

echo "Selecting and validating the newest complete HRRR CONUS +2-hour run"
hrrr_date=''
hrrr_cycle=''
for wx_days_back in 0 1; do
  wx_day=$(utc_day "$wx_days_back")
  for wx_cycle_number in $(seq 23 -1 0); do
    printf -v wx_cycle '%02d' "$wx_cycle_number"
    hrrr_probe="https://noaa-hrrr-bdp-pds.s3.amazonaws.com/hrrr.$wx_day/conus/hrrr.t${wx_cycle}z.wrfsubhf02.grib2.idx"
    if curl -fIs "$hrrr_probe" >/dev/null; then
      hrrr_date=$wx_day
      hrrr_cycle=$wx_cycle
      break 2
    fi
  done
done
if [[ -z "$hrrr_date" ]]; then
  echo "No complete HRRR f02 run was available" >&2
  exit 69
fi
printf '%s %s\n' "$hrrr_date" "$hrrr_cycle" > "$wx_output/hrrr/selected-cycle.txt"
for hrrr_file_hour in 01 02; do
  hrrr_base="https://noaa-hrrr-bdp-pds.s3.amazonaws.com/hrrr.$hrrr_date/conus/hrrr.t${hrrr_cycle}z.wrfsubhf${hrrr_file_hour}.grib2"
  hrrr_index="$wx_output/hrrr/f$hrrr_file_hour.idx"
  hrrr_index_length=$(object_length "$hrrr_base.idx" "$wx_max_index_bytes" "$hrrr_index.preflight.headers")
  curl -fsS --max-filesize "$hrrr_index_length" "$hrrr_base.idx" \
    -D "$hrrr_index.headers" -o "$hrrr_index"
  assert_status "$hrrr_index.headers" 200
  assert_header "$hrrr_index.headers" content-type '^binary/octet-stream$'
  assert_header "$hrrr_index.headers" accept-ranges '^bytes$'
  assert_header "$hrrr_index.headers" last-modified '.+'
  assert_header "$hrrr_index.headers" etag '.+'
  assert_content_length_matches "$hrrr_index.headers" "$hrrr_index"
  if [[ ! "$(head -c 1 "$hrrr_index")" =~ [0-9] ]]; then
    echo "$hrrr_index does not begin with a wgrib2 record" >&2
    exit 65
  fi
  hrrr_length=$(object_length "$hrrr_base" "$wx_max_noaa_object_bytes" \
    "$wx_output/hrrr/f$hrrr_file_hour.object.headers")
  if [[ "$hrrr_file_hour" == '01' ]]; then
    hrrr_minutes='15 30 45 60'
  else
    hrrr_minutes='75 90 105 120'
  fi
  for hrrr_minute in $hrrr_minutes; do
    read -r hrrr_range hrrr_bytes < <(
      "$wx_tool" idx-range "$hrrr_index" \
        ":PRATE:surface:$hrrr_minute min fcst:" "$hrrr_length"
    )
    hrrr_field="$wx_output/hrrr/prate-$hrrr_minute.grib2"
    capture_range "$hrrr_base" "$hrrr_range" "$hrrr_bytes" "$hrrr_length" "$hrrr_field"
    run_measured "hrrr-prate-$hrrr_minute" "$wx_output/hrrr/prate-$hrrr_minute.validation.txt" \
      "$wx_tool" hrrr-prate "$hrrr_field" "$hrrr_minute"
  done
done

echo "Selecting and validating the newest complete GFS worldwide 24-hour floor"
gfs_date=''
gfs_cycle=''
for wx_days_back in 0 1; do
  wx_day=$(utc_day "$wx_days_back")
  for wx_cycle_number in 18 12 6 0; do
    printf -v wx_cycle '%02d' "$wx_cycle_number"
    gfs_probe="https://noaa-gfs-bdp-pds.s3.amazonaws.com/gfs.$wx_day/$wx_cycle/atmos/gfs.t${wx_cycle}z.pgrb2.0p25.f024.idx"
    if curl -fIs "$gfs_probe" >/dev/null; then
      gfs_date=$wx_day
      gfs_cycle=$wx_cycle
      break 2
    fi
  done
done
if [[ -z "$gfs_date" ]]; then
  echo "No complete GFS f024 run was available" >&2
  exit 69
fi
printf '%s %s\n' "$gfs_date" "$gfs_cycle" > "$wx_output/gfs/selected-cycle.txt"
gfs_previous=''
gfs_previous_messages=''
gfs_previous_hour=''
gfs_total_bytes=0
gfs_high_water_bytes=0
gfs_high_water_hour=0
for gfs_hour in $(seq 1 24); do
  printf -v gfs_fh '%03d' "$gfs_hour"
  gfs_base="https://noaa-gfs-bdp-pds.s3.amazonaws.com/gfs.$gfs_date/$gfs_cycle/atmos/gfs.t${gfs_cycle}z.pgrb2.0p25.f$gfs_fh"
  gfs_index="$wx_output/gfs/f$gfs_fh.idx"
  gfs_field="$wx_output/gfs/apcp-f$gfs_fh.grib2"
  gfs_index_length=$(object_length "$gfs_base.idx" "$wx_max_index_bytes" "$gfs_index.preflight.headers")
  curl -fsS --max-filesize "$gfs_index_length" "$gfs_base.idx" \
    -D "$gfs_index.headers" -o "$gfs_index"
  assert_status "$gfs_index.headers" 200
  assert_header "$gfs_index.headers" content-type '^binary/octet-stream$'
  assert_header "$gfs_index.headers" accept-ranges '^bytes$'
  assert_header "$gfs_index.headers" last-modified '.+'
  assert_header "$gfs_index.headers" etag '.+'
  assert_content_length_matches "$gfs_index.headers" "$gfs_index"
  if [[ ! "$(head -c 1 "$gfs_index")" =~ [0-9] ]]; then
    echo "$gfs_index does not begin with a wgrib2 record" >&2
    exit 65
  fi
  gfs_length=$(object_length "$gfs_base" "$wx_max_noaa_object_bytes" "$gfs_index.object.headers")
  if [[ "$gfs_hour" -eq 24 ]]; then
    gfs_selector=':APCP:surface:0-1 day acc fcst:'
  else
    gfs_selector=":APCP:surface:0-$gfs_hour hour acc fcst:"
  fi
  gfs_matches=$(awk -v selector="$gfs_selector" 'index($0, selector) { count++ } END { print count + 0 }' "$gfs_index")
  if [[ "$gfs_matches" -eq 1 ]]; then
    read -r gfs_range gfs_bytes < <(
      "$wx_tool" idx-range "$gfs_index" "$gfs_selector" "$gfs_length"
    )
  elif [[ "$gfs_matches" -eq 2 ]]; then
    read -r gfs_range gfs_bytes < <(
      "$wx_tool" idx-span "$gfs_index" "$gfs_selector" "$gfs_length" 2
    )
  else
    echo "GFS f$gfs_fh cumulative selector matched $gfs_matches records" >&2
    exit 65
  fi
  capture_range "$gfs_base" "$gfs_range" "$gfs_bytes" "$gfs_length" "$gfs_field"
  gfs_total_bytes=$((gfs_total_bytes + gfs_bytes))
  if [[ "$gfs_bytes" -gt "$gfs_high_water_bytes" ]]; then
    gfs_high_water_bytes=$gfs_bytes
    gfs_high_water_hour=$gfs_hour
  fi
  if [[ "$gfs_total_bytes" -gt "$wx_gfs_selection_budget_bytes" ]]; then
    echo "GFS selection exceeded $wx_gfs_selection_budget_bytes bytes at f$gfs_fh" >&2
    exit 65
  fi
  run_measured "gfs-apcp-f$gfs_fh" "$wx_output/gfs/f$gfs_fh.validation.txt" \
    "$wx_tool" gfs-apcp "$gfs_field" "$gfs_matches"
  if [[ "$gfs_hour" -eq 1 ]]; then
    run_measured "gfs-delta-f$gfs_fh" "$wx_output/gfs/f$gfs_fh.delta-validation.txt" \
      "$wx_tool" gfs-apcp-first "$gfs_field" "$gfs_matches"
  else
    run_measured "gfs-delta-f$gfs_fh" "$wx_output/gfs/f$gfs_fh.delta-validation.txt" \
      "$wx_tool" gfs-apcp-step \
      "$gfs_previous" "$gfs_previous_messages" "$gfs_previous_hour" \
      "$gfs_field" "$gfs_matches" "$gfs_hour"
  fi
  gfs_previous=$gfs_field
  gfs_previous_messages=$gfs_matches
  gfs_previous_hour=$gfs_hour
done
gfs_headroom_bytes=$((wx_gfs_selection_budget_bytes - gfs_total_bytes))
printf 'selected_cycle=%sT%sZ selected_spans=24 total_bytes=%s high_water_bytes=%s high_water_hour=%s budget_bytes=%s headroom_bytes=%s\n' \
  "$gfs_date" "$gfs_cycle" "$gfs_total_bytes" "$gfs_high_water_bytes" "$gfs_high_water_hour" \
  "$wx_gfs_selection_budget_bytes" "$gfs_headroom_bytes" \
  | tee "$wx_output/gfs/budget.txt"

cat > "$wx_output/imerg-status.txt" <<'EOF'
IMERG Early V07B: explicit v1 NO-GO. Unattended NASA PPS credentials,
live decode/publication latency, and transformed-output redistribution have not
been proven. Worldwide v1 uses GFS-only; no IMERG observation was fabricated.
EOF
cat "$wx_output/imerg-status.txt"

echo "Running immutable contract tests"
(cd "$wx_repository" && cargo test -p obc-wx-source-spike)

echo "Validated Rust host evidence written to $wx_output"
