# Excluded initial timing attempt

The first acceptance attempt overlapped an unrelated Cargo build after its
first baseline. The competing command was `cargo test --locked -p obc-route
-p obc-reader -p obc-host-core -p obc-sim -p obc-link -p obc-vectors
-p obc-web-assemble -p obc-display --all-features --no-run`, with its target under
another worktree (`ts7-closeout`). Process inspection found cargo PID695264 and
multiple rustc children using roughly 60–76% CPU each. The candidate assembly and
independent read-back both slowed. We did not stop unrelated processes.

Three samples completed before the runner was stopped at the next available
sample boundary. The whole initial attempt is excluded. These raw samples
remain as excluded evidence of known interference, not as a cache comparison.
The fixed acceptance sequence restarts unchanged after the competing build ends.
