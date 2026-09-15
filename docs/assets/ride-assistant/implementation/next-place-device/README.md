# Next-place device check — 2026-09-15

The owner asked to test the Swiss simulator scenario on the nRF54LM20-DK before performance work.
The installed release image uses `debug-uart` for stationary sensor input. Map reads, candidate
ranking, route planning, temporary route storage, and the display run on the physical device.

## Installed image and inputs

- Firmware version: `6c81ddb`; production source at `6c81ddb1a`. Later commits change tests,
  documentation, and the equivalent Clippy match guard only.
- ELF SHA-256: `41caba5bb32d5cb4dcec6f8d1f0bc21e54921f950a00e7b5c34eef98493d286e`.
- Meiringen map: 44,643,264 bytes; SHA-256
  `ebe53f369a558e2c4e8da593b05433e55f730d83d07a928d46f7a236ec145bba`.
- Route: `meiringen-loop-dem.obcr`, 1,366 bytes; SHA-256
  `8518443151990721e247363667d9358605e0046d6be46cfc02d0fa724b47c906`.
- Stationary GPS: latitude 46.723126, longitude 8.194551; heading 225 degrees; altitude 602 m.
  The debug feed sends a fresh fix each second. No route was activated during this check.
- The map and route match the simulator inputs. Debug GPS does not set the wall clock, so opening
  hours do not have the simulator's fixed 10:00 clock. The Swiss map has native elevation, but
  does not contain the separate simulator Peak View surface.

## Observed device behavior

The initial card mount reported `CatalogUnreadable`. The owner explicitly approved formatting.
The production USB FORMAT command succeeded. Normal USB PUT stored map object 1/revision 1 and
route object 2/revision 1. Both length and CRC matched their inputs. A restart mounted a writable
catalog with one map and one route. No raw-card bench or unverified flash was used.

The held Up + Select shortcut opened Assistant. Select opened **Where's the next...**. Water
search planned eight candidates and displayed four choices. Candidate plan times were
200, 187, 317, 331, 487, 407, 458, and 381 ms. From category selection at device time 159.157 s to
the first result-map frame at 168.785 s was about 9.6 s. This includes querying, all candidate
plans, temporary-route publication/removal, and rendering; it is not one routing duration.

Selecting Drinking water opened Visit review directly, with a 17 m route and the **Go here**
action. The intermediate place-detail screen was absent. The device was left on that preview
for the owner. Route-cost ranking is unchanged. Candidate retention and simulator scheduling
speedups are excluded.

The images are decoded captures of the device's actual 240 x 320 framebuffer:

- [Category menu](categories.png)
- [Water choices](water.png)
- [Direct route preview](water-preview.png)

## Validation and remaining work

- Verified, single-buffered programming completed successfully in 184 s.
- [Device resource guards](resources.log) passed against the recorded baseline. No base build.
  The installed image uses 309,320 B linked resident RAM and 132,096 B `.uninit`; largest guarded
  poll frame is 9,776 B. Observed stack high-water during water search was 33,892 / 50,104 B.
- Focused app, reader, route, and host-core suites passed before the device split. Focused app
  suites passed again after the UI copy fixes. All 16 web-demo tests passed after adapting the
  direct-preview flow. Package Clippy, formatting, suite registry, and public-doc link checks passed.
- Independent integrated adversarial review found no code blocker. Its stale-documentation
  finding was fixed in a separate documentation commit; delta review passed.
- One final UI snapshot sweep rendered all 263 named frames. All 16 changed frames were inspected;
  the manifest records the intended title, service-action, coverage-label, and approach-gap changes.
- Exact focused commands: `tools/obc test -p obc-app -p obc-reader -p obc-route -p obc-host-core`;
  `tools/obc test -p obc-app -p obc-web-demo`; corrected web adapter: `tools/obc test -p obc-web-demo`;
  `cargo clippy -p obc-app -p obc-reader -p obc-route -p obc-host-core -p obc-web-demo --all-targets -- -D warnings`;
  `tools/obc suites check`; `python3 docs/build_docs.py --check-links`.
- Owner acceptance is pending: lodging/resupply choices, campsites, More places, preview Back,
  route activation/return, physical button feel, and timing on more demanding routes.
- No performance change, wake isolation, full CI mirror, or destructive fault test was run.
