# Excluded overlap after a clean pair

Samples 1–2 completed without a competing build and remain in the main record.
During sample 3, another `cargo clippy --all-targets --all-features` command
started in the unrelated `ts7-closeout` worktree. Process inspection found
cargo-clippy PID 744324 and multiple rustc children. The runner was stopped at
the sample boundary; only this identified overlap is excluded. No other process
was stopped. The fixed sequence resumes at sample 3 after a quiet interval.

The clean candidate sample 2 completed its independent read-back at
2026-09-15T14:15:44.495Z. The other command's redirected Clippy log was created
at 2026-09-15T14:15:53.615Z (filesystem birth time), nine seconds later. Thus the
known Clippy overlap begins after the retained pair, not during it.
