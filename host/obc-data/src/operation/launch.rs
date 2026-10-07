//! A detached worker leaves the terminal session.

use std::path::Path;
use std::process::{Command, Stdio};

/// Detach `command` into its own session, with its output in the operation `directory`.
#[cfg(unix)]
pub fn detach(command: &mut Command, directory: &Path) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::process::CommandExt;
    let log = |name: &str| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(directory.join(name))
            .map_err(|e| e.to_string())
    };
    command.stdin(Stdio::null()).stdout(log("stdout.json")?).stderr(log("stderr.log")?);
    // SAFETY: setsid is async-signal-safe and runs before exec, with no Rust allocation.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(())
}
