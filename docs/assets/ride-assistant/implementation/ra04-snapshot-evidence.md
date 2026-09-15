# Route facts and lifecycle snapshot evidence

CI run 34955219674 at 51371533 produced all 259 expected frames. The manifest comparison found
14 changed frames and no missing or extra frames. Each changed frame was inspected before its
hash was recorded. The resulting manifest check passes for all 259 CI frames.

The changed frames cover the route facts profile, Climb, route/trip totals, route overview and
Statistics ETA, waypoint Statistics, and the legacy Up ahead and Detour views. Their layouts
remain legible. The legacy Detour preview shows +0 m and unknown ascent. This is a layout capture,
not evidence for a useful alternative or the final ordinary Assistant journey. RA12 replaces the
legacy entry recipes and records a real Monaco Detour with a 1,333 m connector and +533 m versus
the replaced span. The integrated acceptance records the normal entry separately.

No local snapshot sweep, simulator build, shipping-image build or base-resource rebuild was run
for this manifest correction. The original CI outputs remain the source of these hashes. The
suite registry and public documentation link check pass. Independent delta review and green CI
remain required before merge.
