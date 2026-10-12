//! `obc data open MACHINE`: one SSH tunnel to the page of MACHINE, and the browser at its end. The
//! page acts only on MACHINE; this command sends it nothing.

use std::net::TcpStream;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use super::{Code, Error};

/// The local end of the tunnel. The page of this machine keeps 8700.
const LOCAL: &str = "127.0.0.1:8701";

pub fn open(root: &Path, machine: &str) -> Result<(), Error> {
    let host = crate::machines::ssh(root, machine).map_err(|e| Code::InvalidData.error(e))?;
    if TcpStream::connect(LOCAL).is_ok() {
        return Err(Code::Failed
            .error(format!("{LOCAL} is in use, perhaps by the tunnel of another `obc data open`"))
            .fix("Stop that command, then run this one again."));
    }
    let args = ["-N", "-o", "ExitOnForwardFailure=yes", "-L", "8701:localhost:8700", "--", host.as_str()];
    eprintln!("obc data: ssh {}", args.join(" "));
    let mut ssh = Command::new("ssh").args(args).spawn().map_err(|e| {
        Code::Failed.error(format!("ssh does not start: {e}")).fix("Install OpenSSH, then run the command again.")
    })?;
    // ssh may first ask for a host key, a password or a second factor: wait as long as it runs.
    while TcpStream::connect(LOCAL).is_err() {
        if let Some(status) = ssh.try_wait().map_err(|e| e.to_string())? {
            return Err(tunnel_failed(machine, &host, status));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let url = "http://localhost:8701/";
    let browser = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    if Command::new(browser).arg(url).status().is_err() {
        eprintln!("obc data: open {url} in a browser");
    }
    eprintln!("obc data: the page of {machine} is at {url} until you stop this command (Ctrl-C)");
    let status = ssh.wait().map_err(|e| e.to_string())?;
    if status.success() {
        return Ok(());
    }
    Err(tunnel_failed(machine, &host, status))
}

fn tunnel_failed(machine: &str, host: &str, status: std::process::ExitStatus) -> Error {
    Code::Failed.error(format!("the SSH tunnel to `{host}` stopped ({status})")).fix(format!(
        "Check that `ssh {host}` works and that `obc data ui` runs on {machine}. Port 8701 on this machine must be free."
    ))
}
