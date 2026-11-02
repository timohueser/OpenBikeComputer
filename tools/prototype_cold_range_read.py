#!/usr/bin/env python3
"""PROTOTYPE, never merges (#3000 workstream 1): how long does a cold range read of a large pack
take through maps.openbikecomputer.com?

Uploads one random pack of each size to `test/cold-range-<time>/` in the prod bucket, copies it
server side to fresh keys (a fresh key is a cold cache entry), times range reads with curl, and
deletes the folder at the end.

    python3 tools/prototype_cold_range_read.py [--sizes 1,15,50] [--repeat 3]
"""

import argparse, json, shlex, os, pathlib, statistics, subprocess, sys, tempfile, time

PUBLIC = "https://maps.openbikecomputer.com"
BUCKET = "obc-maps"
CACHE = "public, max-age=31536000, immutable"
MB = 1 << 20
VIA = []  # ["ssh", host] runs curl there


def credentials():
    env = {}
    for line in (pathlib.Path.home() / ".config/openbikecomputer/credentials.env").read_text().splitlines():
        line = line.strip().removeprefix("export ").strip()
        if "=" in line and not line.startswith("#"):
            key, value = line.split("=", 1)
            env[key] = value.strip().strip('"').strip("'")
    return {
        **os.environ,
        "RCLONE_CONFIG_P_TYPE": "s3",
        "RCLONE_CONFIG_P_PROVIDER": "Cloudflare",
        "RCLONE_CONFIG_P_REGION": "auto",
        "RCLONE_CONFIG_P_ENDPOINT": f"https://{env['CLOUDFLARE_ACCOUNT_ID']}.r2.cloudflarestorage.com",
        "RCLONE_CONFIG_P_ACCESS_KEY_ID": env["OBC_R2_ACCESS_KEY_ID"],
        "RCLONE_CONFIG_P_SECRET_ACCESS_KEY": env["OBC_R2_SECRET_ACCESS_KEY"],
        "RCLONE_CONFIG_P_NO_CHECK_BUCKET": "true",
        "OBC_VPS_SSH": env.get("OBC_VPS_SSH", ""),
    }


def rclone(env, *args):
    subprocess.run(["rclone", *args], env=env, check=True, capture_output=True)


FMT = '{"pre":%{time_pretransfer},"ttfb":%{time_starttransfer},"total":%{time_total},"code":%{http_code},"bytes":%{size_download},"cache":"%header{cf-cache-status}","ray":"%header{cf-ray}"}\n'


def reads(requests, parallel=False):
    """One curl process for all `(key, start, end)`, so they share one connection and one
    Cloudflare site. `server_ms` is the time from request sent to first byte."""
    cmd = ["curl", "-s", "--http2"] + (["--parallel"] if parallel else [])
    for i, (key, start, end) in enumerate(requests):
        if i:
            cmd.append("--next")
        cmd += ["-o", "/dev/null", "-w", FMT, f"{PUBLIC}/{key}"]
        if start is not None:
            cmd += ["-H", f"Range: bytes={start}-{end}"]
    out = subprocess.run(VIA + [shlex.join(cmd)] if VIA else cmd, capture_output=True, text=True, check=True).stdout
    results = []
    for line in out.splitlines():
        t = json.loads(line)
        results.append({
            "server_ms": round((t["ttfb"] - t["pre"]) * 1000),
            "total_ms": round(t["total"] * 1000),
            "code": t["code"], "bytes": t["bytes"], "cache": t["cache"] or "-",
            "colo": t["ray"].rpartition("-")[2],
        })
    return results


def show(label, r):
    print(f"  {label:<36} {r['cache']:<8} {r['code']} {r['server_ms']:>6} ms to first byte "
          f"{r['total_ms']:>6} ms total {r['bytes']:>9} B  {r['colo']}", flush=True)
    return r


def scenarios(size, keys):
    """Each scenario takes a fresh key, so its first read is cold."""
    tile = 64 * 1024
    res = {}
    k = iter(keys)

    key = next(k)
    a, b, c = reads([(key, 0, 16383), (key, size // 2, size // 2 + tile - 1), (key, size - tile, size - 1)])
    res["head_cold"] = show("header range, cold", a)["total_ms"]
    res["mid_next"] = show("  then a middle range", b)["total_ms"]
    res["end_next"] = show("  then the last range", c)["total_ms"]

    key = next(k)
    res["end_cold"] = show("last range, cold", reads([(key, size - tile, size - 1)])[0])["total_ms"]

    key = next(k)
    ranges = [(key, 0, 16383)] + [(key, i * size // 5, i * size // 5 + tile - 1) for i in range(1, 5)]
    par = reads(ranges, parallel=True)
    for i, r in enumerate(par):
        show(f"parallel range {i + 1} of {len(par)}, cold", r)
    res["parallel_max"] = max(r["total_ms"] for r in par)

    key = next(k)
    full = show("whole object, cold", reads([(key, None, None)])[0])
    res["full_cold"] = full["total_ms"]
    return res


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sizes", default="1,15,50", help="pack sizes in MB")
    ap.add_argument("--repeat", type=int, default=3)
    ap.add_argument("--vps", action="store_true", help="run the reads on the VPS of OBC_VPS_SSH")
    args = ap.parse_args()
    env = credentials()
    if args.vps:
        VIA.extend(["ssh", *env["OBC_VPS_SSH"].split()])
    folder = f"test/cold-range-{int(time.time())}"
    sizes = [int(s) for s in args.sizes.split(",")]
    print(f"folder {BUCKET}/{folder}")
    try:
        summary = {}
        with tempfile.TemporaryDirectory() as tmp:
            for mb in sizes:
                size = mb * MB
                src = pathlib.Path(tmp) / f"{mb}.pmtiles"
                src.write_bytes(os.urandom(size))
                base = f"{folder}/{mb}mb/base.pmtiles"
                t = time.time()
                rclone(env, "copyto", str(src), f"P:{BUCKET}/{base}", "--s3-no-check-bucket",
                       "--header-upload", f"Cache-Control: {CACHE}",
                       "--header-upload", "Content-Type: application/vnd.pmtiles")
                print(f"\n{mb} MB uploaded in {time.time() - t:.1f} s", flush=True)
                runs = []
                for rep in range(args.repeat):
                    keys = [f"{folder}/{mb}mb/r{rep}-{i}.pmtiles" for i in range(4)]
                    for key in keys:
                        rclone(env, "copyto", f"P:{BUCKET}/{base}", f"P:{BUCKET}/{key}")
                    print(f" run {rep + 1}")
                    runs.append(scenarios(size, keys))
                summary[mb] = runs
        print("\nmedian time to the last byte, ms")
        cols = ["head_cold", "mid_next", "end_next", "end_cold", "parallel_max", "full_cold"]
        print(f"{'MB':>4} " + " ".join(f"{c:>12}" for c in cols))
        for mb, runs in summary.items():
            print(f"{mb:>4} " + " ".join(f"{statistics.median(r[c] for r in runs):>12.0f}" for c in cols))
    finally:
        rclone(env, "purge", f"P:{BUCKET}/{folder}")
        print(f"\ndeleted {BUCKET}/{folder}")


if __name__ == "__main__":
    sys.exit(main())
