//! Running a step in a child process and measuring what it cost.

use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use super::{Function, Request, Run};

pub struct Usage {
    pub wall_ms: u64,
    pub cpu_ms: Option<u64>,
    pub peak_rss_bytes: Option<u64>,
}

/// The first argument of the child process that runs a Rust step: `<binary> obc_data_step
/// <function>`. A test binary reads its arguments as test names, so it runs its test of this name.
const CHILD: &str = "obc_data_step";

/// When the arguments of this process are `obc_data_step <function>`, run that function of
/// `functions` with the request on standard input, and exit: with 0 when it succeeds, else with 1
/// after its error on standard error. Else return. A binary that runs Rust steps calls it first
/// with the functions of its steps: `obc data` in `main`, a test binary in its test
/// `obc_data_step`.
pub fn child(functions: &[&Function]) {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [first, name] = &args[..] else { return };
    if first != CHILD {
        return;
    }
    let function = functions.iter().find(|function| function.name == name);
    let result = function.ok_or_else(|| format!("this binary has no function `{name}`")).and_then(|function| {
        let request = serde_json::from_reader(std::io::stdin().lock()).map_err(|e| format!("the request: {e}"))?;
        let ran = panic::catch_unwind(AssertUnwindSafe(|| (function.run)(&request)));
        ran.unwrap_or_else(|payload| Err(panicked(payload)))
    });
    if let Err(error) = &result {
        // Not `eprintln!`, which a test binary captures.
        let _ = writeln!(std::io::stderr(), "{error}");
    }
    std::process::exit(if result.is_ok() { 0 } else { 1 })
}

pub(crate) fn panicked(payload: Box<dyn std::any::Any + Send>) -> String {
    let message = payload.downcast_ref::<&str>().map(|text| text.to_string());
    let message = message.or_else(|| payload.downcast_ref::<String>().cloned());
    format!("it panicked: {}", message.unwrap_or_default())
}

/// Run the step `run` in a child process in `root`, in a process group of its own, with the request
/// as JSON on standard input. Its standard output goes to standard error. When `stop` is set, its
/// process group gets SIGTERM, and the step fails. The usage is that of the process and of the
/// children that it waited for.
pub fn run(root: &Path, run: &Run, request: &Request, stop: &AtomicBool) -> Result<Usage, String> {
    let (mut command, name) = match run {
        Run::Rust(function) => {
            let mut command = Command::new(this_binary()?);
            command.args([CHILD, function.name]).current_dir(root);
            (command, function.name.to_string())
        }
        Run::Command(argv) => (command(root, argv, false)?, argv.join(" ")),
        Run::Python(argv) => (command(root, argv, true)?, argv.join(" ")),
    };
    // Temporary files go beside the output, on the disk of the store: `/tmp` can be a tmpfs, whose
    // files take memory that no receipt counts.
    let temporary = request.output.with_file_name("tmp");
    std::fs::create_dir_all(&temporary).map_err(|e| format!("{}: {e}", temporary.display()))?;
    command.env("TMPDIR", &temporary);
    command.stdin(Stdio::piped()).stdout(std::io::stderr()).stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        if let (Run::Rust(_), Some(program)) = (run, std::env::args_os().next()) {
            // `ps` shows `obc-data obc_data_step <function>`, not `/proc/self/exe`.
            command.arg0(program);
        }
        #[cfg(target_os = "linux")]
        let engine = std::process::id();
        // SAFETY: setpgid, prctl and getppid are async-signal-safe. A pre_exec also makes
        // `Command` fork, not vfork: the peak of a forked child starts at the resident set of the
        // engine now, not at its peak.
        unsafe {
            command.pre_exec(move || {
                if libc::setpgid(0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // The step ends with the engine, also when the kernel kills the engine.
                #[cfg(target_os = "linux")]
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 || libc::getppid() as u32 != engine {
                    // No allocation between fork and exec: another thread can hold the malloc lock.
                    return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
                }
                Ok(())
            });
        }
    }
    let start = Instant::now();
    let mut child = command.spawn().map_err(|e| format!("cannot start `{name}`: {e}"))?;
    let errors = copy_lines(child.stderr.take(), "");
    let json = serde_json::to_vec(request).map_err(|e| e.to_string())?;
    // A step that does not read its request closes the pipe; its exit status tells the rest.
    let unwritten = match child.stdin.take().map(|mut stdin| stdin.write_all(&json)) {
        Some(Err(e)) if e.kind() != ErrorKind::BrokenPipe => Some(e),
        _ => None,
    };
    let (status, cpu_ms, peak_rss_bytes) = wait(child, stop)?;
    ended(&name, status, errors)?;
    if let Some(e) = unwritten {
        return Err(format!("cannot write the request to `{name}`: {e}"));
    }
    Ok(Usage { wall_ms: start.elapsed().as_millis() as u64, cpu_ms, peak_rss_bytes })
}

/// The program of this process. On Linux, its image, also when a build replaced its file since.
fn this_binary() -> Result<PathBuf, String> {
    if cfg!(target_os = "linux") {
        return Ok(PathBuf::from("/proc/self/exe"));
    }
    std::env::current_exe().map_err(|e| format!("the program of `obc data`: {e}"))
}

/// Copy `pipe`, an output of a child that a command pipes, to the standard error of this process on
/// a thread, each line that is not blank after `label` and a space. A carriage return ends a line
/// too, so a line that the child writes over shows at once. When the pipe closes, the receiver gets
/// the last line that is not blank.
pub fn copy_lines(pipe: Option<impl Read + Send + 'static>, label: &str) -> Receiver<String> {
    let (send, receive) = mpsc::channel();
    let Some(pipe) = pipe else { return receive };
    let label = if label.is_empty() { String::new() } else { format!("{label} ") };
    std::thread::spawn(move || {
        let (mut pipe, mut last) = (BufReader::new(pipe), String::new());
        loop {
            let mut line = segment(&mut pipe);
            if line.is_empty() {
                break;
            }
            let text = String::from_utf8_lossy(&line).trim().to_string();
            if !text.is_empty() {
                line = [label.as_bytes(), &line].concat();
                last = text;
            }
            if !line.ends_with(b"\n") && !line.ends_with(b"\r") {
                line.push(b'\n');
            }
            let _ = std::io::stderr().write_all(&line);
        }
        let _ = send.send(last);
    });
    receive
}

/// The bytes of `pipe` up to and with the next line feed or carriage return; none at the end.
fn segment(pipe: &mut impl BufRead) -> Vec<u8> {
    let mut line = Vec::new();
    while !line.ends_with(b"\n") && !line.ends_with(b"\r") {
        let buffer = match pipe.fill_buf() {
            Ok([]) => break,
            Ok(buffer) => buffer,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        let end = buffer.iter().position(|&byte| byte == b'\n' || byte == b'\r').map_or(buffer.len(), |at| at + 1);
        line.extend_from_slice(&buffer[..end]);
        pipe.consume(end);
    }
    line
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

/// Wait for `child`, and give its exit status, CPU milliseconds and peak. When `stop` is set, the
/// process group of the child gets SIGTERM, and the wait fails once the child ended.
#[cfg(unix)]
fn wait(child: Child, stop: &AtomicBool) -> Result<(ExitStatus, Option<u64>, Option<u64>), String> {
    wait_pid(child.id(), stop)
}

/// Wait for the child `pid` of this process: its exit status, user plus system CPU milliseconds and
/// peak resident bytes. Once `stop` is set, its process group gets SIGTERM, and the run stopped.
#[cfg(unix)]
pub(crate) fn wait_pid(pid: u32, stop: &AtomicBool) -> Result<(ExitStatus, Option<u64>, Option<u64>), String> {
    use std::os::unix::process::ExitStatusExt;
    let pid = pid as libc::pid_t;
    let (mut stopped, mut pause) = (false, Duration::from_millis(1));
    loop {
        let mut status = 0;
        // SAFETY: rusage is plain data, and wait4 fills it.
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        // SAFETY: the pid is our child, which nothing else waits for.
        match unsafe { libc::wait4(pid, &mut status, libc::WNOHANG, &mut usage) } {
            0 => {}
            -1 => {
                let error = std::io::Error::last_os_error();
                if error.kind() != ErrorKind::Interrupted {
                    return Err(format!("wait for the step: {error}"));
                }
            }
            _ if stopped => return Err("the run stopped".into()),
            _ => {
                let (cpu, peak) = measure(&usage);
                return Ok((ExitStatus::from_raw(status), Some(cpu), Some(peak)));
            }
        }
        if !stopped && stop.load(Ordering::Relaxed) {
            // SAFETY: kill sends a signal. The child is not reaped, so its group is still its own.
            unsafe { libc::kill(-pid, libc::SIGTERM) };
            stopped = true;
        }
        std::thread::sleep(pause);
        pause = (pause * 2).min(Duration::from_millis(10));
    }
}

#[cfg(not(unix))]
fn wait(mut child: Child, _: &AtomicBool) -> Result<(ExitStatus, Option<u64>, Option<u64>), String> {
    Ok((child.wait().map_err(|e| format!("wait for the step: {e}"))?, None, None))
}

/// The peak resident bytes of this process, or, when that is more, `at_once` times the peak of the
/// largest child that it waited for: a bound for `at_once` children that run at the same time.
#[cfg(unix)]
pub fn peak_rss(at_once: u64) -> Option<u64> {
    let peak = |who| {
        // SAFETY: rusage is plain data, and getrusage fills it.
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        (unsafe { libc::getrusage(who, &mut usage) } == 0).then(|| measure(&usage).1)
    };
    peak(libc::RUSAGE_SELF).max(peak(libc::RUSAGE_CHILDREN).map(|peak| peak * at_once))
}

#[cfg(not(unix))]
pub fn peak_rss(_: u64) -> Option<u64> {
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
    fn a_failed_program_names_the_last_line_that_it_wrote_to_standard_error() {
        let ended_by = |script: &str| {
            let mut child = Command::new("sh").args(["-c", script]).stderr(Stdio::piped()).spawn().unwrap();
            let errors = copy_lines(child.stderr.take(), "");
            let status = child.wait().unwrap();
            ended("sh", status, errors).unwrap_err()
        };
        let failed = ended_by("echo working >&2; echo 'ValueError: boom' >&2; echo >&2; exit 3");
        assert_eq!(failed, "`sh` failed (exit status: 3): ValueError: boom");
        // A carriage return ends a line, as a progress that writes over its line does.
        let progress = ended_by("printf 'a.zip: 10%%\\ra.zip: 100%%\\r' >&2; exit 3");
        assert_eq!(progress, "`sh` failed (exit status: 3): a.zip: 100%");
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
