# Active-route place search

The owner measured more than 40 seconds for Water on the nRF54LM20A with the Meiringen route active.
The earlier 5.801-second result used the physical device with no active route. It did not measure
active-route Visit planning.

## Same-location physical comparison

The Swiss map is object 1, revision 1, in store `dc2b0e053244229207bd15bb4812750e`.
The original Meiringen route is object 2, revision 1. Normal GPS updates settle its progress at
1,664 m. Fixed input is 46.723126 N, 8.194551 E, altitude 602 m, heading 225 degrees.

| Category | Initial image `7315fa201` | Reviewed shortlist `4f6c584e7` |
| --- | ---: | ---: |
| Water | 39.816 s | 7.822 s |
| Resupply | 28.826 s | 7.016 s |
| Campsites | — | 9.677 s |
| Lodging | — | 9.521 s |
| Pharmacy | — | 4.233 s |
| Bike shops | — | 8.547 s |
| Train stations | — | 12.894 s |

Times run from the device's category Select event to the first completed results frame.
[Baseline timestamps](baseline-timings.json) and [reviewed-shortlist timestamps](ranked-timings.json)
record those events. Water repeated at 7.361 s. These are device times, not simulator wall times.
The owner accepted the longer Train result and requested that searches finish without a time cutoff.

The initial ELF SHA-256 is `c2cbe630333d441a884e7ccb5541ad6cdd4dc86ce36e2cab835fdf59f5b84644`.
The reviewed-shortlist ELF SHA-256 is `0af18e4957ad4b8116108dc6eca474e36955f56ddd79c3bb931f0506a312fd81`.
The final compass image is recorded separately below.

## What changed

A visit retains the original route from current progress to the point nearest the place, makes two
directed graph searches to the place and back, and retains the original tail. The search uses the
nearest forward route point within 20 km. Equal whole-metre distances use the first forward pass.
Original waypoints and loops remain. Imported route connections within the normal 100 m snap limit
retain both coordinates and their measured distance. Their surface and elevation are unknown.

Find measures up to four nearby and four corridor candidates, with duplicate identities removed.
Corridor selection uses the same route occurrence as Visit plus its straight connection distance.
The final four Water choices arrive at 17, 489, 970, and 972 m. This replaces the misleading 3,739 m
choice caused by two different interpretations of the overlapping route. All offered choices have
complete routes and are ranked by measured route costs. More places retains the wider paged list.

Repeated source checks and catalog work are reduced. Full map rendering stops during the planning
batch. The loading pill stays visible and its shared compass turns by one third of a revolution
once per second. Only its framebuffer band is repainted; that path does not read the map.

The Visit preview fits departure, stop, and return. The nearest Water preview spans 18 m while its
stored route retains the complete 4,863 m continuation. Opening and reopening it starts no new
routing: [physical reuse trace](ranked-preview-reuse.log). The offline simulator also checks
identical saved route bytes, object identities, and preview pixels.

## Where time remains

For Water, logged frame preparation/render/display takes 1.112 s and banner updates take 0.115 s.
The remaining 6.59 s includes routing, storage, validation, and scheduling. The first frame timer
includes place-query preparation, so it is not a pure drawing measurement. Train has 1.015 s of
frame work, 0.198 s of banner updates, and 11.68 s elsewhere. Its long second candidate occupies
about 7.26 s between the preceding catalog completion and its construction-complete marker.

The traces do not separate A* computation from graph reads, route writes, and storage waits.
Two potential larger improvements remain unmeasured: retaining graph tiles across outward/return
legs, and combining repeated original-index, candidate-CRC, and candidate-geometry reads. These
could reduce particular read/parse work substantially; no further 30% elapsed-time gain is claimed.
No additional routing optimization is part of the final compass change.

## Evidence and verification

[Offline simulator evidence](simulator/README.md) includes the real map/route hashes, production
input scripts, six named captures, geometry checks, and preview reuse hashes. Network access is
denied during those runs. Intermediate timings remain in [the development record](intermediate-timings.json)
and [the initial eight-candidate record](bounded-timings.json).

The local resource bundle used `ae000874` and the recorded baseline; it did not rebuild the base.
It measured 309,808 B resident RAM, 132,096 B uninitialized arenas, 1,725,760 B flash, and a 49,616 B
residual stack. Later changes use the CI resource gate. CI run 35018098962 measured App at 52,392 B,
linked resident at 308,168 B, and residual stack at 51,256 B on its default image. The exact allocation
record was updated from that report; resource and stack limits stay unchanged. No second local
resource bundle is run.

## Final compass device check

The final image uses production source `9f6c93e6d`, features `debug-uart`, and ELF SHA-256
`6c3891e643f1b9508a4ffc5074fd1162bbac14e72af993a4d39eafb6da9a7c48`.
Verified programming completed before the same original-route scenario was restored.
Water displayed its four choices in **7.395 s**; Train displayed its complete result in **12.555 s**.
[Device timestamps and image identity](compass-timings.json) accompany the raw
[Water](compass-water.log) and [Train](compass-train.log) traces.
[The stable device preview](device-water-preview.png) shows the nearest Water visit after completion.
The compass repaints rows 88..144 once per second. Each repaint takes about 19–20 ms including
panel transfer. Both searches finish normally; no elapsed-time cutoff is installed.

[The verification record](verification.md) lists checks and omissions. The independent source,
fix-delta, and integrated visual reviews have no remaining finding. The final CI status is recorded
on pull request 1803. [The device checklist](device-checklist.md) preserves this test setup and the
remaining on-road acceptance work.
