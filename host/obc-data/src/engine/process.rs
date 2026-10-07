//! Running a step and measuring what it cost.

use std::io::{ErrorKind, Write};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::time::Instant;

use super::{Code, Request};

pub struct Usage {
    pub wall_ms: u64,
    pub cpu_ms: Option<u64>,
    pub peak_rss_bytes: Option<u64>,
}

/// Run a step in this process. Its CPU time is the time of the whole process, so it is exact only
/// while no other step runs. Its peak is the peak of the whole process so far.
pub fn in_process(step: impl FnOnce() -> Result<(), String>) -> Result<Usage, String> {
    let start = Instant::now();
    let before = own_usage();
    step()?;
    let after = own_usage();
    Ok(Usage {
        wall_ms: start.elapsed().as_millis() as u64,
        cpu_ms: before.zip(after).map(|(before, after)| after.0.saturating_sub(before.0)),
        peak_rss_bytes: after.map(|after| after.1),
    })
}

pub(super) fn command(root: &Path, argv: &[String], code: Option<(&Code, &str)>) -> Result<Command, String> {
    let (program, args) = argv.split_first().ok_or("the command is empty")?;
    let mut command = Command::new(program);
    command.args(args).current_dir(root);
    if let Some((code, expected)) = code {
        if code.python.is_some() {
            super::code::python_command(root, code, expected, &mut command)?;
        } else if !code.crates.is_empty()
            && !matches!(code.rust, Some(super::Rust::Prepared { .. }))
            && matches!(Path::new(program).file_stem().and_then(|name| name.to_str()), Some("cargo" | "rustc"))
        {
            crate::worker::compiler_command(&mut command);
        }
    }
    Ok(command)
}

/// Run `argv` in `root` with the request as JSON on standard input. Its standard output goes to
/// standard error. The usage is that of the process and the children it waited for.
pub fn run(root: &Path, argv: &[String], request: &Request, code: Option<(&Code, &str)>) -> Result<Usage, String> {
    let mut command = command(root, argv, code)?;
    command.stdin(Stdio::piped()).stdout(std::io::stderr());
    let program = &argv[0];
    let start = Instant::now();
    let mut child = command.spawn().map_err(|e| format!("cannot start `{program}`: {e}"))?;
    let json = serde_json::to_vec(request).map_err(|e| e.to_string())?;
    // A step that does not read its request closes the pipe; its exit status tells the rest.
    let unwritten = match child.stdin.take().map(|mut stdin| stdin.write_all(&json)) {
        Some(Err(e)) if e.kind() != ErrorKind::BrokenPipe => Some(e),
        _ => None,
    };
    let (status, cpu_ms, peak_rss_bytes) = wait(child)?;
    if !status.success() {
        return Err(format!("`{}` failed: {status}", argv.join(" ")));
    }
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

/// User plus system CPU milliseconds, and the peak resident set in bytes.
#[cfg(unix)]
fn measure(usage: &libc::rusage) -> (u64, u64) {
    let ms = |time: libc::timeval| time.tv_sec as u64 * 1000 + time.tv_usec as u64 / 1000;
    // macOS counts the peak in bytes; Linux and the BSDs in KiB.
    let unit = if cfg!(target_os = "macos") { 1 } else { 1024 };
    (ms(usage.ru_utime) + ms(usage.ru_stime), usage.ru_maxrss as u64 * unit)
}
