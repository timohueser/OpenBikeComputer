# OBC rain radar demo

A standalone development viewer for the canonical `wx/v2` precipitation mosaic. It overlays the
published one-kilometre OBCG shards on OpenStreetMap, exposes the complete zero-to-two-hour
timeline, and keeps the dataset and request accounting visible while inspecting coverage.

Run it from anywhere in the repository:

```sh
obc rain-radar
```

The command builds the small Vite frontend and starts the caching local proxy at
<http://127.0.0.1:4174>. The proxy is required because the public weather origin is deliberately a
static object service, not a development web backend.

## Request budget

The app requests only manifest-present shards intersecting the viewport, at concurrency four.
Manifest-declared dry shards produce no request. Immutable shard bodies are cached by the browser
and in a 256 MiB in-process LRU, and simultaneous reads of the same key are coalesced. The proxy
refuses new upstream reads after 2,000 requests in one process; the side panel shows upstream reads,
cache hits, and bytes continuously.

Optional runtime settings:

```sh
OBC_WX_BASE_URL=https://wx.openbikecomputer.com obc rain-radar 4180
OBC_RADAR_MAX_REQUESTS=500 OBC_RADAR_CACHE_MB=128 obc rain-radar
```

The first positional argument is the port. Restarting the process starts a new in-memory cache and
request budget.

## Focused verification

```sh
cd tools/rain-radar-demo
npm ci
npm run check
npm test
npm run build
```

The decoder tests consume the repository's normative OBCG and manifest vectors, including all
three tile codecs, no-data, dry sentinels, and integrity failures.
