#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = [
#   "h5py==3.16.0",
#   "numpy==2.4.2",
#   "pyproj==3.7.2",
#   "rasterio==1.5.1",
# ]
# ///

"""Fail-closed semantic validation for the opt-in WX1 live reproduction."""

from __future__ import annotations

import base64
import hashlib
import json
import re
import sys
import tarfile
import xml.etree.ElementTree as ET
from datetime import datetime, timedelta, timezone
from io import BytesIO
from itertools import pairwise
from pathlib import Path

import h5py
import numpy as np
import rasterio
from pyproj import CRS, Transformer

EXPECTED_DWD_ENVELOPE = (
    45.68555450439453,
    1.4656230211257935,
    56.21059036254883,
    18.71379280090332,
)
EXPECTED_GFS_FIXTURE_SHA256 = (
    "be6705b5d5a3e56b5a11cf42295ea1804e5b4a49b237082d877df3612e385566"
)
DWD_REPROJECTION_BOUNDARY_TOLERANCE_CELLS = 0.1


def require(condition: bool, message: str) -> None:
    if not condition:
        raise RuntimeError(message)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def response_headers(path: Path) -> tuple[int, dict[str, str]]:
    status = 0
    headers: dict[str, str] = {}
    for raw_line in path.read_text(encoding="utf-8").splitlines():
        if raw_line.startswith("HTTP/"):
            match = re.search(r"\s(\d{3})(?:\s|$)", raw_line)
            if match:
                status = int(match.group(1))
                headers = {}
            continue
        if ":" in raw_line:
            name, value = raw_line.split(":", 1)
            headers[name.strip().lower()] = value.strip()
    return status, headers


def require_response(
    headers_path: Path,
    expected_status: int,
    content_type_prefix: str | None = None,
) -> dict[str, str]:
    status, headers = response_headers(headers_path)
    require(status == expected_status, f"{headers_path}: HTTP {status}")
    if content_type_prefix is not None:
        content_type = headers.get("content-type", "").lower()
        require(
            content_type.startswith(content_type_prefix),
            f"{headers_path}: unexpected Content-Type {content_type!r}",
        )
    return headers


def parse_utc(value: str) -> datetime:
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


def nearest_cell_candidates(coordinate: float) -> list[int]:
    """Include both cells only within 100 m of a reprojected 1 km boundary."""
    nearest = round(coordinate)
    fraction = coordinate - np.floor(coordinate)
    if abs(fraction - 0.5) <= DWD_REPROJECTION_BOUNDARY_TOLERANCE_CELLS:
        return sorted({nearest, int(np.floor(coordinate)), int(np.ceil(coordinate))})
    return [nearest]


def xml_element(root: ET.Element, suffix: str) -> ET.Element:
    for element in root.iter():
        if element.tag.endswith(suffix):
            return element
    raise RuntimeError(f"XML element {suffix} is missing")


def validate_dwd(root: Path) -> dict[str, object]:
    dwd = root / "dwd"
    require_response(dwd / "capabilities.headers", 200, "application/xml")
    require_response(dwd / "describe.headers", 200, "application/xml")
    require_response(dwd / "raw-rv.headers", 200, "application/octet-stream")

    capabilities_text = (dwd / "capabilities.xml").read_text(encoding="utf-8")
    describe_xml = (dwd / "describe.xml").read_text(encoding="utf-8")
    describe_root = ET.fromstring(describe_xml)
    require("mm/h" in capabilities_text, "DWD capabilities lost the mm/h description")
    require(
        "W.m-2.Sr-1" in describe_xml,
        "DWD DescribeCoverage metadata contradiction changed; re-audit units",
    )

    envelope = xml_element(describe_root, "EnvelopeWithTimePeriod")
    lower = [
        float(value) for value in xml_element(envelope, "lowerCorner").text.split()
    ]
    upper = [
        float(value) for value in xml_element(envelope, "upperCorner").text.split()
    ]
    observed_envelope = (lower[0], lower[1], upper[0], upper[1])
    require(
        all(
            abs(actual - expected) <= 1e-9
            for actual, expected in zip(observed_envelope, EXPECTED_DWD_ENVELOPE)
        ),
        f"DWD supported envelope changed: {observed_envelope}",
    )

    reference = (dwd / "selected-reference.txt").read_text(encoding="utf-8").strip()
    advertised_references = {
        element.text.strip()
        for element in describe_root.iter()
        if element.tag.endswith("timePosition") and element.text
    }
    require(reference in advertised_references, "raw RV run is not advertised by WCS")

    valid_times = [
        line.strip()
        for line in (dwd / "valid-times.txt").read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    require(len(valid_times) == 9, "DWD reproduction must contain nine frames")
    reference_date = parse_utc(reference)
    require(
        [parse_utc(value) for value in valid_times]
        == [
            reference_date + timedelta(minutes=minutes) for minutes in range(0, 121, 15)
        ],
        "DWD frame times do not form one coherent +0…+120 run",
    )

    frame_paths: list[Path] = []
    frame_hashes: dict[str, str] = {}
    frame_facts: list[dict[str, object]] = []
    for valid_time in valid_times:
        token = re.sub(r"\D", "", valid_time)
        frame = dwd / f"{token}.tif"
        headers = dwd / f"{token}.headers"
        require_response(headers, 200, "image/tiff")
        require(
            frame.read_bytes()[:4] in (b"MM\x00*", b"II*\x00"),
            f"{frame}: missing TIFF magic",
        )
        with rasterio.open(frame) as dataset:
            require(dataset.driver == "GTiff", f"{frame}: not GeoTIFF")
            require(dataset.crs == CRS.from_epsg(4326), f"{frame}: unexpected CRS")
            require(dataset.count == 1, f"{frame}: expected one band")
            require(dataset.dtypes == ("float32",), f"{frame}: expected float32")
            require(
                dataset.width <= 512 and dataset.height <= 512, "DWD crop too large"
            )
            require(dataset.nodata == 4294967296.0, "DWD no-data sentinel changed")
            require(
                dataset.transform.b == 0 and dataset.transform.d == 0,
                "rotated WCS grid",
            )
            frame_facts.append(
                {
                    "valid_time": valid_time,
                    "width": dataset.width,
                    "height": dataset.height,
                    "transform": list(dataset.transform)[:6],
                    "bounds": list(dataset.bounds),
                }
            )
        frame_paths.append(frame)
        frame_hashes[valid_time] = sha256(frame)
    first_frame_geometry = {
        key: frame_facts[0][key] for key in ("width", "height", "transform", "bounds")
    }
    require(
        first_frame_geometry["width"] == 104 and first_frame_geometry["height"] == 104,
        f"DWD route crop dimensions changed: {first_frame_geometry}",
    )
    require(
        all(
            all(frame[key] == first_frame_geometry[key] for key in first_frame_geometry)
            for frame in frame_facts
        ),
        "DWD frame geometry changed within one reference run",
    )

    raw_tar = dwd / "composite_rv_LATEST.tar"
    require(tarfile.is_tarfile(raw_tar), "DWD raw response is not a tar archive")
    raw_reference_token = reference_date.strftime("%Y%m%d_%H%M")
    pattern = re.compile(rf"^composite_rv_{raw_reference_token}_(\d{{3}})-hd5$")
    with tarfile.open(raw_tar) as archive:
        matching = [
            (member, int(match.group(1)))
            for member in archive.getmembers()
            if (match := pattern.match(member.name))
        ]
        require(
            sorted(lead for _, lead in matching) == list(range(0, 121, 5)),
            "DWD raw RV archive does not contain one complete 0…120 minute run",
        )
        analysis_member = next(member for member, lead in matching if lead == 0)
        extracted = archive.extractfile(analysis_member)
        require(extracted is not None, "DWD analysis HDF5 is unreadable")
        hdf_bytes = extracted.read()

    with h5py.File(BytesIO(hdf_bytes), "r") as hdf:
        raw = hdf["dataset1/data1/data"][:]
        data_what = hdf["dataset1/data1/what"].attrs
        product_what = hdf["dataset1/what"].attrs
        where = hdf["where"].attrs
        require(raw.shape == (1200, 1100), f"DWD raw shape changed: {raw.shape}")
        require(data_what["quantity"] == b"ACRR", "DWD raw quantity is not ACRR")
        require(float(where["xscale"]) == 1000, "DWD raw x resolution changed")
        require(float(where["yscale"]) == 1000, "DWD raw y resolution changed")
        require(
            product_what["startdate"].decode() + product_what["starttime"].decode()
            == (reference_date - timedelta(minutes=5)).strftime("%Y%m%d%H%M%S"),
            "DWD analysis start time changed",
        )
        require(
            product_what["enddate"].decode() + product_what["endtime"].decode()
            == reference_date.strftime("%Y%m%d%H%M%S"),
            "DWD analysis end time changed",
        )
        gain = float(data_what["gain"])
        offset = float(data_what["offset"])
        nodata = int(data_what["nodata"])
        projection = where["projdef"].decode()

    comparisons = 0
    mismatches = 0
    mismatch_examples: list[dict[str, object]] = []
    maximum_error = 0.0
    positive_matches = 0
    boundary_matches = 0
    sampled_raw_cells: set[tuple[int, int]] = set()
    with rasterio.open(frame_paths[0]) as wcs:
        values = wcs.read(1)
        transformer = Transformer.from_crs(
            wcs.crs,
            CRS.from_proj4(projection),
            always_xy=True,
        )
        for row, column in np.ndindex(values.shape):
            longitude, latitude = wcs.transform * (column + 0.5, row + 0.5)
            x, y = transformer.transform(longitude, latitude)
            raw_columns = nearest_cell_candidates(x / 1000)
            raw_rows = nearest_cell_candidates(-y / 1000)
            candidates = [
                (raw_row, raw_column)
                for raw_row in raw_rows
                for raw_column in raw_columns
                if 0 <= raw_row < raw.shape[0] and 0 <= raw_column < raw.shape[1]
            ]
            if not candidates:
                continue
            wcs_value = float(values[row, column])
            if wcs_value in (-999.0, 4294967296.0):
                continue
            candidate_errors = [
                (
                    abs(wcs_value - (int(raw[raw_row, raw_column]) * gain + offset)),
                    raw_row,
                    raw_column,
                    int(raw[raw_row, raw_column]),
                )
                for raw_row, raw_column in candidates
                if int(raw[raw_row, raw_column]) != nodata
            ]
            if not candidate_errors:
                continue
            nearest_position = (round(-y / 1000), round(x / 1000))
            nearest_error = next(
                (
                    candidate[0]
                    for candidate in candidate_errors
                    if (candidate[1], candidate[2]) == nearest_position
                ),
                None,
            )
            error, raw_row, raw_column, raw_value = min(candidate_errors)
            comparisons += 1
            sampled_raw_cells.add((raw_row, raw_column))
            maximum_error = max(maximum_error, error)
            if error > 1e-6:
                mismatches += 1
                if len(mismatch_examples) < 5:
                    mismatch_examples.append(
                        {
                            "wcs_row_column": [row, column],
                            "wcs_value": wcs_value,
                            "raw_row_column": [raw_row, raw_column],
                            "raw_value": raw_value,
                            "absolute_error": error,
                        }
                    )
            elif nearest_error is None or nearest_error > 1e-6:
                boundary_matches += 1
            if wcs_value >= 0:
                positive_matches += 1
    require(comparisons >= 1000, "too few raw/WCS cell comparisons")
    require(
        mismatches == 0,
        f"{mismatches} DWD raw/WCS samples disagree: {mismatch_examples}",
    )
    require(
        len(sampled_raw_cells) / comparisons >= 0.95,
        "WCS crop oversamples too few native RV cells",
    )

    return {
        "coverage_envelope_lat_lon": observed_envelope,
        "reference_time": reference,
        "frame_count": len(frame_paths),
        "frames": frame_facts,
        "frame_sha256": frame_hashes,
        "raw_tar_sha256": sha256(raw_tar),
        "raw_shape": list(raw.shape),
        "raw_resolution_m": 1000,
        "raw_wcs_comparisons": comparisons,
        "raw_wcs_unique_source_cells": len(sampled_raw_cells),
        "raw_wcs_positive_matches": positive_matches,
        "raw_wcs_boundary_matches": boundary_matches,
        "raw_wcs_maximum_absolute_error": maximum_error,
    }


def hourly_records(document: dict[str, object]) -> list[dict[str, object]]:
    properties = document["properties"]
    assert isinstance(properties, dict)
    records = properties["timeseries"]
    assert isinstance(records, list)
    return records[:24]


def validate_hour_spacing(records: list[dict[str, object]], label: str) -> None:
    times = [parse_utc(str(record["time"])) for record in records]
    require(len(times) == 24, f"{label}: expected 24 hourly records")
    require(
        all(
            later - earlier == timedelta(hours=1) for earlier, later in pairwise(times)
        ),
        f"{label}: records are not hourly and ordered",
    )


def met_field_counts(records: list[dict[str, object]]) -> dict[str, int]:
    counts = {
        "air_temperature": 0,
        "wind_from_direction": 0,
        "wind_speed": 0,
        "wind_speed_of_gust": 0,
        "precipitation_amount": 0,
        "probability_of_precipitation": 0,
        "symbol_code": 0,
    }
    for record in records:
        data = record["data"]
        instant = data["instant"]["details"]
        next_hour = data.get("next_1_hours", {})
        details = next_hour.get("details", {})
        summary = next_hour.get("summary", {})
        for name in (
            "air_temperature",
            "wind_from_direction",
            "wind_speed",
            "wind_speed_of_gust",
        ):
            counts[name] += int(name in instant)
        for name in ("precipitation_amount", "probability_of_precipitation"):
            counts[name] += int(name in details)
        counts["symbol_code"] += int("symbol_code" in summary)
    return counts


def validate_met(root: Path) -> dict[str, object]:
    met = root / "met"
    results: dict[str, object] = {}
    for location in ("oslo", "manila"):
        require_response(met / f"{location}.headers", 200, "application/json")
        source = met / f"{location}.json"
        document = json.loads(source.read_text(encoding="utf-8"))
        records = hourly_records(document)
        validate_hour_spacing(records, location)
        counts = met_field_counts(records)
        units = document["properties"]["meta"]["units"]
        for required_unit in (
            "air_temperature",
            "wind_from_direction",
            "wind_speed",
            "precipitation_amount",
        ):
            require(required_unit in units, f"{location}: missing unit {required_unit}")
        if location == "oslo":
            require(
                all(count == 24 for count in counts.values()),
                "Oslo schema is incomplete",
            )
            require(
                "wind_speed_of_gust" in units
                and "probability_of_precipitation" in units,
                "Oslo metadata lost required units",
            )
        else:
            require(
                counts["air_temperature"] == 24
                and counts["wind_from_direction"] == 24
                and counts["wind_speed"] == 24
                and counts["precipitation_amount"] == 24
                and counts["symbol_code"] == 24,
                "Manila core hourly fields are incomplete",
            )
            require(
                counts["wind_speed_of_gust"] < 24
                and counts["probability_of_precipitation"] < 24,
                "Manila now satisfies the worldwide schema; re-open the provider decision",
            )
        results[location] = {
            "sha256": sha256(source),
            "bytes": source.stat().st_size,
            "field_counts_first_24": counts,
            "provider_updated_at": document["properties"]["meta"]["updated_at"],
        }

    require_response(met / "conditional.headers", 304)
    conditional_body = met / "conditional.body"
    require(
        not conditional_body.exists() or conditional_body.stat().st_size == 0,
        "MET 304 response unexpectedly has a body",
    )
    return results


def validate_gfs(root: Path) -> dict[str, object]:
    gfs = root / "gfs"
    selected = (gfs / "selected-cycle.txt").read_text(encoding="utf-8").split()
    require(len(selected) == 2, "GFS selected cycle is malformed")
    hashes: dict[str, str] = {}
    total_bytes = 0
    for hour in range(1, 25):
        token = f"f{hour:03d}"
        require_response(gfs / f"{token}.headers", 200, "application/octet-stream")
        grib = gfs / f"{token}.grib2"
        payload = grib.read_bytes()
        require(payload.startswith(b"GRIB"), f"{token}: missing GRIB magic")
        require(payload.endswith(b"7777"), f"{token}: missing GRIB terminator")
        hashes[token] = sha256(grib)
        total_bytes += len(payload)

    repository = Path(__file__).resolve().parents[2]
    encoded_fixture = repository / (
        "companion-ios/Packages/OBCKit/Tests/OBCFormatsTests/Fixtures/"
        "gfs-manila-apcp-f006.grib2.b64"
    )
    fixture_bytes = base64.b64decode(encoded_fixture.read_text(encoding="utf-8"))
    require(
        hashlib.sha256(fixture_bytes).hexdigest() == EXPECTED_GFS_FIXTURE_SHA256,
        "stored GFS fixture hash changed",
    )
    return {
        "selected_cycle": selected,
        "file_count": 24,
        "total_bytes": total_bytes,
        "live_sha256": hashes,
        "fixture_sha256": EXPECTED_GFS_FIXTURE_SHA256,
    }


def main() -> None:
    require(len(sys.argv) == 2, "usage: validate_sources.py OUTPUT_DIRECTORY")
    root = Path(sys.argv[1]).resolve()
    require(root.is_dir(), f"missing evidence directory: {root}")
    report = {
        "validated_at": datetime.now(timezone.utc).isoformat(),
        "dwd": validate_dwd(root),
        "met": validate_met(root),
        "gfs": validate_gfs(root),
    }
    output = root / "validation.json"
    output.write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(f"Validated source contracts; report written to {output}")


if __name__ == "__main__":
    main()
