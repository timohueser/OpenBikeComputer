#!/usr/bin/env python3
"""The external freshness and cost probe for the OBC weather service.

    ops/weather/freshness_probe.py --url https://wx.openbikecomputer.com
    ops/weather/freshness_probe.py --manifest ./manifest.json --now 2026-08-09T18:00:00Z

It fetches (or reads) `wx/v2/manifest.json` — the manifest of the one canonical sharded dataset —
and answers two questions: **is what the service is serving right now still usable**, and **is it
still costing what it is supposed to cost**. Nothing about the VPS is consulted; that is the whole
point. The baker is a stateless publisher, so if it dies R2 keeps serving the last objects and the
product degrades honestly through staleness. The heartbeat is therefore the published manifest, and
the alarm lives somewhere the baker's death cannot silence it (WX18 runs it from GitHub Actions).

Checks, in the order they are reported:

  1. the manifest is fetchable and parses, and its `version` is one this probe understands;
  2. `generated_at` is no older than --max-manifest-age-min. With one dataset on one timer this is
     also the dead-timer check: there is no longer a fresh-manifest-but-dead-timer state to miss,
     because the timer that would be dead is the one that writes the manifest;
  3. the document's own deadlines — `freshness.next_generation_expected_at` (the service is late)
     and `freshness.stale_after` (this generation can no longer answer anything);
  4. presence: published shards against the grid the manifest states, the bitmap and the shard list
     agreeing, and — with --expect-sources — every source that should be in the mosaic still in
     `attribution[]`;
  5. cost guards: the published set, the retained footprint (set x generations retained), the
     cadence the document implies, and one HEAD proving the retention sweep is actually collecting.

No threshold here is invented locally where the document states one. The manifest carries its own
deadlines, its own grid, its own cadence and its own retention chain, so the probe compares them
against the clock and against the objects the document claims — which is the discipline the client
follows too, and the reason a cadence change is a baker deploy rather than an edit here.

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

MANIFEST_PATH = "wx/v2/manifest.json"
SUPPORTED_MANIFEST_VERSIONS = {2}
USER_AGENT = "obc-wx-freshness-probe/2"
BYTES_PER_MB = 1000 * 1000

# ── The cost model, and where every number in it comes from ──────────────────────────────────────
#
# WXR1 (#1254) measured one **wet global cycle** at the published lattice — 36,000 x 18,000 cells,
# 24 shards x 9 frames, tile edge 256, per-tile deflate — at **14.69 MB** of published objects.
# That is the worst case, not the average: a real cycle omits every all-dry shard.
#
# The quantity that matters is therefore `bytes per cycle x generations retained`. Retention is
# current plus two (OBCG_Spec.md §10.4), so a healthy bucket holds **3 x 14.69 = 44 MB**, and the
# baker's own sweep is what keeps it there. That is the whole storage story: not the 3-24 GB the
# 24 h lifecycle window used to hold, and not a projection from a cadence table.
#
# The two gates below are set against that measurement rather than against the old architecture:
#
#   --max-set-bytes        30 MB  ~2x the wet-global measurement. It catches "the dataset grew" —
#                                 a shard-size regression, a codec falling back to raw4, an adapter
#                                 painting noise into cells that should be dry.
#
# **There is deliberately only one storage gate.** The obvious second one — retained bytes against
# ~90 MB — cannot fire without the set gate firing first: `retained = set x (1 + len(previous))`,
# `len(previous)` is capped at 2 by OBCG §10.4, so the retained figure is bounded by 3 x the
# published set by construction. It is a restatement of the same measurement, not a second opinion,
# and a gate that arithmetically cannot fire on its own is a gate people learn to ignore. The
# retained figure is still *printed*, because it is the number the bucket actually holds.
#
# What that figure cannot see at all is a sweep that stopped: it is a projection of what the bucket
# holds *if the sweep is working*, and no arithmetic over a manifest can check that. Two things can,
# and both are here or in the runbook — `check_sweep`'s single HEAD against a generation the
# manifest no longer names, and T9's monthly look at R2's own stored-bytes figure.
#
# The mistake the old 1.5 GB gate existed to catch was a cadence fat-finger, and that mistake has
# changed shape rather than gone away. Generations are anchored on the quarter hour, so a timer
# firing every 2 minutes does not mint extra generations — it *re-bakes the same one*, seven times
# over, and storage never moves. What it burns is Class A operations: 217 writes a cycle at the
# intended 96 cycles/day is ~633 k/month against R2's 1 M free, so five times the cadence is five
# times over the free tier and the first real bill this service has ever produced. Storage cannot
# see that; `check_cadence` can, from a single sample — see its own comment.
DEFAULT_MAX_SET_BYTES = 30 * BYTES_PER_MB
# OBCG §10.4's cap, normative on readers as well as publishers: a document naming more previous
# generations than this disagrees with the publisher's sweep about what exists, and raising it is a
# manifest version bump rather than a configuration change.
RETAINED_PREVIOUS_GENERATIONS = 2


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


def parse_generation(text: str) -> datetime:
    """`YYYYMMDD'T'HHMM'Z'` — the generation identifier, which is a reference time to the minute."""
    return datetime.strptime(text, "%Y%m%dT%H%MZ").replace(tzinfo=timezone.utc)


def format_generation(stamp: datetime) -> str:
    return stamp.strftime("%Y%m%dT%H%MZ")


def age_text(delta: timedelta) -> str:
    minutes = int(delta.total_seconds() // 60)
    if abs(minutes) < 120:
        return f"{minutes} min"
    return f"{minutes / 60:.1f} h"


def fetch(url: str, timeout: float, method: str = "GET") -> tuple[bytes, str]:
    request = urllib.request.Request(url, method=method, headers={"User-Agent": USER_AGENT, "Cache-Control": "no-cache"})
    with urllib.request.urlopen(request, timeout=timeout) as response:  # noqa: S310 - fixed https URL
        return response.read(), response.headers.get("Cache-Control", "")


def manifest_url(base: str, path: str = MANIFEST_PATH) -> str:
    if base.endswith(".json"):
        return base
    return base.rstrip("/") + "/" + path


def object_url(base: str, key: str) -> str:
    """The public URL of one object key. `--url` may name the manifest directly, in which case the
    service origin is that URL minus the manifest path."""
    origin = base
    if origin.endswith(MANIFEST_PATH):
        origin = origin[: -len(MANIFEST_PATH)]
    elif origin.endswith(".json"):
        origin = origin.rsplit("/", 1)[0] + "/"
    return origin.rstrip("/") + "/" + key


# ─────────────────────────────────────────────────────────────────────────────────────────────────
# Checks
# ─────────────────────────────────────────────────────────────────────────────────────────────────


def check_deadlines(freshness, generation, now, grace, report, alerts, result) -> None:
    """3. The document's own deadlines. Nothing here is a duration this file chose."""
    due = parse_rfc3339(freshness["next_generation_expected_at"])
    stale_after = parse_rfc3339(freshness["stale_after"])
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
        report.append(
            f"ok       usable for another {age_text(stale_after - now)} "
            f"(next due {freshness['next_generation_expected_at']})"
        )


def check_presence(document, shard_count, report, alerts, result) -> int:
    """4. What the dataset actually published. Returns the published set size in bytes."""
    set_bytes = 0
    published = 0
    frame_rows = []
    frames = document["frames"]
    for frame in frames:
        try:
            shards = frame["shards"]
            bytes_now = sum(int(shard["bytes"]) for shard in shards)
            flagged = bin(int(frame["present"], 16)).count("1")
        except (KeyError, TypeError, ValueError) as error:
            alerts.append(f"a frame entry is malformed ({error})")
            continue
        # The bitmap and the list are one statement (OBCG §10.3). A client refuses a frame where they
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
    return set_bytes


def check_sources(document, expect, report, alerts, result) -> None:
    """4b. Every source that should be in the mosaic is still in `attribution[]`.

    It is worth being exact about what this can see. `attribution[]` names every source that *may
    have painted a cell* — it comes from the
    baker's priority table, not from a per-cell record — so this cannot detect an upstream outage;
    a source whose provider is down still appears, and the mosaic falls through to the next
    priority row, which is the designed behaviour and not an alert.

    What it does catch is a **deploy that went backwards**: an older binary rolled onto the box, or
    a build with a source accidentally dropped from `MOSAIC_PRIORITY`, silently degrades the
    dataset's resolution over whole regions while every freshness check stays green.
    """
    listed = [entry.get("source_id") for entry in document.get("attribution", [])]
    result["sources"] = listed
    if not expect:
        report.append(f"sources  {len(listed)} in the mosaic: {', '.join(str(name) for name in listed)}")
        return
    missing = [name for name in expect if name not in listed]
    for name in missing:
        alerts.append(
            f"{name} is not in the mosaic's attribution at all. An upstream outage leaves a source "
            "listed (the mosaic just falls through to the next priority row), so this is the binary, "
            "not the provider: check what `install.sh` last deployed, or drop it from "
            "--expect-sources if the removal was deliberate"
        )
        report.append(f"MISSING  {name} expected by --expect-sources, absent from attribution[]")
    if not missing:
        report.append(f"sources  all {len(expect)} expected sources present ({len(listed)} listed)")
    unexpected = sorted(set(str(name) for name in listed) - set(expect))
    if unexpected:
        report.append(f"note     listed but not expected: {', '.join(unexpected)}")


def check_cadence(document, cadence, args, report, alerts, result) -> None:
    """5b. The cadence guard: does the box publish at the cadence the dataset is defined at?

    One sample is enough, and the trick is worth writing down. `generated_at` is stamped from the
    instant the cycle *started*, and `reference_time` is that same instant floored to the cadence
    (`CycleTimes::anchored_at`), so their difference is neither bake time nor lateness — it is
    exactly **the phase of the timer within one step**. On the shipped `*:0/15` unit with
    `RandomizedDelaySec=60` that is 0-60 seconds, every single tick, forever.

    A timer firing faster does not mint extra generations — it re-bakes the same anchor — so
    storage never moves and nothing else in this probe can see it. What it does is spread that
    phase uniformly across the whole step, and burn a full object set of Class A writes per extra
    tick. At the intended cadence the service writes ~633 k of R2's 1 M free Class A operations a
    month, so the headroom for this mistake is 1.6x, and it is the tightest line in the budget.

    Hence the threshold: several times the unit's own jitter, comfortably under half a step. A
    correct timer cannot reach it; a five-times-too-fast one trips it on more than half of all
    samples, which at a 15-minute probe cadence means "within the hour".

    **One legitimate way to trip it, and it is in this repository's own runbook**: a bake started by
    hand mid-step (`systemctl start obc-wx-bake@cycle.service`) stamps whatever phase the operator
    happened to be at, which is often most of a step. That is not a false positive worth suppressing
    — it is a true statement about the manifest that is live — but it is self-clearing, so the alert
    says so rather than sending someone to read `list-timers` for nothing. The runbook's §8 rows say
    the same thing at the other end.
    """
    step_min = int(cadence["frame_step_min"])
    generated_at = parse_rfc3339(document["generated_at"])
    reference_time = parse_rfc3339(document["reference_time"])
    phase = generated_at - reference_time
    result["timer_phase_min"] = round(phase.total_seconds() / 60, 1)
    if phase >= timedelta(minutes=args.max_timer_phase_min):
        alerts.append(
            f"the cycle started {age_text(phase)} into its own {step_min}-minute step (limit "
            f"{args.max_timer_phase_min} min). The shipped timer fires on the step boundary with at most "
            "60 s of randomized delay, so this is usually a timer running faster than the cadence: every "
            "extra tick rewrites the whole object set, and Class A operations are the tightest line in "
            "the budget. Check `systemctl list-timers 'obc-wx-bake@*'` against ops/weather/adapters.conf. "
            "If someone just ran a bake by hand (`systemctl start obc-wx-bake@cycle.service` — the "
            "runbook asks for one in several recovery steps), this is that, and it clears on the next "
            "scheduled tick"
        )
        report.append(f"CADENCE  the cycle started {age_text(phase)} into a {step_min} min step")
    elif phase < timedelta(0):
        report.append(f"note     generated_at precedes reference_time by {age_text(-phase)} — check the box's clock")
    else:
        report.append(f"ok       cycle started {age_text(phase)} into its {step_min} min step")


def check_sweep(document, base_url, timeout, report, alerts, result) -> None:
    """5c. Is the retention sweep actually collecting? One HEAD, no listing, no credentials.

    The sweep (WXR8 #1247) is a new failure mode: before it, the baker provably never deleted
    anything, and storage was bounded by a lifecycle rule instead. A sweep that silently stops
    working looks *exactly* like a healthy service from the manifest — the retention chain still
    says "current plus two" — and the bill is the only other witness, a month later.

    So the probe checks it from outside, the way a client would. The generation one step older than
    the oldest one the manifest still names is a generation the sweep was supposed to collect, and
    its south-west shard at f0 is the one object guaranteed to have existed: shard row 0 reaches
    below `covered_rows.start`, so it always holds no-data cells, and a shard with a single no-data
    cell is never omitted as dry (OBCG §10.3). If that object answers 200, nothing is being swept.

    It fails **safe** in every ambiguous direction: a gap in publishing (the box was down) names a
    generation that never existed, and a 404 passes; a service that has not yet published three
    generations is skipped entirely; a re-cut grid is skipped by the `covered_rows` test below.
    """
    previous = document["previous_generations"]
    lattice = document["lattice"]
    if len(previous) < 2:
        report.append("sweep    skipped: fewer than two previous generations, nothing has fallen off the chain yet")
        return
    if not int(lattice["covered_rows"]["start"]) > 0:
        report.append("sweep    skipped: shard row 0 is fully covered, so it has no guaranteed-published object")
        return
    swept = format_generation(parse_generation(previous[-1]) - timedelta(minutes=int(document["cadence"]["frame_step_min"])))
    key = f"{document['key_prefix']}/{swept}/f0/s0-0.obcg"
    result["sweep_probe_key"] = key
    try:
        fetch(object_url(base_url, key), timeout, method="HEAD")
    except urllib.error.HTTPError as error:
        if error.code in (403, 404):
            report.append(f"ok       swept: {swept} is gone (probed {key.rsplit('/', 2)[0]}/f0/s0-0.obcg)")
            return
        report.append(f"note     sweep check inconclusive: HTTP {error.code} for {key}")
        return
    except (urllib.error.URLError, TimeoutError, OSError) as error:
        report.append(f"note     sweep check inconclusive: {error}")
        return
    alerts.append(
        f"generation {swept} is still fetchable, and the manifest no longer names it. The retention "
        "sweep is not collecting: storage grows by a full object set every cycle, and the only other "
        "thing bounding it is the bucket's 1-day lifecycle backstop. Read the last cycles' journal "
        "for `retention sweep:` warnings (ops/weather/RUNBOOK.md §7)"
    )
    report.append(f"SWEEP    {key} is still there; the manifest stopped naming {swept}")


def check_cost(set_bytes, previous, args, report, alerts, result) -> None:
    """5. Storage. See the model at the top of this file for where the one gate comes from, and why
    the retained figure beside it is reported rather than gated."""
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
            f"{args.max_set_bytes / BYTES_PER_MB:.0f} MB guard — WXR1 measured a *wet global* cycle at "
            "14.7 MB, so this is a dataset that grew, not weather. The retained footprint is three "
            f"times it, {retained_bytes / BYTES_PER_MB:.0f} MB"
        )
    # The one thing about retention a manifest *can* state wrongly, and it is a spec violation
    # rather than a budget question: a chain longer than the cap means the document and the sweep
    # disagree about which generations exist, and §10.4 says a reader rejects it rather than
    # truncating. The baker refuses to publish one; this is the outside check that it did.
    if len(previous) > RETAINED_PREVIOUS_GENERATIONS:
        alerts.append(
            f"the manifest names {len(previous)} previous generations; OBCG §10.4 caps it at "
            f"{RETAINED_PREVIOUS_GENERATIONS} and the cap is normative. The document and the retention "
            "sweep now disagree about which generations exist, which is how a client gets a 404 on a "
            "generation the manifest promised"
        )


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


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--url", help="service base URL (or a full manifest URL)")
    source.add_argument("--manifest", type=Path, help="read a local manifest instead of fetching")
    parser.add_argument("--now", help="RFC 3339 override for the current time (drills and tests)")
    parser.add_argument("--max-manifest-age-min", type=int, default=30,
                        help="alert when generated_at is older than this (default: 30)")
    parser.add_argument("--grace-min", type=int, default=15,
                        help="grace added to the document's own deadlines (default: 15)")
    parser.add_argument("--expect-sources", default="",
                        help="comma-separated mosaic source ids that must appear in attribution[] "
                             "(e.g. dwd-rv,mrms,gfs); catches a deploy that went backwards")
    parser.add_argument("--max-set-bytes", type=int, default=DEFAULT_MAX_SET_BYTES,
                        help="alert when the currently published generation exceeds this (default: 30 MB)")
    parser.add_argument("--max-timer-phase-min", type=int, default=7,
                        help="alert when a cycle started this far into its own cadence step, which only a "
                             "too-fast timer can do (default: 7)")
    parser.add_argument("--no-sweep-check", action="store_true",
                        help="skip the HEAD that proves the retention sweep is collecting")
    parser.add_argument("--timeout", type=float, default=20.0)
    parser.add_argument("--json", action="store_true", help="emit a machine-readable summary too")
    # Retired in round 1 of #1274's review: `retained = set x (1 + len(previous))` with `len` capped
    # at 2, so it could never fire without --max-set-bytes firing first. The figure is still
    # reported; only the gate is gone. Both spellings still parse so no invocation breaks.
    parser.add_argument("--max-retained-bytes", "--max-rolling-bytes", type=int, dest="max_retained_bytes",
                        default=None, help=argparse.SUPPRESS)
    return parser


def probe(args) -> int:
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
            raw, cache_control = fetch(where, args.timeout)
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

    try:
        generation = document["generation"]
        previous = document["previous_generations"]
        lattice = document["lattice"]
        cadence = document["cadence"]
        shard_count = int(lattice["shard_cols"]) * int(lattice["shard_rows"])
        freshness = document["freshness"]
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

    # ── 2. Manifest age: the heartbeat, and with one timer also the dead-timer check ───────────
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

    # ── 3-5. The document's own deadlines, what it published, and what it costs ────────────────
    check_deadlines(freshness, generation, now, timedelta(minutes=args.grace_min), report, alerts, result)
    set_bytes = check_presence(document, shard_count, report, alerts, result)
    check_sources(document, [name.strip() for name in args.expect_sources.split(",") if name.strip()],
                  report, alerts, result)
    check_cost(set_bytes, previous, args, report, alerts, result)
    try:
        check_cadence(document, cadence, args, report, alerts, result)
    except (KeyError, TypeError, ValueError) as error:
        report.append(f"note     cadence check skipped: {error}")
    if args.url and not args.no_sweep_check:
        try:
            check_sweep(document, args.url, args.timeout, report, alerts, result)
        except (KeyError, TypeError, ValueError) as error:
            report.append(f"note     sweep check skipped: {error}")
    if args.max_retained_bytes is not None:
        report.append("note     --max-retained-bytes is accepted and ignored: it could never fire on its own; see --max-set-bytes")

    return emit(where, now, report, alerts, result, args.json)


def main() -> int:
    return probe(build_parser().parse_args())


if __name__ == "__main__":
    sys.exit(main())
