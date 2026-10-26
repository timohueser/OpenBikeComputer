//! What publishing to prod and each deploy need, checked now from this machine, each with the fix
//! that a person applies. The checks only read: an SSH login that runs [`PROBE`], and a GET of the
//! Cloudflare API.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use super::{git, What, VPS};
use crate::fetch::http::Http;
use crate::r2::{Bucket, Credentials};

/// One thing that an action needs.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Check {
    pub name: String,
    /// Why it fails; `null` when it passes.
    pub problem: Option<String>,
    /// What to do about the problem, and where the commands run.
    pub fix: Option<String>,
    /// The commands of the fix, one per line.
    pub commands: Vec<String>,
    /// Whether the action waits for it.
    pub blocks: bool,
}

impl Check {
    fn passed(name: &str) -> Self {
        Check { name: name.into(), problem: None, fix: None, commands: Vec::new(), blocks: true }
    }

    fn failed(name: &str, problem: impl Into<String>, fix: impl Into<String>, commands: Vec<String>) -> Self {
        Check { name: name.into(), problem: Some(problem.into()), fix: Some(fix.into()), commands, blocks: true }
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
            "Put the OBC_R2_* variables of the bucket in tools/obc.local of this checkout, then start `obc data ui` \
             again.",
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
    match credential.present() {
        true => Check::passed(name),
        false => Check::failed(
            name,
            format!("{} needs {}, which this machine does not have", what.label(), credential.describe()),
            format!("Set it here, or with `obc data deploy credential {}`.", what.name()),
            Vec::new(),
        ),
    }
}

/// Whether `planner/tiles` holds only what the commit holds: `wrangler deploy` reads the files of the
/// checkout.
pub fn clean(root: &Path) -> Check {
    let name = "planner/tiles committed";
    match git(root, &["status", "--porcelain", "--", "planner/tiles"]) {
        Ok(changes) if changes.is_empty() => Check::passed(name),
        Ok(_) => Check::failed(
            name,
            "planner/tiles has changes that the commit does not hold",
            "Commit or remove them in this checkout.",
            vec!["git status -- planner/tiles".into()],
        ),
        Err(e) => Check::failed(name, e.message, e.fix, Vec::new()),
    }
}

/// What a deploy of the tiles Worker needs: the credential, a token that may edit the Workers of the
/// account, and a committed `planner/tiles`.
pub fn tiles(root: &Path) -> Vec<Check> {
    let credential = credential(What::Tiles);
    if !credential.ok() {
        return vec![credential, clean(root)];
    }
    let [token, account] =
        ["CLOUDFLARE_API_TOKEN", "CLOUDFLARE_ACCOUNT_ID"].map(|name| std::env::var(name).unwrap_or_default());
    let api = "https://api.cloudflare.com";
    let url = format!("{api}/client/v4/accounts/{account}/workers/scripts");
    let answer = Http::new().with_bearer(api, token).answer(&url, "application/json");
    let name = "Cloudflare token";
    let token = match answer {
        Ok((200, _)) => Check::passed(name),
        Ok((status, body)) => {
            let body: Value = serde_json::from_slice(&body).unwrap_or_default();
            let said = body["errors"][0]["message"].as_str().unwrap_or("no reason");
            Check::failed(
                name,
                format!("Cloudflare refuses to list the Workers of account {account}: HTTP {status}, {said}"),
                "Create a token with the template \"Edit Cloudflare Workers\": the account of the ID, and the zone \
                 openbikecomputer.com. The ID is under Account details in the Workers & Pages overview. Then \
                 replace the credential here.",
                Vec::new(),
            )
        }
        Err(e) => Check::failed(name, e, "Check the network of this machine, then check again.", Vec::new()),
    };
    vec![credential, token, clean(root)]
}

/// The script that the VPS runs for [`services`]: one line for each piece of its one-time setup that
/// it has. It changes nothing.
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

/// What a deploy of the follower needs: the SSH host, a login of this machine as root without a
/// prompt, and the one-time setup of `tools/vps/README.md` on the VPS. `python` and `node` are the
/// `major.minor` versions of `data/planner-runtime.toml`.
pub fn services(home: Option<&Path>, (python, node): (&str, &str)) -> Vec<Check> {
    let credential = credential(What::Services);
    if !credential.ok() {
        return vec![credential];
    }
    let host = std::env::var(VPS).unwrap_or_default();
    if host.starts_with('-') {
        return vec![Check::failed(
            "credential",
            format!("{VPS} starts with `-`"),
            "Set the SSH host again.",
            Vec::new(),
        )];
    }
    // `accept-new` keeps the key of a VPS that this machine never saw, so the deploy can log in.
    let options = ["-o", "BatchMode=yes", "-o", "ConnectTimeout=8", "-o", "StrictHostKeyChecking=accept-new"];
    let probe = Command::new("ssh").args(options).arg("--").arg(&host).arg(PROBE).stdin(Stdio::null()).output();
    let login = "login as root";
    let output = match probe {
        Ok(output) if output.status.success() => output,
        Ok(output) => return vec![credential, refused(login, &host, &String::from_utf8_lossy(&output.stderr), home)],
        Err(e) => {
            return vec![
                credential,
                Check::failed(login, format!("ssh: {e}"), "Install OpenSSH on this machine.", Vec::new()),
            ]
        }
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
    let on_this = "Run on this machine, in the root of this checkout:";
    need(
        "user obc-data",
        "user",
        on_this,
        vec![ssh("useradd --system --create-home --home-dir /var/lib/obc-data --shell /usr/sbin/nologin obc-data")],
    );
    need("/opt/obc-data", "opt", on_this, vec![ssh("install -d -o obc-data -g obc-data /opt/obc-data")]);
    need(
        "Rust of obc-data",
        "rust",
        on_this,
        vec![as_user("curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path")],
    );
    need("uv of obc-data", "uv", on_this, vec![as_user("curl -LsSf https://astral.sh/uv/install.sh | sh")]);
    need("C compiler", "cc", on_this, vec![ssh("apt-get install -y build-essential")]);
    let install =
        |what: &str| format!("Install {what} on the VPS, so that `python3` and `node` give it, then check again.");
    need(&format!("Python {python}"), &format!("python={python}"), &install(&format!("Python {python}")), Vec::new());
    need(&format!("Node {node}"), &format!("node={node}"), &install(&format!("Node {node}")), Vec::new());
    need(
        "unit obc-data-serve",
        "unit",
        on_this,
        vec![
            format!("scp tools/vps/obc-data-serve.service {host}:/etc/systemd/system/"),
            ssh("systemctl daemon-reload && systemctl enable obc-data-serve"),
        ],
    );
    need(
        "Caddy route of /planner-api/",
        "caddy",
        &format!("After the first deploy. {on_this}"),
        vec![
            format!("scp tools/vps/obc-data-serve.caddy {host}:/etc/caddy/planner/"),
            ssh("caddy validate --config /etc/caddy/Caddyfile && systemctl reload caddy"),
        ],
    );
    // The follower runs without the route; only riders cannot reach it.
    if let Some(caddy) = checks.last_mut() {
        caddy.blocks = false;
    }
    checks
}

/// The check of a login that `ssh` refused with `stderr`, and how to give this machine access.
fn refused(name: &str, host: &str, stderr: &str, home: Option<&Path>) -> Check {
    let said = stderr.lines().map(str::trim).rfind(|line| !line.is_empty()).unwrap_or("no reason").to_string();
    if !said.contains("Permission denied") {
        return Check::failed(
            name,
            said,
            format!("Check the host and the network. `ssh {host}` in a terminal shows more."),
            Vec::new(),
        );
    }
    match public_key(home) {
        Some((file, key)) => Check::failed(
            name,
            said,
            format!(
                "The VPS does not know the key of this machine, {}. Add it to the keys of root: run this on the VPS \
                 as root, for example in the console of the hosting provider. Then check again.",
                file.display()
            ),
            vec![format!("mkdir -p /root/.ssh && echo '{key}' >> /root/.ssh/authorized_keys")],
        ),
        None => Check::failed(
            name,
            said,
            "This machine has no SSH key. Make one on this machine, then check again: the page then shows the line \
             to add on the VPS.",
            vec!["ssh-keygen -t ed25519 -N '' -f ~/.ssh/id_ed25519".into()],
        ),
    }
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

        std::fs::create_dir_all(home.0.join(".ssh")).unwrap();
        std::fs::write(home.0.join(".ssh/id_ed25519.pub"), "ssh-ed25519 AAAAfake me@bake\n").unwrap();
        let check = refused("login", "root@vps.test", denied, Some(&home.0));
        assert_eq!(
            check.commands,
            ["mkdir -p /root/.ssh && echo 'ssh-ed25519 AAAAfake me@bake' >> /root/.ssh/authorized_keys"]
        );

        let check = refused("login", "root@vps.test", "ssh: Could not resolve hostname vps.test\n", Some(&home.0));
        assert!(check.commands.is_empty() && check.fix.unwrap().starts_with("Check the host"));
    }
}
