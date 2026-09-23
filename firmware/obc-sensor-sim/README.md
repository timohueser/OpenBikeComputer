# BLE sensor simulator

Use a bare **nRF54L15 DK (PCA10156)** to simulate one BLE sensor at a time.
No external parts are required. This image runs directly on the application core without a bootloader.

## Build and flash

Install the repository Rust toolchain, libclang, probe-rs and cargo-nextest 0.9.143.
Run from this directory:

```sh
bash check.sh
cargo build --release --locked --features device --target thumbv8m.main-none-eabihf
probe-rs list
probe-rs run --chip nRF54L15 --probe <VID:PID:SERIAL> target/thumbv8m.main-none-eabihf/release/obc-sensor-sim
```

Select the spare L15 probe, especially when the bike computer is also connected.
`probe-rs run` writes the image and shows RTT output. The board then runs without a host.
Do not use `obc flash`: it targets the bike computer.

## Buttons and LEDs

The DK labels its buttons and LEDs **0–3**.

| Button | Press | Hold |
| --- | --- | --- |
| 0 | Select power → cadence → heart rate | Toggle stopped output after 1 second |
| 1 | Select steady → ramp → intervals | Toggle offline after 1 second |
| 2 | Decrease by 1 W, rpm or bpm | Repeat every 60 ms after 500 ms |
| 3 | Increase by 1 W, rpm or bpm | Repeat every 60 ms after 500 ms |

| LED | Meaning |
| --- | --- |
| 0 / 1 / 2 | Power / cadence / heart rate selected |
| Selected LED | 1 / 2 / 3 pulses: steady / ramp / intervals; solid: stopped |
| 3 | Blink: advertising; solid: connected; off: offline |

RTT prints the selected sensor, level and scenario after each button action.

| Sensor | Advertised name | Initial level | Range |
| --- | --- | --- | --- |
| Power | OBC Mock Power | 200 W | 0–2000 W |
| Cadence | OBC Mock Cadence | 90 rpm | 0–250 rpm |
| Heart rate | OBC Mock HR | 120 bpm | 0–240 bpm |

The level is the scenario ceiling. Ramp rises from 50% to 100% in 20 seconds, then falls in
20 seconds. Intervals alternate 50% and 100% every 10 seconds. Measurements arrive once a second.
Power includes crank data at 90 rpm when watts are nonzero. Stopped output sends zero watts or
bpm and stops crank events. Heart rate also reports loss of skin contact.
Offline disconnects and stops advertising; simulation time continues.

Sensor changes disconnect and expose the selected service under a distinct, stable address.
Levels remain in RAM across sensor changes. Each selection starts steady with stopped output off.
A reset restores all defaults. Battery is fixed at 90%; device information is readable.
Pairing and bonding are not required. Only one collector can connect at a time.

## Power calibration

Enable indications on Cycling Power Control Point (`0x2A66`), then write `0C` with a Write Request.
After one second, stopped cranks return `20 0C 01 00 00`: success with a simulated zero-force offset.
Turning cranks return `20 0C 04`: operation failed. Hold button 0 to test both outcomes.
The collector must confirm the indication before another request can start.

Missing indication subscriptions and concurrent requests return standard ATT errors.
Malformed calibration parameters and unsupported procedures return control-point error indications.
Only crank data and offset compensation are advertised as power features. Advanced cycling dynamics,
crank-length adjustment and enhanced calibration are unsupported.
See the [Cycling Power Service specification](https://www.bluetooth.com/specifications/specs/cycling-power-service/).

The bike computer does not yet initiate calibration. Use a BLE GATT client to test this exchange.
Use the sensor menu to check discovery, levels, stopped output, mode changes and reconnects on hardware.
