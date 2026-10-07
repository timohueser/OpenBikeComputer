# Cloudflare edge settings — runbook

These settings live in the Cloudflare dashboard. No repository file holds them. Check them after
any account change. The settings that the repository does hold are in the
[tile Worker README](../planner/tiles/README.md).

## What costs money

The tile Worker is the only part with a real per-request price. It serves tiles, fonts, sprites and
the device catalog. Cloudflare bills each Worker request, also when the edge cache answers it. A
request that misses the cache adds R2 reads. Egress from R2 is free. A request to `maps.` or
`updates.` costs one R2 read and no Worker request.
The VPS has a fixed price.

## Budget alert

Create the alert in Billing > Billable Usage. An alert only sends an email. It never stops usage.
Create two: one near the expected monthly spend and one at five times that figure.

## Rate limit rule

Create the rule in Security > WAF > Rate limiting rules. The free plan allows one rule, counted per
IP address over 10 seconds, with a 10 second block.

| Field | Value |
| --- | --- |
| Expression | `http.host in {"tiles.openbikecomputer.com" "maps.openbikecomputer.com"}` |
| Characteristic | IP address |
| Requests per 10 seconds | 3000 |
| Action | Block for 10 seconds |

The limit sits above the fastest honest burst. The web planner loads about 900 tiles within seconds
when a rider highlights a place type on a 150 km route. A blocked request never starts the Worker,
so Cloudflare does not bill it.

## Cache Rule for `maps.` and `updates.`

Cloudflare caches by file extension. It does not cache `.json`, `.pbf` or `.sqlite`, or any
extension that is not in its list, unless a Cache Rule allows it. Without the rule, every request
reads R2.

Create the rule in Caching > Cache Rules:

| Field | Value |
| --- | --- |
| Expression | `http.host in {"maps.openbikecomputer.com" "updates.openbikecomputer.com"}` |
| Cache eligibility | Eligible for cache |
| Edge TTL | Use cache-control header if present, bypass cache if not |

Uploads set `Cache-Control`. An object uploaded before it did stays uncached until it is uploaded
again.

## Check

Run each command twice. The second answer must read `HIT`. `DYNAMIC` means the object is not cached.

```sh
curl -sI https://maps.openbikecomputer.com/planner/catalog.json | grep -i cf-cache-status
curl -sI https://tiles.openbikecomputer.com/releases/ID/basemap/9/268/176.mvt | grep -i cf-cache-status
```

Open Workers & Pages > `obc-planner-tiles` > Settings > Domains & Routes. The only entry must be
`tiles.openbikecomputer.com`. A `workers.dev` entry skips the zone rules above.
