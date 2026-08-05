//! Throwaway paths under the system temp dir — the one implementation of an idiom eleven test
//! modules had each written out by hand (repo root, packer, bakery, mkimage, USB host, assemble
//! oracle, and five of `obc-sim`'s store tests).
//!
//! No `tempfile` dependency: the host tools deliberately carry a small tree, and the whole
//! requirement is "a path nothing else in this run will pick". `<prefix>-<pid>-<seq>-<tag>` gives
//! that — the **pid** separates concurrent `cargo test` runs over one workspace (two checkouts, or
//! a re-run started before the first finished), and the process-local **seq** separates calls
//! within a run. A counter rather than a timestamp on purpose: back-to-back clock reads can tie,
//! and macOS's case-insensitive temp dir would alias two tags differing only in case.
//!
//! Nothing here removes anything on drop. A failed test that leaves its tree behind is
//! *evidence* — the temp dir is where you go to look at it — and the next run picks a fresh path
//! rather than inheriting it.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQ: AtomicU64 = AtomicU64::new(0);

/// A unique path under the system temp dir. **Nothing is created** — the caller decides whether it
/// becomes a file or a directory, which is what the image tools need (an output path a CLI is
/// asked to write, and asserting on the absence is half the test).
pub fn scratch_path(prefix: &str, tag: &str) -> PathBuf {
    let pid = std::process::id();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{prefix}-{pid}-{seq}-{tag}"))
}

/// A scratch **directory**, created and guaranteed empty: [`scratch_path`] plus a best-effort
/// wipe of anything already there. Panics if the directory cannot be created — a test that cannot
/// get a scratch tree has nothing left to assert.
pub fn scratch_dir(prefix: &str, tag: &str) -> PathBuf {
    let dir = scratch_path(prefix, tag);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create the scratch directory");
    dir
}
