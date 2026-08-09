#!/usr/bin/env python3
"""The external freshness probe for the OBC weather service.

    ops/weather/freshness_probe.py --url https://wx.openbikecomputer.com
    ops/weather/freshness_probe.py --manifest ./manifest.json --now 2026-08-09T18:00:00Z

It fetches (or reads) `wx/v1/manifest.json` and answers one question: **is what the service is
serving right now still usable?** Nothing about the VPS is consulted — that is the whole point.
The baker is a stateless publisher; if it dies, R2 keeps serving the last objects and the product
degrades honestly through staleness. So the heartbeat is the published manifest, and the alarm
must live somewhere the baker's death cannot silence it (WX18 runs it from GitHub Actions).

Checks, in the order they are reported:

  1. the manifest is fetchable and parses, and its `version` is one this probe understands;
  2. `generated_at` is no older than --max-manifest-age-min (the epic's 30-minute rule: even with
     every upstream unchanged, a healthy box republishes the manifest on every tick);
  3. every product is inside its own `staleness_deadline` plus --grace-min;
  4. every product named by --expect is actually listed. A product only ever leaves the manifest
     when a human retires its adapter, so an absent one means its *timer* died — disabled,
     renamed, never installed — rather than its upstream. That failure otherwise looks exactly
     like health: everything still listed keeps publishing happily without it;
  5. cost guardrails: the current published set, and the projected 48 h rolling bucket footprint
     (each product's current bytes x its upstream runs/day from adapters.conf x 2 days).

Exit codes: 0 fresh, 1 stale or over budget, 2 the manifest could not be read at all. 1 and 2 are
both alerts — a manifest that cannot be fetched is an outage of exactly the thing riders read —
but they are worth telling apart in the alert text.

Standard library only; it must run on any runner, any box, with no install step.
"""

from __future__ import annotations

import argparse
import json
import sys
import urllib.error
import urllib.request
from datetime import datetime, timedelta, timezone
from pathlib import Path

MANIFEST_PATH = "wx/v1/manifest.json"
SUPPORTED_MANIFEST_VERSIONS = {1}
USER_AGENT = "obc-wx-freshness-probe/1"
BYTES_PER_MB = 1000 * 1000


def parse_rfc3339(text: str) -> datetime:
    """RFC 3339 as the baker writes it (`2026-08-09T14:30:00Z`)."""
    if not isinstance(text, str):
        raise ValueError(f"not a timestamp: {text!r}")
    normalized = text.strip()
    if normalized.endswith(("Z", "z")):
        normalized = normalized[:-1] + "+00:00"
    stamp = datetime.fromisoformat(normalized)
    if stamp.tzinfo is None:
        raise ValueError(f"timestamp without a zone: {text!r}")
    return stamp.astimezone(timezone.utc)


def age_text(delta: timedelta) -> str:
    minutes = int(delta.total_seconds() // 60)
    if abs(minutes) < 120:
        return f"{minutes} min"
    return f"{minutes / 60:.1f} h"


def read_runs_per_day(path: Path) -> dict[str, int]:
    """The `runs/day` column of adapters.conf — upstream publications, not poll ticks."""
    runs: dict[str, int] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        fields = line.split()
        if len(fields) < 4 or not fields[3].isdigit():
            continue
        runs[fields[0]] = int(fields[3])
    return runs


def fetch_manifest(url: str, timeout: float) -> tuple[bytes, str]:
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT, "Cache-Control": "no-cache"})
    with urllib.request.urlopen(request, timeout=timeout) as response:  # noqa: S310 - fixed https URL
        return response.read(), response.headers.get("Cache-Control", "")


def manifest_url(base: str) -> str:
    if base.endswith(".json"):
        return base
    return base.rstrip("/") + "/" + MANIFEST_PATH


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--url", help="service base URL (or a full manifest URL)")
    source.add_argument("--manifest", type=Path, help="read a local manifest instead of fetching")
    parser.add_argument("--now", help="RFC 3339 override for the current time (drills and tests)")
    parser.add_argument("--max-manifest-age-min", type=int, default=30,
                        help="alert when generated_at is older than this (default: 30)")
    parser.add_argument("--grace-min", type=int, default=15,
                        help="grace added to each product's staleness_deadline (default: 15)")
    parser.add_argument("--expect", default="",
                        help="comma-separated product ids that must be listed (e.g. dwd-rv,icon-eu); "
                             "catches a dead timer, which no freshness check can see")
    parser.add_argument("--max-set-bytes", type=int, default=50 * BYTES_PER_MB,
                        help="alert when the currently published frame set exceeds this (default: 50 MB)")
    parser.add_argument("--max-rolling-bytes", type=int, default=1000 * BYTES_PER_MB,
                        help="alert when the projected 48 h bucket footprint exceeds this (default: 1 GB)")
    parser.add_argument("--adapters-conf", type=Path, default=Path(__file__).with_name("adapters.conf"),
                        help="cadence table used for the rolling-storage projection")
    parser.add_argument("--timeout", type=float, default=20.0)
    parser.add_argument("--json", action="store_true", help="emit a machine-readable summary too")
    args = parser.parse_args()

    now = parse_rfc3339(args.now) if args.now else datetime.now(timezone.utc)
    report: list[str] = []
    alerts: list[str] = []
    result: dict[str, object] = {"checked_at": now.isoformat().replace("+00:00", "Z")}

    # ── 1. Read it ────────────────────────────────────────────────────────────────────────────
    if args.manifest:
        where = str(args.manifest)
        try:
            raw = args.manifest.read_bytes()
        except OSError as error:
            print(f"UNREACHABLE  {where}: {error}")
            return 2
        cache_control = ""
    else:
        where = manifest_url(args.url)
        try:
            raw, cache_control = fetch_manifest(where, args.timeout)
        except (urllib.error.URLError, urllib.error.HTTPError, TimeoutError, OSError) as error:
            print(f"UNREACHABLE  {where}: {error}")
            print("The manifest is what every rider's phone reads first. Unreachable is an outage of the")
            print("whole weather product, whether the cause is the bucket, DNS, or the public-access config.")
            return 2

    result["source"] = where
    try:
        document = json.loads(raw)
        version = document["version"]
        generated_at = parse_rfc3339(document["generated_at"])
        products = document["products"]
        if not isinstance(products, list):
            raise TypeError("products is not a list")
    except (json.JSONDecodeError, KeyError, TypeError, ValueError) as error:
        print(f"UNREADABLE   {where}: {error}")
        print(f"({len(raw)} bytes fetched; a corrupt or truncated manifest is as bad as a missing one.)")
        return 2

    if version not in SUPPORTED_MANIFEST_VERSIONS:
        alerts.append(f"manifest version {version} is not one this probe understands {sorted(SUPPORTED_MANIFEST_VERSIONS)}")

    # ── 2. Manifest age ───────────────────────────────────────────────────────────────────────
    manifest_age = now - generated_at
    result["generated_at"] = document["generated_at"]
    result["manifest_age_min"] = round(manifest_age.total_seconds() / 60, 1)
    if manifest_age > timedelta(minutes=args.max_manifest_age_min):
        alerts.append(
            f"the manifest is {age_text(manifest_age)} old (limit {args.max_manifest_age_min} min) — "
            "every timer tick republishes it, so nothing has run on the box"
        )
        report.append(f"STALE    manifest generated_at {document['generated_at']} ({age_text(manifest_age)} ago)")
    elif manifest_age < timedelta(minutes=-5):
        alerts.append(f"the manifest is stamped {age_text(-manifest_age)} in the future — check the box's clock")
    else:
        report.append(f"ok       manifest generated_at {document['generated_at']} ({age_text(manifest_age)} ago)")
    if cache_control and "max-age" not in cache_control:
        report.append(f"note     manifest Cache-Control is {cache_control!r} (the baker asks for max-age=60)")

    # ── 3. Per-product staleness ──────────────────────────────────────────────────────────────
    grace = timedelta(minutes=args.grace_min)
    set_bytes = 0
    rolling_bytes = 0
    runs_per_day: dict[str, int] = {}
    if args.adapters_conf.exists():
        runs_per_day = read_runs_per_day(args.adapters_conf)
    product_rows = []

    if not products:
        alerts.append("the manifest lists no products at all — the service is serving nothing")

    for product in products:
        try:
            identifier = product["id"]
            deadline = parse_rfc3339(product["staleness_deadline"])
            frames = product["frames"]
            bytes_now = sum(int(frame["bytes"]) for frame in frames)
        except (KeyError, TypeError, ValueError) as error:
            alerts.append(f"a product entry is malformed ({error})")
            continue
        set_bytes += bytes_now
        runs = runs_per_day.get(identifier)
        rolling_bytes += bytes_now * (runs * 2 if runs else 2)
        left = deadline - now
        row = {
            "id": identifier,
            "tier": product.get("tier"),
            "frames": len(frames),
            "bytes": bytes_now,
            "staleness_deadline": product["staleness_deadline"],
            "minutes_left": round(left.total_seconds() / 60, 1),
        }
        product_rows.append(row)
        if now > deadline + grace:
            alerts.append(
                f"{identifier} (tier {product.get('tier')}) went stale {age_text(now - deadline)} ago "
                f"— deadline {product['staleness_deadline']}, grace {args.grace_min} min"
            )
            report.append(f"STALE    {identifier:10s} deadline {product['staleness_deadline']} ({age_text(now - deadline)} past)")
        elif now > deadline:
            report.append(f"grace    {identifier:10s} deadline {product['staleness_deadline']} ({age_text(now - deadline)} past, inside grace)")
        else:
            report.append(f"ok       {identifier:10s} {len(frames):3d} frames, {bytes_now / 1000:8.1f} kB, {age_text(left)} of validity left")

    # ── 4. Product presence ───────────────────────────────────────────────────────────────────
    expected = [name.strip() for name in args.expect.split(",") if name.strip()]
    listed = {row["id"] for row in product_rows}
    result["expected"] = expected
    for name in expected:
        if name in listed:
            continue
        alerts.append(
            f"{name} is not in the manifest at all. An outage leaves a product listed and expired, so "
            "this is its timer, not its upstream: check `systemctl list-timers 'obc-wx-bake@*'` and "
            "ops/weather/adapters.conf (or drop it from --expect if the removal was deliberate)"
        )
        report.append(f"MISSING  {name:10s} expected by --expect, absent from the manifest")
    if expected:
        unexpected = sorted(listed - set(expected))
        if unexpected:
            report.append(f"note     listed but not expected: {', '.join(unexpected)}")

    # ── 5. Cost guardrails ────────────────────────────────────────────────────────────────────
    result["published_set_bytes"] = set_bytes
    result["projected_rolling_bytes"] = rolling_bytes
    report.append(f"cost     published set {set_bytes / BYTES_PER_MB:.2f} MB; projected 48 h bucket {rolling_bytes / BYTES_PER_MB:.0f} MB")
    if set_bytes > args.max_set_bytes:
        alerts.append(
            f"the published set is {set_bytes / BYTES_PER_MB:.1f} MB, over the {args.max_set_bytes / BYTES_PER_MB:.0f} MB guard — "
            "a product got much bigger, and storage/egress scale with it"
        )
    if rolling_bytes > args.max_rolling_bytes:
        alerts.append(
            f"the projected 48 h bucket footprint is {rolling_bytes / BYTES_PER_MB:.0f} MB, over the "
            f"{args.max_rolling_bytes / BYTES_PER_MB:.0f} MB budget (WX1's rolling-window gate)"
        )
    missing_cadence = [row["id"] for row in product_rows if row["id"] not in runs_per_day]
    if missing_cadence:
        report.append(f"note     no adapters.conf cadence for {', '.join(missing_cadence)} — projected at 1 run/day")

    result["alerts"] = alerts
    result["products"] = product_rows

    print(f"OBC weather freshness probe — {where}")
    print(f"checked {now.isoformat().replace('+00:00', 'Z')}")
    print()
    for line in report:
        print(line)
    if alerts:
        print()
        print("ALERTS")
        for alert in alerts:
            print(f"  - {alert}")
    if args.json:
        print()
        print(json.dumps(result, indent=2, sort_keys=True))
    return 1 if alerts else 0


if __name__ == "__main__":
    sys.exit(main())
