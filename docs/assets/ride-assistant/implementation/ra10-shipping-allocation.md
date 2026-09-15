# Shipping allocation evidence

The default shipping build at `208dd4b5` reports an App allocation of 51,528 bytes.
The exact allocation record now matches that result. The check previously compared it with
the inherited 49,080-byte parent record.

Source: [CI shipping build](https://github.com/timohueser/OpenBikeComputer/actions/runs/34947822787/job/104312140767).
The build completed; the resource guard stopped at the exact App allocation mismatch.
No resource cap, recorded hardware high-water value, stack floor, or other allocation changed.
No local firmware image was built. The next CI run must pass the complete resource gate.
Public conceptual documentation is unchanged.
