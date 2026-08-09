#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 OUTPUT_DIRECTORY" >&2
  exit 64
fi

for wx_command in cargo curl awk date sed seq sort tail tee tr wc; do
  if ! command -v "$wx_command" >/dev/null 2>&1; then
    echo "$wx_command is required" >&2
    exit 69
  fi
done

wx_output=$1
wx_script_dir=$(cd "$(dirname "$0")" && pwd)
wx_repository=$(cd "$wx_script_dir/../.." && pwd)
wx_tool="$wx_repository/target/release/obc-wx-source-spike"
mkdir -p "$wx_output"/{dwd,met,mrms,icon-eu,hrrr,gfs}

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

object_length() {
  local url=$1
  local length
  length=$(curl -fsSI "$url" | tr -d '\r' | awk 'tolower($1) == "content-length:" { print $2 }' | tail -n 1)
  if [[ ! "$length" =~ ^[0-9]+$ ]] || [[ "$length" -eq 0 ]]; then
    echo "No valid Content-Length for $url" >&2
    return 1
  fi
  printf '%s\n' "$length"
}

capture_range() {
  local object_url=$1
  local range=$2
  local expected_bytes=$3
  local destination=$4
  curl -fsS --range "$range" "$object_url" -o "$destination"
  local actual_bytes
  actual_bytes=$(wc -c < "$destination" | tr -d ' ')
  if [[ "$actual_bytes" -ne "$expected_bytes" ]]; then
    echo "$destination has $actual_bytes bytes; expected $expected_bytes from range $range" >&2
    return 1
  fi
}

echo "Building the Rust-only source validator"
(cd "$wx_repository" && cargo build --release -p obc-wx-source-spike)

echo "Fetching and validating the complete DWD RV raw tar"
dwd_url='https://opendata.dwd.de/weather/radar/composite/rv/composite_rv_LATEST.tar'
curl -fsS "$dwd_url" \
  -D "$wx_output/dwd/response.headers" \
  -o "$wx_output/dwd/composite_rv_LATEST.tar" \
  -w 'http=%{http_code} bytes=%{size_download} ttfb=%{time_starttransfer} total=%{time_total}\n' \
  | tee "$wx_output/dwd/metrics.txt"
"$wx_tool" dwd-rv-tar "$wx_output/dwd/composite_rv_LATEST.tar" \
  | tee "$wx_output/dwd/validation.txt"

echo "Fetching and validating the phone-only MET schema at Nordic and non-Nordic points"
met_user_agent='OpenBikeComputer/WX1 https://github.com/timohueser/OpenBikeComputer'
met_url='https://api.met.no/weatherapi/locationforecast/2.0/complete'
for met_case in oslo manila; do
  if [[ "$met_case" == 'oslo' ]]; then
    met_lat='59.9139'; met_lon='10.7522'; met_altitude='23'
  else
    met_lat='14.5995'; met_lon='120.9842'; met_altitude='16'
  fi
  curl -fsS --compressed --get "$met_url" \
    -H "User-Agent: $met_user_agent" \
    --data "lat=$met_lat" --data "lon=$met_lon" --data "altitude=$met_altitude" \
    -D "$wx_output/met/$met_case.headers" \
    -o "$wx_output/met/$met_case.json" \
    -w "$met_case http=%{http_code} wire_bytes=%{size_download} ttfb=%{time_starttransfer} total=%{time_total}\n" \
    | tee -a "$wx_output/met/metrics.txt"
  "$wx_tool" met-response "$wx_output/met/$met_case.json" \
    | tee "$wx_output/met/$met_case.validation.txt"
done

echo "Discovering and validating the newest NOAA MRMS CONUS observation"
mrms_key=''
for wx_days_back in 0 1; do
  wx_day=$(utc_day "$wx_days_back")
  mrms_listing="https://noaa-mrms-pds.s3.amazonaws.com/?list-type=2&prefix=CONUS/PrecipRate_00.00/$wx_day/"
  mrms_key=$(curl -fsS "$mrms_listing" \
    | tr '<' '\n' \
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
curl -fsS "$mrms_url" \
  -D "$wx_output/mrms/response.headers" \
  -o "$wx_output/mrms/precip-rate.grib2.gz" \
  -w 'http=%{http_code} bytes=%{size_download} ttfb=%{time_starttransfer} total=%{time_total}\n' \
  | tee "$wx_output/mrms/metrics.txt"
"$wx_tool" mrms "$wx_output/mrms/precip-rate.grib2.gz" \
  | tee "$wx_output/mrms/validation.txt"

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
  curl -fsS "$icon_url" -o "$icon_file"
  "$wx_tool" icon-eu "$icon_file" > "$wx_output/icon-eu/f$icon_fh.validation.txt"
  if [[ -n "$icon_previous" ]]; then
    "$wx_tool" icon-eu-delta "$icon_previous" "$icon_file" \
      > "$wx_output/icon-eu/f$icon_fh.delta-validation.txt"
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
  curl -fsS "$hrrr_base.idx" -o "$wx_output/hrrr/f$hrrr_file_hour.idx"
  hrrr_length=$(object_length "$hrrr_base")
  if [[ "$hrrr_file_hour" == '01' ]]; then
    hrrr_minutes='15 30 45 60'
  else
    hrrr_minutes='75 90 105 120'
  fi
  for hrrr_minute in $hrrr_minutes; do
    read -r hrrr_range hrrr_bytes < <(
      "$wx_tool" idx-range "$wx_output/hrrr/f$hrrr_file_hour.idx" \
        ":PRATE:surface:$hrrr_minute min fcst:" "$hrrr_length"
    )
    hrrr_field="$wx_output/hrrr/prate-$hrrr_minute.grib2"
    capture_range "$hrrr_base" "$hrrr_range" "$hrrr_bytes" "$hrrr_field"
    "$wx_tool" hrrr-prate "$hrrr_field" \
      > "$wx_output/hrrr/prate-$hrrr_minute.validation.txt"
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
for gfs_hour in $(seq 1 24); do
  printf -v gfs_fh '%03d' "$gfs_hour"
  gfs_base="https://noaa-gfs-bdp-pds.s3.amazonaws.com/gfs.$gfs_date/$gfs_cycle/atmos/gfs.t${gfs_cycle}z.pgrb2.0p25.f$gfs_fh"
  gfs_index="$wx_output/gfs/f$gfs_fh.idx"
  gfs_field="$wx_output/gfs/apcp-f$gfs_fh.grib2"
  curl -fsS "$gfs_base.idx" -o "$gfs_index"
  gfs_length=$(object_length "$gfs_base")
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
  capture_range "$gfs_base" "$gfs_range" "$gfs_bytes" "$gfs_field"
  "$wx_tool" gfs-apcp "$gfs_field" "$gfs_matches" \
    > "$wx_output/gfs/f$gfs_fh.validation.txt"
  if [[ "$gfs_hour" -eq 1 ]]; then
    "$wx_tool" gfs-apcp-first "$gfs_field" "$gfs_matches" \
      > "$wx_output/gfs/f$gfs_fh.delta-validation.txt"
  else
    "$wx_tool" gfs-apcp-step \
      "$gfs_previous" "$gfs_previous_messages" "$gfs_previous_hour" \
      "$gfs_field" "$gfs_matches" "$gfs_hour" \
      > "$wx_output/gfs/f$gfs_fh.delta-validation.txt"
  fi
  gfs_previous=$gfs_field
  gfs_previous_messages=$gfs_matches
  gfs_previous_hour=$gfs_hour
done

cat > "$wx_output/imerg-status.txt" <<'EOF'
IMERG Early V07B: explicit v1 NO-GO. Unattended NASA PPS credentials,
live decode/publication latency, and transformed-output redistribution have not
been proven. Worldwide v1 uses GFS-only; no IMERG observation was fabricated.
EOF
cat "$wx_output/imerg-status.txt"

echo "Running immutable contract tests"
(cd "$wx_repository" && cargo test -p obc-wx-source-spike)

echo "Validated Rust host evidence written to $wx_output"
