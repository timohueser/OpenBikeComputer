//! What publishing to prod and each deploy need, checked now from this machine, each with the fix
//! that a person applies. A check only reads: an SSH login that runs [`PROBE`], the host key of the
//! VPS, a GET of the Cloudflare API, and a fetch of `origin/develop`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use super::{git, What, VPS};
use crate::fetch::http::Http;
use crate::r2::{Bucket, Credentials};

/// The longest that a check that reaches another machine takes.
const LIMIT: Duration = Duration::from_secs(15);

/// One thing that an action needs.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Check {
    pub name: String,
    /// Why it fails; `null` when it passes.
    pub problem: Option<String>,
    /// What to do about the problem, and where the commands run.
    pub fix: Option<String>,
    /// The commands of the fix, one per line.
    pub commands: Vec<String>,
    /// The arguments after `obc data` of the command that applies the fix, which the page runs
    /// after a confirm; `null` when a person applies it.
    pub action: Option<Vec<String>>,
    /// Whether the action waits for it.
    pub blocks: bool,
}

impl Check {
    fn passed(name: &str) -> Self {
        Check { name: name.into(), problem: None, fix: None, commands: Vec::new(), action: None, blocks: true }
    }

    fn failed(name: &str, problem: impl Into<String>, fix: impl Into<String>, commands: Vec<String>) -> Self {
        Check { problem: Some(problem.into()), fix: Some(fix.into()), commands, ..Check::passed(name) }
    }

    pub fn ok(&self) -> bool {
        self.problem.is_none()
    }
}

/// The first check of `checks` that fails and that the action waits for.
pub fn blocking(checks: &[Check]) -> Option<&Check> {
    checks.iter().find(|check| !check.ok() && check.blocks)
}

/// What `update prod` and the delete of its old data need on this machine.
pub fn publish() -> Vec<Check> {
    let mut checks = vec![match Bucket::from_env(Credentials::Main) {
        Ok(_) => Check::passed("R2 key"),
        Err(e) => Check::failed(
            "R2 key",
            e,
            "Put the OBC_R2_* variables of the bucket in ~/.config/openbikecomputer/credentials.env: one \
             NAME=VALUE line each, mode 0600. Then check again.",
            Vec::new(),
        ),
    }];
    let rclone = Command::new("rclone").arg("version").stdout(Stdio::null()).stderr(Stdio::null()).status();
    checks.push(match rclone {
        Ok(status) if status.success() => Check::passed("rclone"),
        _ => Check::failed(
            "rclone",
            "rclone is not on PATH",
            "Install it on this machine.",
            vec![if cfg!(target_os = "macos") { "brew install rclone" } else { "sudo apt install rclone" }.into()],
        ),
    });
    checks
}

/// Whether this machine has the credential of `what`. The page sets it.
pub fn credential(what: What) -> Check {
    let credential = what.credential();
    let name = "credential";
    if !credential.present() {
        return Check::failed(
            name,
            format!("{} needs {}, which this machine does not have", what.label(), credential.describe()),
            format!(
                "Set it under Services on the prod screen of `obc data ui`, or with `obc data deploy credential {}`.",
                what.name()
            ),
            Vec::new(),
        );
    }
    // `ssh` would read a host that starts with `-` as an option.
    if what == What::Services && std::env::var(VPS).is_ok_and(|host| host.starts_with('-')) {
        return Check::failed(name, format!("{VPS} starts with `-`"), "Set the SSH host of the VPS again.", Vec::new());
    }
    Check::passed(name)
}

/// Whether the files that a deploy of `what` reads from the checkout, not from the commit, hold
/// only what the commit holds: `planner/tiles` for `wrangler deploy`, and the script and the
/// runtime file of the follower.
pub fn clean(root: &Path, what: What) -> Check {
    let paths: &[&str] = match what {
        What::Services => &["tools/vps", "data/planner-runtime.toml"],
        What::Tiles => &["planner/tiles"],
    };
    let name = "files committed";
    match git(root, &[&["status", "--porcelain", "--"], paths].concat()) {
        Ok(changes) if changes.is_empty() => Check::passed(name),
        Ok(_) => Check::failed(
            name,
            format!("{} has changes that the commit does not hold", paths.join(" or ")),
            "Commit or remove them in this checkout.",
            vec![format!("git status -- {}", paths.join(" "))],
        ),
        Err(e) => Check::failed(name, e.message, e.fix, Vec::new()),
    }
}

/// Whether `commit` is on `origin/develop`, after a fetch: a deploy puts only reviewed code in
/// place, and an older commit of develop serves to go back.
pub fn merged(root: &Path, commit: &str) -> Check {
    let name = "commit on origin/develop";
    let mut fetch = Command::new("git");
    fetch.args(["fetch", "--quiet", "origin", "develop"]).current_dir(root);
    // Never a prompt on the terminal: a check without an answer fails.
    fetch.env("GIT_TERMINAL_PROMPT", "0").env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    let fetch = limited(&mut fetch);
    if let Err(e) = fetch.and_then(|output| succeeded("git fetch", output)) {
        return Check::failed(name, e, "Check the network and the remote `origin`, then check again.", Vec::new());
    }
    let mut ancestor = Command::new("git");
    ancestor.args(["merge-base", "--is-ancestor", commit, "origin/develop"]).current_dir(root);
    match ancestor.status() {
        Ok(status) if status.success() => Check::passed(name),
        _ => Check::failed(
            name,
            format!("commit {} of this checkout is not on origin/develop", &commit[..commit.len().min(12)]),
            "Check out a commit of develop in this checkout, then start `obc data ui` again. A pull request \
             deploys after it merges.",
            vec!["git switch --detach origin/develop".into()],
        ),
    }
}

/// Whether the Cloudflare token of this machine may list the Workers of its account.
pub fn token() -> Check {
    let [token, account] =
        ["CLOUDFLARE_API_TOKEN", "CLOUDFLARE_ACCOUNT_ID"].map(|name| std::env::var(name).unwrap_or_default());
    let api = "https://api.cloudflare.com";
    let url = format!("{api}/client/v4/accounts/{account}/workers/scripts");
    let name = "Cloudflare token";
    match Http::new().with_bearer(api, token).answer(&url, "application/json") {
        Ok((200, _)) => Check::passed(name),
        Ok((status, body)) => {
            let body: Value = serde_json::from_slice(&body).unwrap_or_default();
            let said = body["errors"][0]["message"].as_str().unwrap_or("no reason");
            Check::failed(
                name,
                format!("Cloudflare refuses to list the Workers of account {account}: HTTP {status}, {said}"),
                "Create a token with the template \"Edit Cloudflare Workers\": the account of the ID, and the \
                 zone openbikecomputer.com. The ID is under Account details in the Workers & Pages overview. \
                 Then replace the credential here.",
                Vec::new(),
            )
        }
        Err(e) => Check::failed(name, e, "Check the network of this machine, then check again.", Vec::new()),
    }
}

/// The script that the VPS runs for [`services`]: one line for each piece of its one-time setup
/// that it has. It changes nothing.
const PROBE: &str = r#"id -u obc-data >/dev/null 2>&1 && echo user
[ -d /opt/obc-data ] && echo opt
[ -x /var/lib/obc-data/.cargo/bin/cargo ] && echo rust
[ -x /var/lib/obc-data/.local/bin/uv ] && echo uv
command -v cc >/dev/null && echo cc
echo "python=$(python3 -c 'import sys; print(*sys.version_info[:2], sep=".")' 2>/dev/null)"
echo "node=$(node -p 'process.versions.node.split(".").slice(0, 2).join(".")' 2>/dev/null)"
systemctl is-enabled obc-data-serve >/dev/null 2>&1 && echo unit
[ -f /etc/caddy/planner/obc-data-serve.caddy ] && echo caddy
true"#;

/// The options of every SSH check: no prompt, no host key that this machine does not know, and an
/// end within [`LIMIT`].
const SSH: [&str; 10] = [
    "-o",
    "BatchMode=yes",
    "-o",
    "StrictHostKeyChecking=yes",
    "-o",
    "ConnectTimeout=8",
    "-o",
    "ServerAliveInterval=5",
    "-o",
    "ServerAliveCountMax=2",
];

/// What a deploy of the follower needs: the SSH host, a login of this machine as root without a
/// prompt, and the one-time setup of `tools/vps/README.md` on the VPS, each missing piece with its
/// command. `python` and `node` are the `major.minor` of `data/planner-runtime.toml`.
pub fn services(home: Option<&Path>, (python, node): (&str, &str)) -> Vec<Check> {
    let credential = credential(What::Services);
    if !credential.ok() {
        return vec![credential];
    }
    let host = std::env::var(VPS).unwrap_or_default();
    let probe = limited(Command::new("ssh").args(SSH).arg("--").arg(&host).arg(PROBE));
    let login = "login as root";
    let output = match probe {
        Ok(output) if output.status.success() => output,
        Ok(output) => return vec![credential, refused(login, &host, &String::from_utf8_lossy(&output.stderr), home)],
        Err(e) => return vec![credential, Check::failed(login, e, "Check the host and the network.", Vec::new())],
    };
    let said = String::from_utf8_lossy(&output.stdout);
    let has: BTreeSet<&str> = said.lines().map(str::trim).collect();
    let ssh = |command: &str| format!("ssh {host} '{command}'");
    let as_user = |command: &str| format!("ssh {host} \"runuser -u obc-data -- sh -c '{command}'\"");
    let mut checks = vec![credential, Check::passed(login)];
    let mut need = |name: &str, line: &str, fix: &str, commands: Vec<String>| {
        checks.push(match has.contains(line) {
            true => Check::passed(name),
            false => Check::failed(name, "the VPS lacks it", fix, commands),
        });
    };
    let here = "Run on this machine, in the root of this checkout:";
    let useradd = "useradd --system --create-home --home-dir /var/lib/obc-data --shell /usr/sbin/nologin obc-data";
    need("user obc-data", "user", here, vec![ssh(useradd)]);
    need("/opt/obc-data", "opt", here, vec![ssh("install -d -o obc-data -g obc-data /opt/obc-data")]);
    let rustup = "curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path";
    need("Rust of obc-data", "rust", here, vec![as_user(rustup)]);
    need("uv of obc-data", "uv", here, vec![as_user("curl -LsSf https://astral.sh/uv/install.sh | sh")]);
    need("C compiler", "cc", here, vec![ssh("apt-get install -y build-essential")]);
    let install = |what: &str| format!("Install {what} on the VPS, so that `python3` and `node` give it.");
    need(&format!("Python {python}"), &format!("python={python}"), &install(&format!("Python {python}")), Vec::new());
    need(&format!("Node {node}"), &format!("node={node}"), &install(&format!("Node {node}")), Vec::new());
    let unit = vec![
        format!("scp tools/vps/obc-data-serve.service {host}:/etc/systemd/system/"),
        ssh("systemctl daemon-reload && systemctl enable obc-data-serve"),
    ];
    need("unit obc-data-serve", "unit", here, unit);
    let caddy = vec![
        format!("scp tools/vps/obc-data-serve.caddy {host}:/etc/caddy/planner/"),
        ssh("caddy validate --config /etc/caddy/Caddyfile && systemctl reload caddy"),
    ];
    need("Caddy route of /planner-api/", "caddy", &format!("After the first deploy. {here}"), caddy);
    // The follower runs without the route; only riders cannot reach it.
    if let Some(caddy) = checks.last_mut() {
        caddy.blocks = false;
    }
    checks
}

/// The check of a login that `ssh` refused with `stderr`, and how to give this machine access.
fn refused(name: &str, host: &str, stderr: &str, home: Option<&Path>) -> Check {
    let said = stderr.lines().map(str::trim).rfind(|line| !line.is_empty()).unwrap_or("no reason").to_string();
    if stderr.contains("IDENTIFICATION HAS CHANGED") {
        return Check::failed(
            name,
            "the host key of the VPS is not the one that this machine knows",
            "The VPS was set up again, or another machine answers. Compare the fingerprint in the console of \
             the hosting provider. Only when it is the VPS, remove the old key, then check again.",
            vec![match endpoint(host) {
                Ok(at) => format!("ssh-keygen -R '{}' -f '{}'", at.name, at.file.display()),
                Err(_) => format!("ssh-keygen -R '{host}'"),
            }],
        );
    }
    if stderr.contains("Host key verification failed") {
        return match host_key(host) {
            Ok(HostKey { fingerprint, .. }) => Check {
                action: Some(["deploy", "trust", "--expect", &fingerprint].map(String::from).to_vec()),
                ..Check::failed(
                    name,
                    format!("this machine does not know the host key of the VPS, {fingerprint}"),
                    "Compare it with the ED25519 fingerprint of the VPS in the console of the hosting provider, \
                     or of `ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub` there. Trust it only when they \
                     are the same.",
                    Vec::new(),
                )
            },
            Err(e) => Check::failed(name, e, "Check the host and the network.", Vec::new()),
        };
    }
    if !said.contains("Permission denied") {
        return Check::failed(
            name,
            said,
            format!("Check the host and the network. `ssh {host}` shows more."),
            Vec::new(),
        );
    }
    let cases = "The VPS refuses each key that this machine offers. The host must log in as root, such as \
                 root@HOST. A key with a passphrase needs an agent that holds it: `ssh-add`.";
    match public_key(home) {
        Some((file, key)) => Check::failed(
            name,
            said,
            format!(
                "{cases} Or the VPS does not know the key of this machine, {}: add it to the keys of root, \
                 as root on the VPS, such as in the console of the hosting provider.",
                file.display()
            ),
            vec![format!("mkdir -p /root/.ssh && echo '{key}' >> /root/.ssh/authorized_keys")],
        ),
        None => Check::failed(
            name,
            said,
            format!("{cases} This machine has no SSH key: make one, then check again for the line to add."),
            vec!["ssh-keygen -t ed25519 -N '' -f ~/.ssh/id_ed25519".into()],
        ),
    }
}

/// The ED25519 host key of the VPS, as `ssh` looks it up.
pub struct HostKey {
    /// Its line of `known_hosts`, under [`Endpoint::name`].
    pub line: String,
    pub fingerprint: String,
    pub at: Endpoint,
}

/// Where `ssh` reaches an SSH host, and where it looks its host key up.
pub struct Endpoint {
    host: String,
    port: String,
    /// The name of the host key: `HostKeyAlias`, else the host, with a port other than 22.
    pub name: String,
    /// The first `UserKnownHostsFile`, which `ssh` reads.
    pub file: PathBuf,
}

/// The ED25519 host key of the SSH host `host`, and its fingerprint.
pub fn host_key(host: &str) -> Result<HostKey, String> {
    let at = endpoint(host)?;
    let scan = ["-T", "5", "-t", "ed25519", "-p", &at.port, "--", &at.host];
    let scan = limited(Command::new("ssh-keyscan").args(scan))?;
    let text = String::from_utf8_lossy(&scan.stdout);
    let line = text.lines().find(|line| !line.starts_with('#') && !line.trim().is_empty());
    let line = line.ok_or_else(|| format!("{} gives no ED25519 host key", at.host))?;
    let key: Vec<&str> = line.split_whitespace().skip(1).collect();
    let line = format!("{} {}", at.name, key.join(" "));
    let name = &at.host;
    let mut keygen = Command::new("ssh-keygen");
    keygen.args(["-l", "-f", "-"]).stdin(Stdio::piped()).stdout(Stdio::piped());
    let mut keygen = keygen.spawn().map_err(|e| format!("ssh-keygen: {e}"))?;
    let written = std::io::Write::write_all(&mut keygen.stdin.take().expect("piped"), format!("{line}\n").as_bytes());
    written.map_err(|e| format!("ssh-keygen: {e}"))?;
    let listed = keygen.wait_with_output().map_err(|e| format!("ssh-keygen: {e}"))?;
    let listed = String::from_utf8_lossy(&listed.stdout);
    let fingerprint = listed.split_whitespace().nth(1).filter(|field| field.starts_with("SHA256:"));
    let fingerprint = fingerprint.ok_or_else(|| format!("ssh-keygen cannot read the host key of {name}"))?;
    let fingerprint = fingerprint.to_string();
    Ok(HostKey { line, fingerprint, at })
}

/// Whether the known hosts of `at` hold a key of its name: a key that changed goes only with
/// `ssh-keygen -R`, never beside the old one.
pub fn known(at: &Endpoint) -> bool {
    let mut find = Command::new("ssh-keygen");
    find.arg("-F").arg(&at.name).arg("-f").arg(&at.file);
    limited(&mut find).is_ok_and(|output| output.status.success())
}

/// Where `ssh` reaches `host`, an alias of `~/.ssh/config` too, from `ssh -G`, which reads only the
/// configuration.
fn endpoint(host: &str) -> Result<Endpoint, String> {
    let config = limited(Command::new("ssh").args(["-G", "--", host]))?;
    endpoint_in(host, &String::from_utf8_lossy(&config.stdout))
}

fn endpoint_in(host: &str, config: &str) -> Result<Endpoint, String> {
    let value = |key: &str| config.lines().find_map(|line| line.strip_prefix(key)).map(str::to_string);
    let name = value("hostname ").ok_or_else(|| format!("ssh names no host for {host}"))?;
    let port = value("port ").unwrap_or_else(|| "22".into());
    let alias = value("hostkeyalias ").filter(|alias| alias != "none");
    let key = alias.unwrap_or_else(|| if port == "22" { name.clone() } else { format!("[{name}]:{port}") });
    let files = value("userknownhostsfile ").unwrap_or_default();
    let file = files.split_whitespace().next().unwrap_or("~/.ssh/known_hosts");
    Ok(Endpoint { host: name, port, name: key, file: crate::sources::expand_home(file) })
}

/// The output of `command`, which ends within [`LIMIT`] or is killed.
fn limited(command: &mut Command) -> Result<Output, String> {
    limited_to(command, LIMIT)
}

fn limited_to(command: &mut Command, limit: Duration) -> Result<Output, String> {
    let program = command.get_program().to_string_lossy().to_string();
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    let deadline = Instant::now() + limit;
    while child.try_wait().map_err(|e| e.to_string())?.is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{program} gave no answer within {} s", limit.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child.wait_with_output().map_err(|e| format!("{program}: {e}"))
}

/// `output` when it succeeded, else its last line of standard error.
fn succeeded(what: &str, output: Output) -> Result<Output, String> {
    if output.status.success() {
        return Ok(output);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(format!("{what}: {}", stderr.lines().map(str::trim).rfind(|line| !line.is_empty()).unwrap_or("failed")))
}

/// The first public key of `~/.ssh` that `ssh` offers, and its line.
fn public_key(home: Option<&Path>) -> Option<(PathBuf, String)> {
    let dir = home?.join(".ssh");
    ["id_ed25519.pub", "id_ecdsa.pub", "id_rsa.pub"].iter().find_map(|name| {
        let file = dir.join(name);
        let key = std::fs::read_to_string(&file).ok()?.trim().to_string();
        (!key.is_empty() && !key.contains('\'')).then_some((file, key))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::Scratch;

    #[test]
    fn a_refused_login_shows_the_key_to_add_or_how_to_make_one() {
        let home = Scratch::new("deploy-ssh-key");
        let denied = "Warning: added host\nroot@vps.test: Permission denied (publickey).\n";
        let check = refused("login", "root@vps.test", denied, Some(&home.0));
        assert_eq!(check.problem.as_deref(), Some("root@vps.test: Permission denied (publickey)."));
        assert_eq!(check.commands, ["ssh-keygen -t ed25519 -N '' -f ~/.ssh/id_ed25519"]);
        assert!(check.fix.unwrap().contains("passphrase"));

        std::fs::create_dir_all(home.0.join(".ssh")).unwrap();
        std::fs::write(home.0.join(".ssh/id_ed25519.pub"), "ssh-ed25519 AAAAfake me@bake\n").unwrap();
        let check = refused("login", "root@vps.test", denied, Some(&home.0));
        let line = "mkdir -p /root/.ssh && echo 'ssh-ed25519 AAAAfake me@bake' >> /root/.ssh/authorized_keys";
        assert_eq!(check.commands, [line]);

        let check = refused("login", "root@vps.test", "ssh: Could not resolve hostname vps.test\n", Some(&home.0));
        assert!(
            check.commands.is_empty() && check.action.is_none() && check.fix.unwrap().starts_with("Check the host")
        );
    }

    #[test]
    fn a_host_key_goes_under_the_name_and_into_the_file_that_ssh_reads() {
        let home = Scratch::new("deploy-known-hosts");
        let file = home.0.join("known");
        let config =
            format!("user root\nhostname 203.0.113.7\nport 22\nuserknownhostsfile {} /etc/x\n", file.display());
        let at = endpoint_in("vps", &config).unwrap();
        assert_eq!((at.name.as_str(), at.file.as_path()), ("203.0.113.7", file.as_path()));
        let at = endpoint_in("vps", &config.replace("port 22", "port 2222\nhostkeyalias none")).unwrap();
        assert_eq!(at.name, "[203.0.113.7]:2222");
        let at = endpoint_in("vps", &format!("{config}hostkeyalias vps-key\n")).unwrap();
        assert_eq!(at.name, "vps-key");

        assert!(!known(&at));
        let key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";
        std::fs::write(&file, format!("vps-key {key}\n")).unwrap();
        assert!(known(&at), "a second key of the host only goes with ssh-keygen -R");
    }

    #[test]
    fn a_command_that_does_not_end_is_killed_at_the_limit() {
        let started = Instant::now();
        let ended = limited_to(Command::new("sh").args(["-c", "sleep 60"]), Duration::from_secs(1));
        assert_eq!(ended.unwrap_err(), "sh gave no answer within 1 s");
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
