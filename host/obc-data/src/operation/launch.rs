//! The two supported detached hosts. Environment files belong to host setup, never a bundle.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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
