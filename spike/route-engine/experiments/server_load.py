"""Bounded HTTP route load with separate warmup and optional health monitoring."""

import argparse
from collections import Counter
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
import gzip
import hashlib
import io
import json
import math
from pathlib import Path
import socket
import threading
import time
from urllib.error import HTTPError, URLError
from urllib.parse import urlparse
from urllib.request import Request, urlopen


def percentiles(samples):
    ordered = sorted(samples)
    if not ordered:
        return {key: None for key in ("p50", "p95", "p99")}
    return {key: ordered[max(0, math.ceil(len(ordered) * fraction) - 1)]
            for key, fraction in (("p50", 0.5), ("p95", 0.95), ("p99", 0.99))}


def request_route(url, fixture, timeout):
    started = time.perf_counter()
    result = {"status": "error", "response_body_bytes": 0,
              "request_body_bytes": len(fixture["body"])}
    try:
        request = Request(url, data=fixture["body"],
                          headers={"Content-Type": "application/json", "Accept-Encoding": "gzip"}, method="POST")
        try:
            response = urlopen(request, timeout=timeout)
        except HTTPError as error:
            response = error
        with response:
            result["http_status"] = response.code
            compressed = response.headers.get("Content-Encoding", "").lower() == "gzip"
            body = response.read(64 * 1024 * 1024 + 1)
        result["response_body_bytes"] = len(body)
        if len(body) > 64 * 1024 * 1024:
            result["status"] = "oversized_response"
            return result
        if response.code in (429, 503):
            result["status"] = "busy"
            return result
        if compressed:
            with gzip.GzipFile(fileobj=io.BytesIO(body)) as stream:
                body = stream.read(64 * 1024 * 1024 + 1)
            if len(body) > 64 * 1024 * 1024:
                result["status"] = "oversized_decoded_response"
                return result
        try:
            data = json.loads(body)
        except (ValueError, UnicodeDecodeError):
            result["status"] = "invalid_json"
            return result
        if not isinstance(data, dict):
            result["status"] = "invalid_response"
        elif data.get("kind") == "busy":
            result["status"] = "busy"
        elif response.code != 200 or data.get("kind") != "done":
            result["status"] = "route_error"
        elif not all(key in data for key in ("cost", "roads", "geometry", "totals")):
            result["status"] = "incomplete_response"
        elif "expect_cost" in fixture and data["cost"] != fixture["expect_cost"]:
            result["status"] = "cost_mismatch"
        else:
            canonical = json.dumps({key: data[key] for key in ("cost", "roads", "geometry", "totals")},
                                   sort_keys=True, separators=(",", ":"), allow_nan=False).encode()
            result["fingerprint"] = hashlib.sha256(canonical).hexdigest()
            result["status"] = "success"
    except (TimeoutError, socket.timeout):
        result["status"] = "timeout"
    except URLError as error:
        result["status"] = "timeout" if isinstance(error.reason, (TimeoutError, socket.timeout)) else "connection_error"
    except (OSError, ValueError):
        result["status"] = "error"
    finally:
        result["wall_ms"] = (time.perf_counter() - started) * 1000
    return result


def health_sample(url, timeout):
    started = time.perf_counter()
    status = None
    try:
        with urlopen(url, timeout=timeout) as response:
            status = response.code
            response.read(1024 * 1024)
        ok = status == 200
    except HTTPError as error:
        status, ok = error.code, False
        error.close()
    except (OSError, URLError):
        ok = False
    return {"ok": ok, "http_status": status, "wall_ms": (time.perf_counter() - started) * 1000}


def phase(url, fixtures, count, concurrency, timeout, abort, fingerprints):
    started = time.perf_counter()
    statuses, http_statuses = Counter(), Counter()
    per_fixture = [{"name": fixture["name"], "statuses": Counter(), "successful_ms": []}
                   for fixture in fixtures]
    successful_ms = []
    response_bytes = request_bytes = submitted = 0
    with ThreadPoolExecutor(max_workers=concurrency) as pool:
        pending = {}

        def submit():
            nonlocal submitted
            index = submitted % len(fixtures)
            pending[pool.submit(request_route, url, fixtures[index], timeout)] = index
            submitted += 1

        while submitted < min(count, concurrency) and not abort.is_set():
            submit()
        while pending:
            completed, _ = wait(pending, return_when=FIRST_COMPLETED)
            for future in completed:
                index = pending.pop(future)
                result = future.result()
                if result["status"] == "success":
                    key = fixtures[index]["request_hash"]
                    fingerprint = result["fingerprint"]
                    if key in fingerprints and fingerprints[key]["sha256"] != fingerprint:
                        result["status"] = "fingerprint_mismatch"
                    else:
                        fingerprints.setdefault(key, {"sha256": fingerprint, "consistent_responses": 0})
                        fingerprints[key]["consistent_responses"] += 1
                        successful_ms.append(result["wall_ms"])
                        per_fixture[index]["successful_ms"].append(result["wall_ms"])
                statuses[result["status"]] += 1
                per_fixture[index]["statuses"][result["status"]] += 1
                if "http_status" in result:
                    http_statuses[str(result["http_status"])] += 1
                response_bytes += result["response_body_bytes"]
                request_bytes += result["request_body_bytes"]
                if submitted < count and not abort.is_set():
                    submit()
    seconds = time.perf_counter() - started
    completed = sum(statuses.values())
    successes = statuses["success"]
    return {"requested": count, "submitted": submitted, "completed": completed,
            "not_submitted": count - submitted, "successes": successes,
            "failures": completed - successes, "statuses": dict(statuses),
            "http_statuses": dict(http_statuses), "elapsed_seconds": seconds,
            "completed_requests_per_second": completed / seconds,
            "successful_requests_per_second": successes / seconds,
            "successful_wall_ms": percentiles(successful_ms),
            "per_fixture": [{"name": item["name"], "statuses": dict(item["statuses"]),
                             "successful_wall_ms": percentiles(item["successful_ms"])} for item in per_fixture],
            "attempted_request_body_bytes": request_bytes, "received_response_body_bytes": response_bytes}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("url", help="Full /route POST endpoint")
    parser.add_argument("requests_json", type=Path)
    parser.add_argument("--requests", type=int, required=True)
    parser.add_argument("--concurrency", type=int, required=True)
    parser.add_argument("--warmup", type=int, help="Defaults to one request per fixture; use 0 for no warmup")
    parser.add_argument("--timeout", type=float, default=30)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--health-url")
    args = parser.parse_args()
    if not 1 <= args.requests <= 1_000_000 or not 1 <= args.concurrency <= 64 or (
        args.warmup is not None and not 0 <= args.warmup <= 1_000_000
    ):
        parser.error("Use --requests 1..1000000, --concurrency 1..64 and --warmup 0..1000000")
    if not math.isfinite(args.timeout) or not 0 < args.timeout <= 600:
        parser.error("--timeout must be finite and within (0, 600] seconds")
    for url in (args.url, args.health_url):
        if url and urlparse(url).scheme not in ("http", "https"):
            parser.error("URLs must use HTTP or HTTPS")
    try:
        source = json.loads(args.requests_json.read_text())
        if not isinstance(source, list) or not source or any(not isinstance(item, dict) for item in source):
            raise ValueError("Fixture must be a nonempty array of request objects")
        fixtures = []
        for index, item in enumerate(source):
            body = json.dumps({key: value for key, value in item.items() if key not in ("expect_cost", "name")},
                              sort_keys=True, separators=(",", ":"), allow_nan=False).encode()
            fixture = {"body": body, "request_hash": hashlib.sha256(body).hexdigest(),
                       "name": item.get("name", f"fixture-{index}")}
            if "expect_cost" in item:
                fixture["expect_cost"] = item["expect_cost"]
            fixtures.append(fixture)
    except (OSError, ValueError) as error:
        parser.error(str(error))
    if args.warmup is None:
        args.warmup = len(fixtures)

    abort, finished = threading.Event(), threading.Event()
    samples, fingerprints = [], {}
    baseline = health_sample(args.health_url, args.timeout) if args.health_url else None
    if baseline and not baseline["ok"]:
        abort.set()

    def monitor():
        # The batch is finite; this bound also prevents an orphaned monitor loop.
        limit = math.ceil((args.warmup + args.requests) * args.timeout) + 1
        for _ in range(limit):
            if finished.wait(1) or abort.is_set():
                return
            sample = health_sample(args.health_url, args.timeout)
            samples.append(sample)
            if not sample["ok"]:
                abort.set()
                return

    thread = threading.Thread(target=monitor) if args.health_url else None
    if thread:
        thread.start()
    try:
        warmup = phase(args.url, fixtures, args.warmup, args.concurrency, args.timeout, abort, fingerprints)
        measured = phase(args.url, fixtures, args.requests, args.concurrency, args.timeout, abort, fingerprints)
    finally:
        finished.set()
        if thread:
            thread.join()
    output = {
        "warmup": warmup, "measured": measured, "concurrency": args.concurrency,
        "fixture_count": len(fixtures), "timeout_seconds": args.timeout,
        "stopped_for_health": abort.is_set(),
        "health": {"baseline": baseline, "during_load_samples": len(samples),
                   "failed_samples": sum(not sample["ok"] for sample in samples),
                   "wall_ms": percentiles([sample["wall_ms"] for sample in samples]),
                   "response_bodies_recorded": False} if args.health_url else None,
        "fingerprints": fingerprints,
        "limitations": ["Latency percentiles include successful consistent responses only; failures are counted separately",
                        "Received body bytes count compressed data when gzip is returned; headers and partial failed reads are excluded",
                        "Alternative-route quality is not evaluated by this load driver",
                        "urllib opens a connection per request; this is not a browser connection-pooling model",
                        "Health failure stops new submissions; in-flight requests run to their timeout",
                        "Health monitoring samples after a one-second wait, with request time additional",
                        "Request timeout is a socket-operation timeout, not a total response deadline"]}
    encoded = json.dumps(output, indent=2, allow_nan=False)
    if args.output:
        args.output.write_text(encoded + "\n")
    print(encoded)
    return 1 if abort.is_set() or warmup["failures"] or measured["failures"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
