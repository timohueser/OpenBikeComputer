//! One publication owner. An unacknowledged mutation stops every later commit.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::engine::runs::{Publication, Run};
use crate::store::sha256_hex;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Intent {
    pub mutation: Publication,
    /// Exact expected and desired identities. A missing object has no identity.
    pub expected: Option<String>,
    pub desired: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub run: String,
    pub bundle: String,
    pub pending: Option<Intent>,
    pub finished: bool,
}

/// The lock is inherited by children. A surviving mutator still excludes a new owner.
pub struct Owner {
    state: State,
    path: PathBuf,
    _lock: File,
    _run_lock: File,
}

impl Owner {
    pub fn open(directory: &Path, run: &str, bundle: &[u8]) -> Result<Self, String> {
        crate::engine::runs::check_id(run)?;
        durable_directory(directory)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("owner.lock"))
            .map_err(|e| e.to_string())?;
        inherit_lock(&lock)?;
        let digest = sha256_hex(bundle);
        let run_lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join(format!("{run}-{digest}.owner.lock")))
            .map_err(|e| e.to_string())?;
        inherit_lock(&run_lock)?;
        let path = directory.join(format!("{run}.json"));
        for entry in std::fs::read_dir(directory).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.path().extension().is_some_and(|ext| ext == "json") {
                let state: State = serde_json::from_slice(&std::fs::read(entry.path()).map_err(|e| e.to_string())?)
                    .map_err(|e| format!("commit state: {e}"))?;
                if state.pending.is_some() {
                    return Err(format!(
                        "commit {} has an unknown mutation outcome; reconcile it before any commit",
                        state.run
                    ));
                }
            }
        }
        let state = if path.exists() {
            let state: State =
                serde_json::from_slice(&std::fs::read(&path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            if state.run != run || state.bundle != digest {
                return Err(format!("commit {run} is already bound to another bundle"));
            }
            state
        } else {
            State { run: run.into(), bundle: digest, pending: None, finished: false }
        };
        let owner = Self { state, path, _lock: lock, _run_lock: run_lock };
        owner.save()?;
        durable(&directory.join("owner.active"), &serde_json::to_vec(&owner.state).map_err(|e| e.to_string())?)?;
        Ok(owner)
    }

    /// A retained mutator child counts as ownership too. No PID or service name proves it.
    pub fn active(directory: &Path, run: &str, bundle: &str) -> Result<bool, String> {
        let lock = match File::open(directory.join("owner.lock")) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.to_string()),
        };
        let run_lock = match File::open(directory.join(format!("{run}-{bundle}.owner.lock"))) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.to_string()),
        };
        let held = || {
            for file in [&lock, &run_lock] {
                match file.try_lock() {
                    Ok(()) => return Ok(false),
                    Err(std::fs::TryLockError::WouldBlock) => {}
                    Err(std::fs::TryLockError::Error(error)) => return Err(error.to_string()),
                }
            }
            Ok(true)
        };
        if !held()? {
            return Ok(false);
        }
        let state: State = match std::fs::read(directory.join("owner.active")) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| e.to_string())?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.to_string()),
        };
        Ok(state.run == run && state.bundle == bundle && held()?)
    }

    pub fn finished(&self) -> bool {
        self.state.finished
    }

    pub fn unknown(&self) -> bool {
        self.state.pending.is_some()
    }

    /// Fsync intent before the call; fsync its journal acknowledgement before clearing intent.
    pub fn mutate<T>(
        &mut self,
        run: &mut Run,
        intent: Intent,
        action: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        if self.state.finished || self.state.pending.is_some() || run.id() != self.state.run {
            return Err("commit owner does not admit another mutation".into());
        }
        self.state.pending = Some(intent.clone());
        self.save()?;
        let value = action()?;
        run.record(&crate::engine::runs::Event::Published { mutation: intent.mutation })?;
        run.sync()?;
        self.state.pending = None;
        self.save()?;
        Ok(value)
    }

    pub fn finish(&mut self) -> Result<(), String> {
        if self.state.pending.is_some() {
            return Err("commit has an unknown mutation outcome".into());
        }
        self.state.finished = true;
        self.save()
    }

    fn save(&self) -> Result<(), String> {
        durable(&self.path, &serde_json::to_vec(&self.state).map_err(|e| e.to_string())?)
    }
}

pub fn durable(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("durable file has no directory")?;
    durable_directory(parent)?;
    let temporary = path.with_extension("pending");
    let mut file =
        OpenOptions::new().create(true).truncate(true).write(true).open(&temporary).map_err(|e| e.to_string())?;
    file.write_all(bytes).and_then(|()| file.sync_all()).map_err(|e| e.to_string())?;
    std::fs::rename(&temporary, path).map_err(|e| e.to_string())?;
    File::open(parent).and_then(|directory| directory.sync_all()).map_err(|e| e.to_string())
}

/// Persist directory entries too, including ancestors created by store setup.
pub fn durable_directory(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    for directory in path.ancestors() {
        File::open(directory).and_then(|file| file.sync_all()).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(unix)]
fn inherit_lock(file: &File) -> Result<(), String> {
    use std::os::fd::AsRawFd;
    let fd = file.as_raw_fd();
    // SAFETY: the owned descriptor is valid. Clear close-on-exec only after exclusive flock.
    unsafe {
        if libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) != 0 {
            return Err("another publication owner or its child still holds the commit lock".into());
        }
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn inherit_lock(_: &File) -> Result<(), String> {
    Err("publication owner needs a Unix host".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests::fixture;

    fn intent() -> Intent {
        Intent {
            mutation: Publication::Uploaded { key: "test/object".into() },
            expected: None,
            desired: Some("bytes".into()),
        }
    }

    #[test]
    fn acknowledged_mutations_clear_intent_and_keep_the_operation_binding() {
        let fixture = fixture("commit-ack");
        let run = Run::create(&fixture.store, "commit").unwrap();
        let id = run.id().to_string();
        let prefix = crate::engine::runs::events(&fixture.store, &id).unwrap();
        drop(run);
        let store = crate::store::Store::at(fixture.store.root().join("fresh/owner/store"));
        let mut run = Run::attach(&store, &id, &prefix).unwrap();
        let directory = store.root().join("commits");
        let mut owner = Owner::open(&directory, run.id(), b"bundle").unwrap();
        let path = owner.path.clone();
        owner
            .mutate(&mut run, intent(), || {
                let saved: State = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                assert_eq!(saved.pending, Some(intent()));
                Ok(())
            })
            .unwrap();
        let saved: State = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(saved.pending.is_none());
        assert!(matches!(
            crate::engine::runs::events(&store, run.id()).unwrap().last(),
            Some(crate::engine::runs::Event::Published { .. })
        ));
        owner.finish().unwrap();
        drop(owner);
        assert!(Owner::open(&directory, run.id(), b"different bundle").is_err());
        assert!(Owner::open(&directory, run.id(), b"bundle").unwrap().finished());
    }

    #[test]
    fn a_failed_mutation_retains_the_barrier_even_when_desired_bytes_are_visible() {
        let fixture = fixture("commit-unknown");
        let mut run = Run::create(&fixture.store, "commit").unwrap();
        let directory = fixture.store.root().join("commits");
        let mut owner = Owner::open(&directory, run.id(), b"bundle").unwrap();
        let remote = fixture.store.root().join("visible-object");
        let result = owner.mutate::<()>(&mut run, intent(), || {
            std::fs::write(&remote, "bytes").unwrap();
            Err("acknowledgement lost".into())
        });
        assert!(result.is_err());
        assert!(owner.finish().is_err());
        drop(owner);
        assert_eq!(std::fs::read(&remote).unwrap(), b"bytes");
        assert!(Owner::open(&directory, run.id(), b"bundle").is_err());
        let later = Run::create(&fixture.store, "later commit").unwrap();
        assert!(Owner::open(&directory, later.id(), b"later bundle").is_err());
    }

    #[test]
    #[cfg(unix)]
    fn a_child_keeps_exclusion_after_its_owner_drops() {
        use std::process::{Command, Stdio};
        let fixture = fixture("commit-child");
        let directory = fixture.store.root().join("commits");
        let owner = Owner::open(&directory, "2026-10-06-120000", b"bundle").unwrap();
        let mut child = Command::new("sh").args(["-c", "read value"]).stdin(Stdio::piped()).spawn().unwrap();
        assert!(Owner::active(&directory, "2026-10-06-120000", &sha256_hex(b"bundle")).unwrap());
        drop(owner);
        assert!(
            Owner::active(&directory, "2026-10-06-120000", &sha256_hex(b"bundle")).unwrap(),
            "the child retains the run and global locks"
        );
        assert!(!Owner::active(&directory, "2026-10-06-120001", &sha256_hex(b"other")).unwrap());
        assert!(Owner::open(&directory, "2026-10-06-120001", b"other").is_err());
        child.stdin.take().unwrap().write_all(b"done\n").unwrap();
        assert!(child.wait().unwrap().success());
        assert!(!Owner::active(&directory, "2026-10-06-120000", &sha256_hex(b"bundle")).unwrap());
        assert!(Owner::open(&directory, "2026-10-06-120001", b"other").is_ok());
    }
}
