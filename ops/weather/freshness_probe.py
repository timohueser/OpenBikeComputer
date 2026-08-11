#!/usr/bin/env python3
"""The external freshness probe for the OBC weather service.

    ops/weather/freshness_probe.py --url https://wx.openbikecomputer.com
    ops/weather/freshness_probe.py --manifest ./manifest.json --now 2026-08-09T18:00:00Z

It fetches (or reads) the service manifest — `wx/v1/manifest.json`, or `wx/v2/manifest.json`
with `--mosaic` — and answers one question: **is what the service is serving right now still usable?**

**Mid-cutover, 2026-08-11 (#1246 -> WXR8 #1247).**
The v1 half of this probe outlives the v1 *baker*, deliberately. #1246 deleted the multi-product
path from the code, but the deployed VPS is still publishing `wx/v1` and clients are still reading
it, and a probe that could not read what is actually being served would go blind for exactly the
window a cutover most needs watching. It goes when the tree does — WXR8 #1247's last step. Until
then, product ids, tiers and staleness deadlines appear below because that is the shape of the
document this probe is reading, not because the bakery still has such a concept.
Nothing about the VPS is consulted — that is the whole point.
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
  5. cost guardrails: the current published set, and the projected rolling bucket footprint
     (each product's current bytes x its upstream runs/day from adapters.conf), modelled over the
     **24 h** lifecycle the bucket actually has (RUNBOOK T4). It used to assume 48 h and so
     reported twice the truth, which meant the gate had to be read through a mental halving —
     exactly the kind of number people learn to ignore.

Manifest v2 (WXR4 #1243) is one sharded dataset, not a product list, so checks 3-5 change shape:
there is nothing to expire product by product, and the deadlines are stated *in the document* rather
than assumed here. The probe reads `freshness.next_generation_expected_at` (the service is late) and
`freshness.stale_after` (the published generation can no longer answer anything), counts published
shards against the grid the manifest states, and sizes the bucket from the retention contract —
current plus `previous_generations`, which is exactly the set the sweep is allowed to keep.

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
MANIFEST_PATH_V2 = "wx/v2/manifest.json"
SUPPORTED_MANIFEST_VERSIONS = {1, 2}
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


def manifest_url(base: str, path: str) -> str:
    if base.endswith(".json"):
        return base
    return base.rstrip("/") + "/" + path


def finish_v2(document, now, args, where, report, alerts, result) -> int:
    """Checks 3-5 for the canonical sharded dataset (manifest v2, WXR4 #1243).

    Nothing here is a threshold this file invented. The manifest states its own deadlines, its own
    grid and its own retention chain, so the probe's job is to compare them against the clock and
    against the objects the document claims — which is the same discipline the client follows, and
    the reason a cadence change is a baker deploy rather than an edit here.
    """
    grace = timedelta(minutes=args.grace_min)
    try:
        generation = document["generation"]
        previous = document["previous_generations"]
        lattice = document["lattice"]
        shard_count = int(lattice["shard_cols"]) * int(lattice["shard_rows"])
        freshness = document["freshness"]
        due = parse_rfc3339(freshness["next_generation_expected_at"])
        stale_after = parse_rfc3339(freshness["stale_after"])
        frames = document["frames"]
        if not isinstance(frames, list) or not frames or not isinstance(previous, list):
            raise TypeError("frames/previous_generations")
    except (KeyError, TypeError, ValueError) as error:
        print(f"UNREADABLE   {where}: v2 document is missing {error}")
        return 2

    result["generation"] = generation
    result["previous_generations"] = previous
    kept = ", ".join(previous) or "none"
    report.append(f"ok       generation {generation}, keeping {len(previous)} previous ({kept})")

    # ── 3. The document's own deadlines ───────────────────────────────────────────────────────
    result["stale_after"] = freshness["stale_after"]
    result["minutes_left"] = round((stale_after - now).total_seconds() / 60, 1)
    if now > stale_after + grace:
        alerts.append(
            f"the published generation went unusable {age_text(now - stale_after)} ago — its last frame was "
            f"valid at {freshness['stale_after']}, and no newer generation replaced it. Riders have no "
            "weather at all, which is not the same as no rain"
        )
        report.append(f"STALE    generation {generation} unusable since {freshness['stale_after']}")
    elif now > due + grace:
        alerts.append(
            f"the next generation was due at {freshness['next_generation_expected_at']} and is "
            f"{age_text(now - due)} late — the data is still usable, but nothing is baking"
        )
        report.append(f"LATE     next generation due {freshness['next_generation_expected_at']}")
    else:
        report.append(f"ok       usable for another {age_text(stale_after - now)} (next due {freshness['next_generation_expected_at']})")

    # ── 4. Presence: what the dataset actually published ──────────────────────────────────────
    set_bytes = 0
    published = 0
    frame_rows = []
    for frame in frames:
        try:
            shards = frame["shards"]
            bytes_now = sum(int(shard["bytes"]) for shard in shards)
            flagged = bin(int(frame["present"], 16)).count("1")
        except (KeyError, TypeError, ValueError) as error:
            alerts.append(f"a frame entry is malformed ({error})")
            continue
        # The bitmap and the list are one statement (OBCG 10.3). A client refuses a frame where they
        # disagree, which means the rider silently loses that frame; the probe is the one place that
        # can see it happening to everyone at once.
        if flagged != len(shards):
            alerts.append(
                f"f{frame.get('offset_min')}: the presence bitmap flags {flagged} shards and the list "
                f"has {len(shards)} - clients refuse a frame where those disagree, so it reaches nobody"
            )
        set_bytes += bytes_now
        published += len(shards)
        frame_rows.append({"offset_min": frame.get("offset_min"), "shards": len(shards), "bytes": bytes_now})
    result["frames"] = frame_rows
    expected = shard_count * len(frames)
    report.append(f"shards   {published} of {expected} published; the rest are dry, not missing")
    if published == 0:
        alerts.append(
            "not one shard of any frame is published. A dry planet is not a thing that happens: the "
            "mosaic publishes a no-data object wherever it has no source, so this is a broken bake"
        )

    # ── 5. Cost guardrails ────────────────────────────────────────────────────────────────────
    # The bucket holds exactly what the retention contract names: this generation plus the ones it
    # lists. That is the number to gate, and it is derivable from the document instead of modelled
    # from a cadence table, because retention is now stated rather than inferred.
    retained_bytes = set_bytes * (1 + len(previous))
    result["published_set_bytes"] = set_bytes
    result["retained_bytes"] = retained_bytes
    report.append(
        f"cost     published set {set_bytes / BYTES_PER_MB:.2f} MB; retained "
        f"{1 + len(previous)} generations = {retained_bytes / BYTES_PER_MB:.0f} MB"
    )
    if set_bytes > args.max_set_bytes:
        alerts.append(
            f"the published set is {set_bytes / BYTES_PER_MB:.1f} MB, over the "
            f"{args.max_set_bytes / BYTES_PER_MB:.0f} MB guard"
        )
    if retained_bytes > args.max_rolling_bytes:
        alerts.append(
            f"the retained set is {retained_bytes / BYTES_PER_MB:.0f} MB, over the "
            f"{args.max_rolling_bytes / BYTES_PER_MB:.0f} MB budget — either the dataset grew or the "
            "sweep is not collecting generations this manifest no longer names"
        )
    if args.expect:
        report.append("note     --expect names products, and v2 has none; ignored")

    return emit(where, now, report, alerts, result, args.json)


def emit(where, now, report, alerts, result, as_json) -> int:
    result["alerts"] = alerts
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
    if as_json:
        print()
        print(json.dumps(result, indent=2, sort_keys=True))
    return 1 if alerts else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--url", help="service base URL (or a full manifest URL)")
    source.add_argument("--manifest", type=Path, help="read a local manifest instead of fetching")
    parser.add_argument("--mosaic", action="store_true",
                        help=f"probe the canonical sharded dataset at {MANIFEST_PATH_V2} instead of the "
                             "multi-product tree")
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
    # 1.5 GB against a real 24 h window. WX1 wrote 1 GB when the model assumed 48 h — i.e. a real
    # budget of 0.5 GB — and only two adapters were live. Four adapters at their current cadences
    # project ≈ 0.95 GB of genuine daily churn, so 1 GB would now fire on healthy operation, but a
    # gate is only worth having if it still catches the mistakes it exists for. The one that
    # matters is a cadence fat-finger: `us` at `*:0/2` instead of `*:0/5` burns 5x the intended
    # churn and lands at ≈ 1.95 GB, which a 3 GB gate would wave through and this one does not.
    # 1.5 GB keeps ≈ 58 % headroom over healthy operation and stays far under R2's 10 GB free
    # tier. Raise it deliberately, with a number, when a new product genuinely needs the room.
    parser.add_argument("--max-rolling-bytes", type=int, default=1500 * BYTES_PER_MB,
                        help="alert when the projected 24 h bucket footprint exceeds this (default: 1.5 GB)")
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
        where = manifest_url(args.url, MANIFEST_PATH_V2 if args.mosaic else MANIFEST_PATH)
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
        # v2 has no products; its shape is checked inside check_v2.
        products = document["products"] if version < 2 else []
        if not isinstance(products, list):
            raise TypeError("products is not a list")
    except (json.JSONDecodeError, KeyError, TypeError, ValueError) as error:
        print(f"UNREADABLE   {where}: {error}")
        print(f"({len(raw)} bytes fetched; a corrupt or truncated manifest is as bad as a missing one.)")
        return 2

    if version not in SUPPORTED_MANIFEST_VERSIONS:
        # Alert and stop. Running v2's checks over a v3 document would report on fields whose meaning
        # it guessed, which is a worse answer than "I do not understand this".
        print(f"OBC weather freshness probe - {where}")
        print(f"UNSUPPORTED  manifest version {version}, not one of {sorted(SUPPORTED_MANIFEST_VERSIONS)}")
        print("A probe that interprets a document it does not understand reports confident nonsense.")
        return 1

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

    if version >= 2:
        return finish_v2(document, now, args, where, report, alerts, result)

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
        rolling_bytes += bytes_now * (runs if runs else 1)
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
    report.append(f"cost     published set {set_bytes / BYTES_PER_MB:.2f} MB; projected 24 h bucket {rolling_bytes / BYTES_PER_MB:.0f} MB")
    if set_bytes > args.max_set_bytes:
        alerts.append(
            f"the published set is {set_bytes / BYTES_PER_MB:.1f} MB, over the {args.max_set_bytes / BYTES_PER_MB:.0f} MB guard — "
            "a product got much bigger, and storage/egress scale with it"
        )
    if rolling_bytes > args.max_rolling_bytes:
        alerts.append(
            f"the projected 24 h bucket footprint is {rolling_bytes / BYTES_PER_MB:.0f} MB, over the "
            f"{args.max_rolling_bytes / BYTES_PER_MB:.0f} MB budget (the rolling-window gate)"
        )
    missing_cadence = [row["id"] for row in product_rows if row["id"] not in runs_per_day]
    if missing_cadence:
        report.append(f"note     no adapters.conf cadence for {', '.join(missing_cadence)} — projected at 1 run/day")

    result["products"] = product_rows
    return emit(where, now, report, alerts, result, args.json)


if __name__ == "__main__":
    sys.exit(main())
