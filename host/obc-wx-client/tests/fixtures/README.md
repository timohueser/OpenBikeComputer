# obc-wx-client test fixtures

One captured document, so the client's MET parser is pinned against **real** provider output rather
than against something the tests invented. Everything the OBC half needs — shard objects and the v2
manifest that names them — is built at test time through the production OBCG encoder over the
*derived* shard geometry, so a fixture can never drift from the object it describes. The manifest
document itself is pinned across both clients by `specs/vectors/wx-manifest-v2.json`, which is where
a captured manifest would have belonged anyway: it is a contract between two implementations, not a
recording.

No test in this crate touches the network. `--weather live` is the only thing that does.

| File | Captured | Provenance and terms |
| --- | --- | --- |
| `met-freiburg-24h.json` | 2026-08-09T22:5xZ | `https://api.met.no/weatherapi/locationforecast/2.0/complete?lat=48.0600&lon=7.9000`, truncated to the 24 hourly records the client reads. Data from MET Norway, NLOD 2.0 / CC BY 4.0. |

Note what this Freiburg capture happens to prove, because it is the case a synthetic fixture would
have missed: MET supplied **neither** `probability_of_precipitation` nor `wind_speed_of_gust` for
any of the 24 hours — the WX1 record's "Oslo has both, Manila has neither" geography difference, in
a German capture. The client must represent both as *unavailable*, never as zero.

Re-capture with:

```sh
curl -s -H "User-Agent: OpenBikeComputer-sim/0.1 github.com/timohueser/OpenBikeComputer" \
  "https://api.met.no/weatherapi/locationforecast/2.0/complete?lat=48.0600&lon=7.9000"
```

A re-capture changes the timestamps the tests read, so the suite derives "now" from the fixtures
themselves and never hard-codes an instant.
