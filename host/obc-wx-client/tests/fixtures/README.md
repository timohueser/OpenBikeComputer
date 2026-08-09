# obc-wx-client test fixtures

Two captured documents, so the client's parsers are pinned against **real** service output rather
than against something the tests invented. Everything else the suite needs — OBCG objects, their
manifest entries — is built at test time from the checked-in `specs/vectors/grid-*.obcg`, the same
trick the Swift suite uses: a fixture derived from the real vector header can never drift from the
object it describes.

No test in this crate touches the network. `--weather live` is the only thing that does.

| File | Captured | Provenance and terms |
| --- | --- | --- |
| `manifest-production.json` | 2026-08-09T22:5xZ | `https://wx.openbikecomputer.com/wx/v1/manifest.json`, trimmed to the first two frames of each product so the timeline shape survives at a fraction of the bytes. Metadata about DWD (CC BY 4.0) and NOAA (US-government open data) products, produced by this project's own baker. |
| `met-freiburg-24h.json` | 2026-08-09T22:5xZ | `https://api.met.no/weatherapi/locationforecast/2.0/complete?lat=48.0600&lon=7.9000`, truncated to the 24 hourly records the client reads. Data from MET Norway, NLOD 2.0 / CC BY 4.0. |

Note what this Freiburg capture happens to prove, because it is the case a synthetic fixture would
have missed: MET supplied **neither** `probability_of_precipitation` nor `wind_speed_of_gust` for
any of the 24 hours — the WX1 record's "Oslo has both, Manila has neither" geography difference, in
a German capture. The client must represent both as *unavailable*, never as zero.

Re-capture with:

```sh
curl -s https://wx.openbikecomputer.com/wx/v1/manifest.json
curl -s -H "User-Agent: OpenBikeComputer-sim/0.1 github.com/timohueser/OpenBikeComputer" \
  "https://api.met.no/weatherapi/locationforecast/2.0/complete?lat=48.0600&lon=7.9000"
```

A re-capture changes the timestamps the tests read, so the suite derives "now" from the fixtures
themselves and never hard-codes an instant.
