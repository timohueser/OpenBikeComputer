//! Running a step and measuring what it cost.

use std::cell::{Cell, RefCell};
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::Request;

pub struct Usage {
    pub wall_ms: u64,
    pub cpu_ms: Option<u64>,
    pub peak_rss_bytes: Option<u64>,
}

thread_local! {
    /// The CPU milliseconds and the largest peak of the programs that the Rust step of this thread
    /// ran with [`run_child`].
    static CHILDREN: Cell<(u64, u64)> = const { Cell::new((0, 0)) };
    /// Set when the run of the Rust step of this thread stops.
    static STOP: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

/// Run a step in this process, on this thread. Its CPU time is the time of the whole process, so it
/// is exact only while no other step runs. Its peak is the peak of the whole process so far, not
/// the peak of the step: an upper bound, as the scheduler needs. The programs that the step runs
/// with [`run_child`], such as Osmium, add their CPU time, and their peak when it is more. When
/// `stop` is set, [`check_stop`] on this thread fails.
pub fn in_process(stop: Arc<AtomicBool>, step: impl FnOnce() -> Result<(), String>) -> Result<Usage, String> {
    let start = Instant::now();
    CHILDREN.set((0, 0));
    STOP.set(Some(stop));
    let before = own_usage();
    step()?;
    let after = own_usage();
    let (cpu, peak) = CHILDREN.get();
    Ok(Usage {
        wall_ms: start.elapsed().as_millis() as u64,
        cpu_ms: before.zip(after).map(|(before, after)| after.0.saturating_sub(before.0) + cpu),
        peak_rss_bytes: after.map(|after| after.1.max(peak)),
    })
}

/// An error when the run of the Rust step of this thread stops. A long step calls it in its loops,
/// so that a stop ends the step in seconds. It is one atomic load. Outside a step it never fails.
pub fn check_stop() -> Result<(), String> {
    match STOP.with_borrow(|stop| stop.as_ref().is_some_and(|stop| stop.load(Ordering::Relaxed))) {
        true => Err("the run stopped".into()),
        false => Ok(()),
    }
}

/// Run `command` for the Rust step of this thread, and wait for it: its CPU time and its peak
/// count as those of the step.
pub fn run_child(command: &mut Command) -> Result<ExitStatus, String> {
    let (status, cpu, peak) = wait(command.spawn().map_err(|e| e.to_string())?)?;
    let (all, largest) = CHILDREN.get();
    CHILDREN.set((all + cpu.unwrap_or(0), largest.max(peak.unwrap_or(0))));
    Ok(status)
}

/// Copy the standard error of `child`, which a command pipes, to that of this process on a thread.
/// When the pipe closes, the receiver gets the last line that is not blank.
pub fn copy_errors(child: &mut Child) -> Receiver<String> {
    let (send, receive) = mpsc::channel();
    if let Some(errors) = child.stderr.take() {
        std::thread::spawn(move || {
            let mut last = String::new();
            for line in BufReader::new(errors).split(b'\n') {
                let Ok(line) = line else { break };
                let mut log = std::io::stderr().lock();
                let _ = log.write_all(&line).and_then(|()| log.write_all(b"\n"));
                let text = String::from_utf8_lossy(&line);
                if !text.trim().is_empty() {
                    last = text.trim().to_string();
                }
            }
            let _ = send.send(last);
        });
    }
    receive
}

/// Wait until the copy of `errors` ends, and give the error of the program `name` that ended
/// with `status`, with its last error line, such as a Python exception. A program that a signal
/// ended gets no line: what it wrote last is not why it ended. A grandchild that keeps the pipe
/// open delays this by two seconds at most.
pub fn ended(name: &str, status: ExitStatus, errors: Receiver<String>) -> Result<(), String> {
    let last = errors.recv_timeout(Duration::from_secs(2)).unwrap_or_default();
    match status.code() {
        _ if status.success() => Ok(()),
        Some(_) if !last.is_empty() => Err(format!("`{name}` failed ({status}): {last}")),
        _ => Err(format!("`{name}` failed: {status}")),
    }
}

/// The interpreter of a Python step: the system Python that `uv python find` selects for the
/// project, offline, with an explicit `UV_PYTHON` request when set.
pub fn python_executable(root: &Path) -> Result<PathBuf, String> {
    let mut find = Command::new("uv");
    find.current_dir(root).args(["python", "find", "--system", "--offline", "--no-python-downloads"]);
    if let Some(request) = std::env::var_os("UV_PYTHON") {
        find.arg(request);
    }
    let output = find.output().map_err(|e| format!("uv offline interpreter: {e}; prepare the Python runtime first"))?;
    if !output.status.success() {
        return Err(format!(
            "uv offline interpreter: {}; prepare the selected offline Python runtime first",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let path = String::from_utf8(output.stdout).map_err(|e| format!("uv offline interpreter: {e}"))?;
    Ok(PathBuf::from(path.trim()))
}

/// The command `argv` in `root`. A `python` command runs under `uv run` with the interpreter of
/// [`python_executable`].
fn command(root: &Path, argv: &[String], python: bool) -> Result<Command, String> {
    let (program, args) = argv.split_first().ok_or("the command is empty")?;
    let mut command = Command::new(program);
    command.args(args).current_dir(root);
    if python {
        command.env("UV_PYTHON", python_executable(root)?);
        // A no-sync override can make uv keep a project interpreter of another version.
        command.env("UV_NO_SYNC", "0");
    }
    Ok(command)
}

/// Run the [`command`] `argv` in `root` with the request as JSON on standard input. Its standard
/// output goes to standard error. The usage is that of the process and the children it waited for.
pub fn run(root: &Path, argv: &[String], request: &Request, python: bool) -> Result<Usage, String> {
    let mut command = command(root, argv, python)?;
    let program = &argv[0];
    command.stdin(Stdio::piped()).stdout(std::io::stderr()).stderr(Stdio::piped());
    let start = Instant::now();
    let mut child = command.spawn().map_err(|e| format!("cannot start `{program}`: {e}"))?;
    let errors = copy_errors(&mut child);
    let json = serde_json::to_vec(request).map_err(|e| e.to_string())?;
    // A step that does not read its request closes the pipe; its exit status tells the rest.
    let unwritten = match child.stdin.take().map(|mut stdin| stdin.write_all(&json)) {
        Some(Err(e)) if e.kind() != ErrorKind::BrokenPipe => Some(e),
        _ => None,
    };
    let (status, cpu_ms, peak_rss_bytes) = wait(child)?;
    ended(&argv.join(" "), status, errors)?;
    if let Some(e) = unwritten {
        return Err(format!("cannot write the request to `{program}`: {e}"));
    }
    Ok(Usage { wall_ms: start.elapsed().as_millis() as u64, cpu_ms, peak_rss_bytes })
}

#[cfg(unix)]
fn wait(child: std::process::Child) -> Result<(ExitStatus, Option<u64>, Option<u64>), String> {
    use std::os::unix::process::ExitStatusExt;
    let mut status = 0;
    // SAFETY: rusage is plain data, and wait4 fills it.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    loop {
        // SAFETY: the pid is our child, which nothing else waits for.
        if unsafe { libc::wait4(child.id() as libc::pid_t, &mut status, 0, &mut usage) } >= 0 {
            break;
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != ErrorKind::Interrupted {
            return Err(format!("wait for the step: {error}"));
        }
    }
    let (cpu, peak) = measure(&usage);
    Ok((ExitStatus::from_raw(status), Some(cpu), Some(peak)))
}

#[cfg(not(unix))]
fn wait(mut child: std::process::Child) -> Result<(ExitStatus, Option<u64>, Option<u64>), String> {
    Ok((child.wait().map_err(|e| format!("wait for the step: {e}"))?, None, None))
}

/// CPU milliseconds and peak resident bytes of this process so far.
#[cfg(unix)]
fn own_usage() -> Option<(u64, u64)> {
    // SAFETY: rusage is plain data, and getrusage fills it.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    (unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } == 0).then(|| measure(&usage))
}

#[cfg(not(unix))]
fn own_usage() -> Option<(u64, u64)> {
    None
}

/// The peak resident bytes of this process, or of the largest child that it waited for, when that is
/// more.
#[cfg(unix)]
pub fn peak_rss() -> Option<u64> {
    let peak = |who| {
        // SAFETY: rusage is plain data, and getrusage fills it.
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        (unsafe { libc::getrusage(who, &mut usage) } == 0).then(|| measure(&usage).1)
    };
    peak(libc::RUSAGE_SELF).max(peak(libc::RUSAGE_CHILDREN))
}

#[cfg(not(unix))]
pub fn peak_rss() -> Option<u64> {
    None
}

/// User plus system CPU milliseconds, and the peak resident set in bytes.
#[cfg(unix)]
fn measure(usage: &libc::rusage) -> (u64, u64) {
    let ms = |time: libc::timeval| time.tv_sec as u64 * 1000 + time.tv_usec as u64 / 1000;
    // macOS counts the peak in bytes; Linux and the BSDs in KiB.
    let unit = if cfg!(target_os = "macos") { 1 } else { 1024 };
    (ms(usage.ru_utime) + ms(usage.ru_stime), usage.ru_maxrss as u64 * unit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests::write;
    use crate::store::tests::Scratch;

    #[cfg(unix)]
    #[test]
    fn a_rust_step_has_the_peak_of_a_program_that_it_runs() {
        let usage = in_process(Arc::default(), || {
            run_child(Command::new("python3").args(["-c", "b'x' * (600 << 20)"])).map(drop)
        })
        .unwrap();
        assert!(usage.peak_rss_bytes.unwrap() > 600 << 20, "{:?}", usage.peak_rss_bytes);
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_program_names_the_last_line_that_it_wrote_to_standard_error() {
        let ended_by = |script: &str| {
            let mut child = Command::new("sh").args(["-c", script]).stderr(Stdio::piped()).spawn().unwrap();
            let errors = copy_errors(&mut child);
            let status = child.wait().unwrap();
            ended("sh", status, errors).unwrap_err()
        };
        let failed = ended_by("echo working >&2; echo 'ValueError: boom' >&2; echo >&2; exit 3");
        assert_eq!(failed, "`sh` failed (exit status: 3): ValueError: boom");
        let stopped = ended_by("echo 'a warning' >&2; kill -TERM $$");
        assert_eq!(stopped, "`sh` failed: signal: 15 (SIGTERM)");
    }

    #[test]
    fn a_python_step_runs_the_discovered_interpreter_despite_active_and_project_environments() {
        let scratch = Scratch::new("python-interpreter");
        let root = &scratch.0;
        write(&root.join(".python-version"), "3.12\n");
        write(
            &root.join("pyproject.toml"),
            "[project]\nname = \"runtime-fixture\"\nversion = \"0\"\nrequires-python = \">=3.12\"\ndependencies = []\n\
             [tool.uv]\npackage = false\ndefault-groups = []\n",
        );
        write(
            &root.join("uv.lock"),
            "version = 1\nrevision = 3\nrequires-python = \">=3.12\"\n[[package]]\nname = \"runtime-fixture\"\n\
             version = \"0\"\nsource = { virtual = \".\" }\n",
        );
        let base = python_executable(root).unwrap();
        let (active, project) = (root.join("active"), root.join("project"));
        for path in [&active, &project] {
            let result = Command::new(&base).args(["-m", "venv", "--without-pip"]).arg(path).output().unwrap();
            assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        }
        let script = "import json,sys; print(json.dumps([sys._base_executable, sys.prefix]))";
        let argv = ["uv", "run", "--locked", "--offline", "--no-default-groups", "--no-python-downloads"];
        let argv: Vec<String> = argv.iter().chain(&["python", "-c", script]).map(|arg| arg.to_string()).collect();
        for override_project in [false, true] {
            let mut command = command(root, &argv, true).unwrap();
            let no_sync = command.get_envs().find(|(key, _)| *key == "UV_NO_SYNC").and_then(|(_, value)| value);
            assert_eq!(no_sync, Some(std::ffi::OsStr::new("0")));
            command.env_remove("UV_PROJECT_ENVIRONMENT").env("VIRTUAL_ENV", &active);
            if override_project {
                command.env("UV_PROJECT_ENVIRONMENT", &project);
            }
            let output = command.output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            let [used, prefix]: [PathBuf; 2] = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(used.canonicalize().unwrap(), base.canonicalize().unwrap());
            let expected = if override_project { project.clone() } else { root.join(".venv") };
            assert_eq!(prefix.canonicalize().unwrap(), expected.canonicalize().unwrap());
        }
    }
}
