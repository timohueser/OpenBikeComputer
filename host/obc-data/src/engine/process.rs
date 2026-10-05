//! Running a step, offline, and measuring what it cost.

use std::io::{ErrorKind, Write};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::time::Instant;

use super::Request;

pub struct Usage {
    pub wall_ms: u64,
    pub cpu_ms: Option<u64>,
    pub peak_rss_bytes: Option<u64>,
}

/// Run a step in this process. Its CPU time is the time of the whole process, so it is exact only
/// while no other step runs. Its peak is known only when it raised the peak of the process.
pub fn in_process(step: impl FnOnce() -> Result<(), String>) -> Result<Usage, String> {
    let start = Instant::now();
    let before = own_usage();
    step()?;
    let after = own_usage();
    Ok(Usage {
        wall_ms: start.elapsed().as_millis() as u64,
        cpu_ms: before.zip(after).map(|(before, after)| after.0.saturating_sub(before.0)),
        peak_rss_bytes: before.zip(after).and_then(|(before, after)| (after.1 > before.1).then_some(after.1)),
    })
}

/// Run `argv` in `root` with the request as JSON on standard input. Its standard output goes to
/// standard error. The usage is that of the process and the children it waited for.
pub fn run(root: &Path, argv: &[String], request: &Request) -> Result<Usage, String> {
    let (program, args) = argv.split_first().ok_or("the command is empty")?;
    let mut command = Command::new(program);
    command.args(args).current_dir(root).stdin(Stdio::piped()).stdout(std::io::stderr());
    offline(&mut command);
    let start = Instant::now();
    let mut child = command.spawn().map_err(|e| match e.kind() {
        ErrorKind::NotFound => format!("cannot start `{program}`: {e}"),
        _ => format!("cannot start `{program}` offline: {e}{OFFLINE_HINT}"),
    })?;
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

#[cfg(target_os = "linux")]
const OFFLINE_HINT: &str = ". A command step runs in its own user and network namespace; on Ubuntu, \
     `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0` allows them";
#[cfg(not(target_os = "linux"))]
const OFFLINE_HINT: &str = "";

/// Start the command in a new user namespace that maps the user to itself, and a new network
/// namespace whose one interface, loopback, is down: every connection fails. When the kernel
/// refuses, the spawn fails.
#[cfg(target_os = "linux")]
fn offline(command: &mut Command) {
    use std::ffi::CStr;
    use std::os::unix::process::CommandExt;

    // The child runs this between fork and exec, so it allocates nothing.
    fn write(path: &CStr, bytes: &[u8]) -> std::io::Result<()> {
        // SAFETY: `path` is a C string, and `bytes` lives for the call.
        let written = unsafe {
            let fd = libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC);
            if fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let written = libc::write(fd, bytes.as_ptr().cast(), bytes.len());
            libc::close(fd);
            written
        };
        if written == bytes.len() as isize {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    // SAFETY: getuid and getgid cannot fail.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    let uid_map = format!("{uid} {uid} 1");
    let gid_map = format!("{gid} {gid} 1");
    let isolate = move || {
        // SAFETY: unshare changes only the namespaces of this child.
        if unsafe { libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNET) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        write(c"/proc/self/setgroups", b"deny")?;
        write(c"/proc/self/uid_map", uid_map.as_bytes())?;
        write(c"/proc/self/gid_map", gid_map.as_bytes())
    };
    // SAFETY: `isolate` makes only async-signal-safe calls and allocates nothing.
    unsafe { command.pre_exec(isolate) };
}

/// Only Linux isolates a command step; elsewhere it runs with the network.
#[cfg(not(target_os = "linux"))]
fn offline(_: &mut Command) {}

/// Whether this machine can start a command step offline.
#[cfg(test)]
pub fn offline_allowed() -> bool {
    let mut command = Command::new("true");
    offline(&mut command);
    command.status().is_ok_and(|status| status.success())
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
