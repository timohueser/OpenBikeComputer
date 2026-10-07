//! The two supported detached hosts. Environment files belong to host setup, never a bundle.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Check Linux setup before reserving a durable run.
pub fn preflight() -> Result<Option<PathBuf>, String> {
    #[cfg(target_os = "linux")]
    {
        let file = environment()?;
        let manager = Command::new("systemctl")
            .args(["--user", "is-active", "default.target"])
            .output()
            .map_err(|_| "set up the systemd user manager before starting a detached run")?;
        // SAFETY: geteuid only reads the current process's identity.
        let user = unsafe { libc::geteuid() }.to_string();
        let linger = Command::new("loginctl")
            .args(["show-user", &user, "--property=Linger", "--value"])
            .output()
            .map_err(|_| "enable linger for the configured operator before starting a detached run")?;
        if !manager.status.success() || !linger.status.success() || linger.stdout != b"yes\n" {
            return Err("detached runs need an active systemd user manager and enabled linger".into());
        }
        Ok(Some(file))
    }
    #[cfg(target_os = "macos")]
    {
        Ok(None)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Err("detached data runs support macOS or a configured Linux systemd host".into())
    }
}

pub fn environment() -> Result<PathBuf, String> {
    let file = std::env::var_os("OBC_RUN_ENV_FILE")
        .ok_or("set OBC_RUN_ENV_FILE to the private systemd environment file on this host")?;
    private_file(Path::new(&file))
}

pub(crate) fn private_file(file: &Path) -> Result<PathBuf, String> {
    if !file.is_absolute() {
        return Err("OBC_RUN_ENV_FILE must be an absolute path".into());
    }
    let file = file.canonicalize().map_err(|e| format!("OBC_RUN_ENV_FILE: {e}"))?;
    let opened = std::fs::File::open(&file).map_err(|e| format!("OBC_RUN_ENV_FILE: {e}"))?;
    let metadata = opened.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("OBC_RUN_ENV_FILE must be a readable regular file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid only reads the current process's identity.
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            return Err("OBC_RUN_ENV_FILE must be a readable regular file owned privately by the operator".into());
        }
    }
    Ok(file)
}

/// The command has no credential values, shell, login scope, or automatic restart.
pub fn service(unit: &str, root: &Path, directory: &Path, file: &Path, user: bool) -> Command {
    serving(unit, root, directory, file, user)
}

/// Known app serving is independent of finite bake worker resource policy.
pub fn serving(unit: &str, root: &Path, directory: &Path, file: &Path, user: bool) -> Command {
    let mut command = Command::new("systemd-run");
    command.args(["--quiet", "--collect", "--service-type=exec"]);
    if user {
        command.arg("--user");
    }
    command
        .arg(format!("--unit={unit}"))
        .arg(format!("--property=EnvironmentFile={}", file.display()))
        .arg(format!("--property=WorkingDirectory={}", root.display()))
        .arg(format!("--property=StandardOutput=append:{}", directory.join("stdout.json").display()))
        .arg(format!("--property=StandardError=append:{}", directory.join("stderr.log").display()))
        .arg("--property=StandardInput=null")
        .arg("--property=UMask=0077")
        .arg("--property=Restart=no")
        .arg("--property=KillMode=control-group")
        .arg("--property=RuntimeMaxSec=infinity")
        .stdin(Stdio::null());
    command
}

/// Only retained bake workers and their children enter the operator's shared budget.
pub fn bake(unit: &str, root: &Path, directory: &Path, file: &Path, budget: &super::budget::Budget) -> Command {
    let mut command = service(unit, root, directory, file, true);
    command
        .arg(format!("--property=Slice={}", super::budget::SLICE))
        .arg(format!("--property=OnFailure={}", budget.alert));
    command
}

#[cfg(target_os = "macos")]
pub fn mac(command: &mut Command, directory: &Path) -> Result<(), String> {
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

/// Only the local read transport is stopped. The remote publication owner keeps its locks.
pub fn bounded_output(command: &mut Command, limit: Duration) -> Result<Vec<u8>, String> {
    let (status, bytes) = bounded_status(command, limit)?;
    if status.success() {
        Ok(bytes)
    } else {
        Err("host command failed; its result was not accepted".into())
    }
}

pub(crate) fn bounded_status(
    command: &mut Command,
    limit: Duration,
) -> Result<(std::process::ExitStatus, Vec<u8>), String> {
    bounded_read(command, limit, Stdio::null())
}

pub(crate) fn bounded_input(command: &mut Command, input: &[u8], limit: Duration) -> Result<Vec<u8>, String> {
    if input.len() > 128 * 1024 {
        return Err("host observation request is too large".into());
    }
    let scratch = crate::r2::Scratch::new()?;
    let path = scratch.0.join("request.json");
    std::fs::write(&path, input).map_err(|e| e.to_string())?;
    let (status, output) =
        bounded_read(command, limit, Stdio::from(std::fs::File::open(path).map_err(|e| e.to_string())?))?;
    if status.success() {
        Ok(output)
    } else {
        Err("read-only host observation failed; check the installed helper and permissions".into())
    }
}

fn bounded_read(
    command: &mut Command,
    limit: Duration,
    input: Stdio,
) -> Result<(std::process::ExitStatus, Vec<u8>), String> {
    #[cfg(not(unix))]
    {
        let _ = (command, limit, input);
        Err("host observation needs a supported Unix host".into())
    }
    #[cfg(unix)]
    {
        use std::io::Read;
        use std::os::fd::AsRawFd;
        use std::os::unix::process::CommandExt;
        use std::process::Stdio;
        use std::time::Instant;
        let mut child = command
            .process_group(0)
            .stdin(input)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("host observation could not start: {e}"))?;
        let mut stdout = child.stdout.take().expect("piped status output");
        let deadline = Instant::now() + limit;
        let result = (|| -> Result<(std::process::ExitStatus, Vec<u8>), String> {
            // SAFETY: stdout owns this pipe descriptor. Nonblocking reads keep collection within the same deadline.
            unsafe {
                let flags = libc::fcntl(stdout.as_raw_fd(), libc::F_GETFL);
                if flags < 0 || libc::fcntl(stdout.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
                    return Err(std::io::Error::last_os_error().to_string());
                }
            }
            let mut bytes = Vec::new();
            let mut buffer = [0; 16384];
            let mut status = None;
            let mut eof = false;
            loop {
                match stdout.read(&mut buffer) {
                    Ok(0) => eof = true,
                    Ok(count) => {
                        bytes.extend_from_slice(&buffer[..count]);
                        if bytes.len() > 1024 * 1024 {
                            return Err("host observation output is too large".into());
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(error.to_string()),
                }
                if status.is_none() {
                    status = child.try_wait().map_err(|e| e.to_string())?;
                }
                if let Some(status) = status {
                    if eof {
                        return Ok((status, bytes));
                    }
                }
                if Instant::now() >= deadline {
                    return Err("host observation deadline expired; its result stays unknown".into());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        })();
        // SAFETY: this child owns a fresh process group. No publication process runs in it.
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
        let _ = child.kill();
        let _ = child.wait();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_launch_binds_private_files_without_copying_secrets_or_a_login_pipe() {
        let command = service(
            "obc-data-run-2026-10-06-120000",
            Path::new("/checkout"),
            Path::new("/store/operations/run"),
            Path::new("/private/operator.env"),
            true,
        );
        let args: Vec<_> = command.get_args().map(|arg| arg.to_string_lossy().to_string()).collect();
        assert!(args.contains(&"--user".into()));
        assert!(args.contains(&"--property=EnvironmentFile=/private/operator.env".into()));
        assert!(args.contains(&"--property=StandardOutput=append:/store/operations/run/stdout.json".into()));
        assert!(!args.iter().any(|arg| arg == "--pipe" || arg == "--scope" || arg.starts_with("--setenv")));
        assert!(args.contains(&"--property=Restart=no".into()));
        let budget = super::super::budget::Budget {
            cpu_percent: 100,
            memory_bytes: 1024,
            minimum_free: 512,
            alert: "operator-alert.service".into(),
        };
        let bake = bake("bake", Path::new("/checkout"), Path::new("/run"), Path::new("/operator.env"), &budget);
        let args: Vec<_> = bake.get_args().map(|arg| arg.to_string_lossy().to_string()).collect();
        assert!(args.contains(&format!("--property=Slice={}", super::super::budget::SLICE)));
        assert!(args.contains(&"--property=OnFailure=operator-alert.service".into()));
        let owner = service("commit", Path::new("/incoming"), Path::new("/incoming"), Path::new("/owner.env"), false);
        assert!(!owner
            .get_args()
            .any(|arg| arg.to_string_lossy().contains("Slice=") || arg.to_string_lossy().contains("OnFailure=")));
    }

    #[cfg(unix)]
    #[test]
    fn read_only_transport_passes_bounded_input_and_drains_at_the_same_deadline() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "cat"]);
        assert_eq!(
            bounded_input(&mut command, b"exact published slots", Duration::from_secs(2)).unwrap(),
            b"exact published slots"
        );
        let mut blocked = Command::new("/bin/sh");
        blocked.args(["-c", "sleep 10"]);
        assert!(bounded_input(&mut blocked, b"request", Duration::from_millis(20)).unwrap_err().contains("deadline"));
        assert!(bounded_input(&mut command, &vec![0; 128 * 1024 + 1], Duration::from_secs(2))
            .unwrap_err()
            .contains("too large"));
    }

    #[cfg(unix)]
    #[test]
    fn the_environment_file_is_readable_and_private_to_the_operator() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = crate::store::tests::Scratch::new("operation-environment");
        let file = scratch.0.join("operator.env");
        std::fs::write(&file, "PATH=/usr/bin\nOBC_R2_SECRET=private\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(private_file(&file).unwrap(), file.canonicalize().unwrap());
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(private_file(&file).unwrap_err().contains("owned privately"));
        assert!(private_file(Path::new("relative.env")).is_err());
    }
}
