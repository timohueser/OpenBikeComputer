# Offline scenario packaging evidence

The West Cork regional fixture now contains an ordinary OBCM v16 map with native terrain,
services, graph, and compiled landmark text, photo and Sources. The authored Dunlough GPX remains
byte-identical. The normal simulator scenario uses these files without a study model or injected
navigation outcomes. The Swiss regional replacement remains pending its ongoing bake; its existing
v14 package and scenario entries are unchanged.

## Immutable output

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `sim-assistant-west-cork.tar.gz` | 2,425,629 | `3098b38214e3206bea8947927aa4d0a44cf618535b91dc063a4b3baeaa826a26` |
| `west-cork.obcm` | 4,746,240 | `a48ebe53b9a545492b94ef4d59cdd2f371e70705683f092d9370112cccc29b23` |
| native `19_0610_0493.obcd` | 2,097,188 | `c87457f662a394f7cb84c11ec1f7f08f34f602357b5bc7da51667eec57da26b2` |

The normal assembler selected ten cells, retained 55 service records, and wrote one native terrain
cell in four terrain slots. Its graph has 7,986 nodes and 8,607 edges after normal graph merge and
component pruning. The crop deliberately has partial boundary cells. Assembly did not accept
missing cells or skip verification. The header square is larger than the actual source crop;
it must not be presented as full coverage of that square or of Ireland.

The retained source record is `fixtures/sources/ride-assistant/west-cork-v16.json`. It contains
exact executable hashes, input and compiler policy digests, regional bounds, assembly counts,
and log hashes. The producer commits are attributed from retained executable modification times
and the branch reflog, not an embedded build stamp. The direct binary and output hashes are the
strong byte identity evidence. The compiled four-site source set contains four texts and three
photos; this is bounded review content, not a full-country acquisition claim.

## Verification

- The updated recipe generated native sidecars from the completed shipping tree catalog, its exact
  region selection and hashes. The shipping assembler reproduced the final Cork map byte for byte.
  This exercised real cut cells, shared service hours, optional landmark content, graph and raster.
  The completed bake was not repeated, and the long Swiss bake was not restarted.
- The existing deterministic fixture packer reproduced the same archive hash on a second pack.
- `tools/obc fixtures publish` performed its immutable upload and public-domain byte verification.
- An empty fixture cache downloaded the package from the public domain. Cached `sync`, `verify`
  and `resolve` then passed with `urllib.request.urlopen` disabled. Every archive member and the
  tracked authored GPX passed the existing fixture checks.
- `OBC_DRY_RUN=1 tools/obc sim assistant-dunlough-access -- --create-card ...` resolved the ordinary
  map, GPX, explicit clock and persistent-card arguments. This is command-wiring evidence, not a
  completed GUI or acceptance test.
- The complete repository Python suite passes: 187 tests, including the local catalog sidecar,
  changed-source refusal, short-header refusal and completed-map provenance binding. The system
  Python lacked `xmlrunner`; the pinned `tools/requirements-test.txt` was installed in an isolated
  environment, then the documented whole XML-reporting suite passed.
- Suite registry, Python compilation, documentation links and diff checks pass. No Rust rebuild,
  resource image, UI snapshot sweep, full CI mirror or physical-device run was performed here.

## Use and remaining acceptance

The source README documents `tools/obc sim assistant-dunlough-access`, the dense Monaco scenario,
and a persistent Unix card. Dunlough Castle is `Q5315471`, joined to OSM way `300189816`. Its source
has no opening-hours or bicycle/access tags. The mountain-hiking walking approach does not prove
rideability; a truthful unavailable Visit is expected when no source-proven approach qualifies.

The integration owner must exercise the final menu, source-linked text and photo, actual planner
preview and acceptance, recording continuity, and card recovery through normal simulator paths.
The final named-frame/snapshot and shipping resource budgets also remain with that owner. The
Swiss package must be replaced only after its completed crop and provenance are available.
Physical buttons, SD latency and failures, sensor continuity, and measured stack behavior remain
pending a connected-device session. No software fixture result substitutes for those measurements.

## Provenance review delta

Independent review found that `--bin-dir` can select tools built from another source revision.
Commit `9f016807` therefore records the checkout as `recipe_commit`, not `source_commit`, and
hashes all three producer executables before the bake. This matches the retained Cork record's
separation of directly verified binary identity from source attribution. The published package
already has that distinction and is unchanged. The complete Python suite (187 tests), registry,
Python compilation and documentation checks pass. No bake, assembly or upload was repeated.
