//! The work folder, the memory budgets and the memory guard of the bake.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

/// The memory that the bake can fill with the pages of its work files, and the source of the
/// figure: the smallest of the free memory of its cgroup, `MemAvailable`, and
/// `OBC_DATA_MEMORY_BYTES` less the own memory of the bake, each where it exists. The first two
/// already leave out the own memory. It is read once; the step reads it first, so that a bad
/// value fails before the import. It changes the speed of the bake, never its bytes.
pub fn page_budget() -> Result<(u64, &'static str), String> {
    static BUDGET: OnceLock<Result<(u64, &'static str), String>> = OnceLock::new();
    BUDGET
        .get_or_init(|| {
            let cgroup = obc_data::engine::cgroup::memory()
                .and_then(|cgroup| Some(cgroup.max?.saturating_sub(cgroup.anon)))
                .map(|bytes| (bytes, "the cgroup limit"));
            let available = meminfo("MemAvailable:").map(|bytes| (bytes, "MemAvailable"));
            let setting = match std::env::var("OBC_DATA_MEMORY_BYTES") {
                Err(_) => None,
                Ok(value) => {
                    let bytes = value
                        .parse::<u64>()
                        .map_err(|_| format!("OBC_DATA_MEMORY_BYTES is not a number of bytes: {value:?}"))?;
                    Some((bytes.saturating_sub(rss_anon().unwrap_or(0)), "OBC_DATA_MEMORY_BYTES"))
                }
            };
            Ok([cgroup, available, setting].into_iter().flatten().min().unwrap_or((u64::MAX, "no limit")))
        })
        .clone()
}

/// The bytes of one sort run: a quarter of the page budget, from 1 MiB to 4 GiB. A run sorts in
/// place in its column's mapped file, so this bounds the file pages that one sort touches at once,
/// not the bake's own memory. The floor keeps the merge heap small.
pub fn sort_run_bytes() -> Result<usize, String> {
    Ok((page_budget()?.0 / 4).clamp(1 << 20, 4 << 30) as usize)
}

/// A value of `/proc/meminfo` in bytes.
fn meminfo(key: &str) -> Option<u64> {
    let text = fs::read_to_string("/proc/meminfo").ok()?;
    kib(text.lines().find_map(|line| line.strip_prefix(key))?)
}

/// The anonymous memory of this process (`RssAnon`), in bytes: the bake's own memory.
fn rss_anon() -> Option<u64> {
    let text = fs::read_to_string("/proc/self/status").ok()?;
    kib(text.lines().find_map(|line| line.strip_prefix("RssAnon:"))?)
}

fn kib(value: &str) -> Option<u64> {
    Some(value.trim().strip_suffix("kB")?.trim().parse::<u64>().ok()? * 1024)
}

/// The budget of the routing bake's own memory, which the owner set.
pub const OWN_BYTES: u64 = 4 << 30;

struct Stage {
    name: &'static str,
    exempt: bool,
    started: Instant,
    peak: u64,
}

/// The stage of a running guard; `None` without a guard.
static STAGE: Mutex<Option<Stage>> = Mutex::new(None);

/// Samples the own memory of the bake every 100 ms, and ends the bake at the first sample above
/// the budget in a stage that is not exempt. A guard that only reads the peak at the end of a
/// stage misses a table that the stage frees before its end, and lets a stage over the budget run
/// on until an OOM kill. When it is dropped, the guard prints the peak of the last stage.
pub struct Guard {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Guard {
    pub fn start(budget: u64) -> Self {
        *lock() = Some(Stage { name: "start", exempt: true, started: Instant::now(), peak: 0 });
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(100));
                let mut stage = lock();
                let Some(stage) = stage.as_mut() else { return };
                let sample = rss_anon().unwrap_or(0);
                stage.peak = stage.peak.max(sample);
                if let Some(message) = over(stage.name, stage.exempt, sample, budget) {
                    eprintln!("{message}");
                    std::process::exit(1);
                }
            }
        });
        Self { stop, thread: Some(thread) }
    }

    /// Starts the next stage, and prints the time and the peak of the stage before it.
    pub fn stage(name: &'static str) {
        next(name, false);
    }

    /// Starts the next stage, which the guard measures but does not check.
    pub fn exempt(name: &'static str) {
        next(name, true);
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        if let Some(stage) = lock().take() {
            report(&stage);
        }
    }
}

fn lock() -> std::sync::MutexGuard<'static, Option<Stage>> {
    STAGE.lock().unwrap_or_else(PoisonError::into_inner)
}

fn next(name: &'static str, exempt: bool) {
    let mut stage = lock();
    let Some(old) = stage.as_mut() else { return };
    old.peak = old.peak.max(rss_anon().unwrap_or(0));
    report(old);
    *old = Stage { name, exempt, started: Instant::now(), peak: 0 };
}

fn report(stage: &Stage) {
    if stage.name != "start" {
        let seconds = stage.started.elapsed().as_secs_f64();
        eprintln!("Stage {}: {seconds:.1} s, own memory peak {} MiB", stage.name, stage.peak >> 20);
    }
}

/// The message that ends the bake when `sample` bytes of own memory pass `budget` in a stage
/// that is not exempt.
fn over(stage: &str, exempt: bool, sample: u64, budget: u64) -> Option<String> {
    let gib = |bytes: u64| bytes as f64 / f64::from(1u32 << 30);
    (!exempt && sample > budget).then(|| {
        format!("stage {stage} used {:.2} GiB of own memory, above the budget of {:.2} GiB", gib(sample), gib(budget))
    })
}

/// `OBC_DATA_WORK`, else `TMPDIR`, which `obc data` sets on every platform, else the temporary
/// folder of the system.
fn base() -> PathBuf {
    ["OBC_DATA_WORK", "TMPDIR"]
        .into_iter()
        .find_map(|name| std::env::var_os(name).filter(|value| !value.is_empty()))
        .map_or_else(std::env::temp_dir, PathBuf::from)
}

/// A new folder in `base`. The name holds the process ID, but a killed bake or another container
/// can have used it, so the name takes the first free number.
fn new_folder(base: &Path) -> std::io::Result<PathBuf> {
    fs::create_dir_all(base)?;
    for attempt in 0u64.. {
        let folder = base.join(format!("obc-router-build-{}-{attempt}", std::process::id()));
        match fs::create_dir(&folder) {
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            result => return result.map(|()| folder),
        }
    }
    unreachable!("a folder name is free before the numbers end")
}

/// The bake's folder while it holds a file, the name of the next file, and the number of files.
static FILES: Mutex<(Option<PathBuf>, u64, usize)> = Mutex::new((None, 0, 0));

/// A new empty file in the bake's own folder of the work folder, removed when dropped. The folder
/// exists while it holds a file, so the bake leaves nothing behind after an error or a panic.
pub struct File(PathBuf);

impl File {
    pub fn create() -> std::io::Result<Self> {
        let mut files = FILES.lock().unwrap_or_else(PoisonError::into_inner);
        let (folder, next, open) = &mut *files;
        if folder.is_none() {
            *folder = Some(new_folder(&base())?);
        }
        let path = folder.as_ref().expect("the folder exists").join(next.to_string());
        fs::OpenOptions::new().write(true).create_new(true).open(&path)?;
        *next += 1;
        *open += 1;
        Ok(Self(path))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for File {
    fn drop(&mut self) {
        let mut files = FILES.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = fs::remove_file(&self.0);
        files.2 -= 1;
        if files.2 == 0 {
            if let Some(folder) = files.0.take() {
                let _ = fs::remove_dir(folder);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sample_above_the_budget_names_the_stage() {
        let message = over("road-pass", false, 2, 1).unwrap();
        assert!(message.starts_with("stage road-pass used"), "{message}");
        assert_eq!(over("road-pass", false, 1, 1), None);
        assert_eq!(over("catalog", true, 2, 1), None);
    }
}
