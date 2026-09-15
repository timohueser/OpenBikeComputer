# Shipping allocation evidence

The default shipping build at `d23e8270` reports an App allocation of 49,352 bytes.
The exact allocation record now matches that result. The check previously compared it with
the inherited 49,080-byte parent record.

Source: [CI shipping build](https://github.com/timohueser/OpenBikeComputer/actions/runs/34949142820/job/104315878797).
The build completed; the resource guard stopped at the exact App allocation mismatch.
No resource cap, recorded hardware high-water value, stack floor, or other allocation changed.
No local firmware image was built. The next CI run must pass the complete resource gate.
Public conceptual documentation is unchanged.
