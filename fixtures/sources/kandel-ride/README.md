# Kandel ride fixture

`kandel.gpx` is a user-provided Komoot GPX export. Its coordinates, elevation and timestamps
are the source for this fixture. The licence for the export is not confirmed.

Build the finished ride from the repository root:

```sh
cargo run -p obc-replay --example gpx_to_ride -- \
  fixtures/sources/kandel-ride/kandel.gpx \
  fixtures/build/kandel-ride/kandel.obcr \
  1790236800 'Kandel (simulated)'
```

The output is a recorded-ride v6 object. The generator keeps every GPX track point and adds
deterministic synthetic heart rate, cadence and power. The ride records max HR 185 bpm and FTP
250 W, so its heart rate and power fall into effort zones. Two more arguments,
`MAX_HR_BPM FTP_W`, set other limits; `0` is not set. It uses the GPX elevation and time for
grade, speed and ride totals. The chosen start time places the demo ride near the top of the
device ride list. The sensor readings are simulated, not a record of a rider.

## Add the ride to the bike computer

Use the J4 debug cable. Keep the exact normal firmware ELF before the temporary flash.
The transfer protocol does not accept ride uploads.

```sh
OBC_DEMO_RIDE_FILE="$PWD/fixtures/build/kandel-ride/kandel.obcr" obc flash seed-rides
```

Wait for `demo rides: complete`, then press Ctrl-C. The temporary image stays idle.
Restore the saved normal firmware with `obc board run /absolute/path/to/normal.elf`.
The ride remains on the card and syncs to the companion app like a recorded ride.
Reusing the same fixture does not add a duplicate. A v5 ride with the same name is replaced
in place; any other different payload with the same name is refused. The seeder rewrites every
other v5 ride on the card as a v6 ride without limits, with its samples unchanged, because the
v6 firmware cannot read v5 rides. Existing routes and maps are preserved.
