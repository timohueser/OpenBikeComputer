//! `obc data deploy`: the code that serves `prod` beside its data. The follower, `obc data serve
//! prod` on the VPS, runs the planner services, and the tiles Worker on Cloudflare serves the map
//! tiles; `obc data update prod` publishes only data. Without an action, `deploy` asks each which
//! commit it runs. `deploy services` and `deploy tiles` deploy the commit of this checkout in a
//! run.

mod checks;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use clap::{Args, Subcommand, ValueEnum};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use super::api::{confirm, Detached};
use super::setup_cli::{CredentialState, Readiness};
use super::{print_json, print_table, Code, Error};
use crate::engine::{runs, Step};
use crate::fetch::http::Http;
use crate::serve::http::Version;
use crate::sources::Credential;
use crate::store::Store;
use checks::Check;

#[derive(Subcommand)]
pub enum DeployCommand {
    /// Build the commit of this checkout on the VPS and restart the follower, with
    /// `tools/vps/deploy-services.sh`.
    Services(DeployArgs),
    /// Deploy the tiles Worker of this checkout to Cloudflare, with `wrangler deploy`.
    Tiles(DeployArgs),
    /// Set what a deploy needs from standard input, one value per line in the order of `fields` in
    /// `obc data setup`. Never from an argument, which every process can read.
    Credential { what: What },
    /// Add the host key of the VPS to `~/.ssh/known_hosts`, when its fingerprint is `--expect`.
    /// Asks first.
    Trust {
        /// The fingerprint that a person compared, such as `SHA256:…`.
        #[arg(long)]
        expect: String,
        /// Do not ask. Required without a terminal.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Args)]
pub struct DeployArgs {
    /// Write what the deploy does, and change nothing.
    #[arg(long, conflicts_with = "expect")]
    dry_run: bool,
    /// The commit that the dry run names: the deploy starts only while this checkout is at it.
    #[arg(long, value_name = "COMMIT")]
    expect: Option<String>,
    /// Do not ask. Required without a terminal.
    #[arg(long, requires = "expect")]
    yes: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum What {
    Services,
    Tiles,
}

/// The variable that holds the SSH host of the VPS.
const VPS: &str = "OBC_VPS_SSH";

impl What {
    fn name(self) -> &'static str {
        match self {
            What::Services => "services",
            What::Tiles => "tiles",
        }
    }

    fn label(self) -> &'static str {
        match self {
            What::Services => "the follower on the VPS",
            What::Tiles => "the tiles Worker",
        }
    }

    /// The variables of `credentials.env` that a deploy needs.
    fn credential(self) -> Credential {
        match self {
            What::Services => Credential {
                env: vec![VPS.into()],
                hint: Some("The SSH host or alias of the VPS. It logs in as root with the SSH key of this machine, without a prompt.".into()),
                ..Default::default()
            },
            // What `wrangler deploy` needs without a terminal.
            What::Tiles => Credential {
                env: vec!["CLOUDFLARE_API_TOKEN".into(), "CLOUDFLARE_ACCOUNT_ID".into()],
                register: Some("https://dash.cloudflare.com/profile/api-tokens".into()),
                hint: Some("Create a token with the template \"Edit Cloudflare Workers\". Copy it, and the ID of the account.".into()),
                file: None,
            },
        }
    }
}

/// What `deploy` writes.
#[derive(Serialize, JsonSchema)]
pub struct Deployed {
    /// The commit of this checkout: what a deploy puts in place.
    pub checkout: String,
    /// What `update prod` and the delete of its old data need on this machine.
    pub publish: Vec<Check>,
    pub services: Running,
    pub tiles: Running,
}

/// What a deployment says that it runs.
#[derive(Serialize, JsonSchema)]
pub struct Running {
    /// The URL that answers with its commit.
    pub url: String,
    /// The commit that it runs; `null` when it does not answer, or when its build named none.
    pub commit: Option<String>,
    /// Why it cannot serve a release of `obc data`; `null` when it answers.
    pub reason: Option<String>,
    /// What a deploy from this machine needs, each checked now.
    pub checks: Vec<Check>,
    /// The credential that a deploy needs, which the page sets.
    pub credential: CredentialState,
}

/// What `deploy trust` wrote.
#[derive(Serialize, JsonSchema)]
pub struct Trusted {
    pub fingerprint: String,
    /// The file that holds the host key now.
    pub file: PathBuf,
}

/// What `deploy WHAT --dry-run` writes.
#[derive(Serialize, JsonSchema)]
pub struct DeployPlan {
    pub what: String,
    /// The commit of this checkout that it deploys.
    pub commit: String,
    /// What the deploy changes.
    pub text: String,
    /// The commands that it runs in the repository root, in this order.
    pub commands: Vec<String>,
    /// The arguments after `obc data` that start it.
    pub args: Vec<String>,
}

pub fn command(root: &Path, command: Option<DeployCommand>, json: bool) -> Result<(), Error> {
    let (what, args) = match command {
        None => return status(root, json),
        Some(DeployCommand::Services(args)) => (What::Services, args),
        Some(DeployCommand::Tiles(args)) => (What::Tiles, args),
        Some(DeployCommand::Credential { what }) => {
            let place = crate::credentials::place(&what.credential()).expect("an env credential has a place");
            let set = super::setup_cli::write(place, true, Vec::new())?;
            return super::setup_cli::print_set(&set, json);
        }
        Some(DeployCommand::Trust { expect, yes }) => return trust(&expect, yes, json),
    };
    let store = Store::open()?;
    let expect = args.expect.clone().unwrap_or_default();
    let work = |run: &mut runs::Run| deploy(root, &store, what, &expect, run);
    if let Some(done) = super::api::detached(&store, work, |()| Ok(())) {
        return done;
    }
    let plan = plan(root, what)?;
    if args.dry_run {
        return print_plan(&plan, json);
    }
    let Some(expect) = &args.expect else {
        return Err(Code::Usage
            .error("a deploy starts only with `--expect COMMIT`, the commit of its dry run; nothing changed")
            .fix(format!("Run `obc data deploy {} --dry-run`, then the command that it shows.", what.name())));
    };
    expected(what, &plan.commit, expect)?;
    confirm(&format!("Deploy commit {} to {}?", short(&plan.commit), what.label()), args.yes)?;
    let run = runs::detach(&store, &[&plan.args[..], &["--yes".into()]].concat())?;
    if json {
        return print_json(&Detached { run });
    }
    eprintln!("obc data: run {run}");
    super::runs_cli::follow(&store, &run, false)
}

/// What a deploy of `what` does now, or why this machine cannot do it.
fn plan(root: &Path, what: What) -> Result<DeployPlan, Error> {
    let commit = git(root, &["rev-parse", "--verify", "HEAD^{commit}"])?;
    let checks = [checks::credential(what), checks::clean(root, what), checks::merged(root, &commit)];
    if let Some(check) = checks::blocking(&checks) {
        return Err(Code::Blocked
            .error(check.problem.clone().unwrap_or_default())
            .fix(check.fix.clone().unwrap_or_default()));
    }
    let (text, commands) = match what {
        What::Services => (
            format!(
                "Builds commit {} on the VPS in /opt/obc-data/{commit}, points /opt/obc-data/current at it, and \
                 restarts the follower. /opt/obc-data/previous keeps the build before.",
                short(&commit)
            ),
            vec![format!("tools/vps/deploy-services.sh \"${VPS}\" {commit}")],
        ),
        What::Tiles => (
            format!(
                "Deploys the Worker of planner/tiles at commit {} to Cloudflare, as planner/tiles/wrangler.toml \
                     says. It serves tiles at once.",
                short(&commit)
            ),
            vec![
                "npm ci --prefix planner/tiles".into(),
                format!("npm run deploy --prefix planner/tiles -- --var COMMIT:{commit}"),
            ],
        ),
    };
    let args = ["deploy", what.name(), "--expect", &commit].map(String::from).to_vec();
    Ok(DeployPlan { what: what.name().into(), commit, text, commands, args })
}

/// In the run: deploy `what` while this checkout is at `expect`. The output of each command goes to
/// the log of the run.
fn deploy(root: &Path, store: &Store, what: What, expect: &str, run: &mut runs::Run) -> Result<(), Error> {
    let plan = plan(root, what)?;
    expected(what, &plan.commit, expect)?;
    let _deploying = store.try_lock(&format!("deploy-{}", what.name()))?.ok_or_else(|| {
        Code::Blocked
            .error(format!("another run deploys {}", what.label()))
            .fix("Wait for it to end: `obc data runs` shows it.")
    })?;
    for command in &plan.commands {
        if run.stopping() {
            return Err(super::api::stopped());
        }
        eprintln!("$ {command}");
        // Only what the commands need: no R2 key and no credential of a source.
        let base = ["PATH", "HOME", "USER", "LOGNAME", "LANG", "LC_ALL", "TMPDIR", "SSH_AUTH_SOCK"];
        let names = base.iter().map(|name| name.to_string()).chain(what.credential().env);
        let env = names.filter_map(|name| std::env::var_os(&name).map(|value| (name, value)));
        let mut process = Command::new("sh");
        process.args(["-c", command]).current_dir(root).stdin(Stdio::null()).env_clear().envs(env);
        let status = process.status();
        let status = status.map_err(|e| format!("{command}: {e}"))?;
        if !status.success() {
            return Err(Code::Failed
                .error(format!("`{command}` failed ({status}); {} runs the code it ran before", what.label()))
                .fix("Read the log of the run, correct what it says, then deploy again."));
        }
    }
    Ok(())
}

/// Refuse a deploy whose dry run named another commit than `commit`, the one of this checkout.
fn expected(what: What, commit: &str, expect: &str) -> Result<(), Error> {
    if commit == expect {
        return Ok(());
    }
    Err(Code::Usage
        .error(format!("this checkout is at commit {}, not {expect}; nothing changed", short(commit)))
        .fix(format!("Run `obc data deploy {} --dry-run` again, then the command that it shows.", what.name())))
}

fn print_plan(plan: &DeployPlan, json: bool) -> Result<(), Error> {
    if json {
        return print_json(plan);
    }
    println!("{}", plan.text);
    plan.commands.iter().for_each(|command| println!("  {command}"));
    println!("Start it: obc data {}", plan.args.join(" "));
    Ok(())
}

/// Ask the follower and the tiles Worker which commit they run, and check what a deploy of each
/// needs. Each remote command ends within 15 s, and the checks run at once: the check of the
/// follower runs up to three of them in a row (the login, `ssh -G` and `ssh-keyscan`).
fn status(root: &Path, json: bool) -> Result<(), Error> {
    let checkout = git(root, &["rev-parse", "--verify", "HEAD^{commit}"])?;
    let runtime = crate::runtime::load(root).map_err(|e| Code::InvalidData.error(e))?;
    let [follower, worker] = urls_of(&runtime.publication);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let versions = (runtime.target.python.as_str(), runtime.target.node.as_str());
    let ([follows, works], services, (token, merged, publish)) = std::thread::scope(|scope| {
        let ask = |url: &str| {
            let url = url.to_string();
            scope.spawn(move || ask(&Http::new(), &url))
        };
        let answers = [ask(&follower), ask(&worker)];
        let services = scope.spawn(|| checks::services(home.as_deref(), versions));
        let token = scope.spawn(|| checks::credential(What::Tiles).ok().then(checks::token));
        let merged = scope.spawn(|| checks::merged(root, &checkout));
        let publish = checks::publish();
        let join = |answer: std::thread::ScopedJoinHandle<'_, Result<Version, String>>| {
            answer.join().unwrap_or_else(|_| Err("the check panicked".into()))
        };
        let [follows, works] = answers.map(join);
        let joined = (token.join().ok().flatten(), merged.join().expect("a check does not panic"), publish);
        ([follows, works], services.join().expect("a check does not panic"), joined)
    });
    let mut services = services;
    services.extend([checks::clean(root, What::Services), merged.clone()]);
    let mut tiles = vec![checks::credential(What::Tiles)];
    tiles.extend(token);
    tiles.extend([checks::clean(root, What::Tiles), merged]);
    let running = |what: What, url: String, answer: Result<Version, String>, checks: Vec<Check>| Running {
        commit: answer.as_ref().ok().and_then(|version| version.commit.clone()),
        reason: answer.err(),
        checks,
        credential: credential(what),
        url,
    };
    let deployed = Deployed {
        checkout,
        publish,
        services: running(What::Services, follower, follows, services),
        tiles: running(What::Tiles, worker, works, tiles),
    };
    if json {
        return print_json(&deployed);
    }
    let row = |name: &str, running: &Running| {
        let state = match (&running.reason, &running.commit) {
            (Some(reason), _) => format!("✗ {reason}"),
            (None, Some(commit)) => format!("runs {}", short(commit)),
            (None, None) => "answers; its build named no commit".into(),
        };
        vec![format!("  {name}"), state]
    };
    let failed = |checks: &[Check]| -> Vec<Vec<String>> {
        let mut rows = Vec::new();
        for check in checks.iter().filter(|check| !check.ok()) {
            rows.push(vec![format!("    ✗ {}", check.name), check.problem.clone().unwrap_or_default()]);
            rows.push(vec![String::new(), check.fix.clone().unwrap_or_default()]);
            rows.extend(check.commands.iter().map(|command| vec![String::new(), format!("  {command}")]));
            if let Some(action) = &check.action {
                rows.push(vec![String::new(), format!("  obc data {}", action.join(" "))]);
            }
        }
        rows
    };
    let mut rows = vec![vec![format!("this checkout {}", short(&deployed.checkout))]];
    rows.push(vec![
        "  publish".into(),
        if failed(&deployed.publish).is_empty() { "ready" } else { "not ready" }.into(),
    ]);
    rows.extend(failed(&deployed.publish));
    for (name, running) in [("services", &deployed.services), ("tiles", &deployed.tiles)] {
        rows.push(row(name, running));
        rows.extend(failed(&running.checks));
    }
    print_table(&rows);
    Ok(())
}

/// Add the host key of the VPS to `~/.ssh/known_hosts` when its fingerprint is `expect`, the one
/// that a person compared.
fn trust(expect: &str, yes: bool, json: bool) -> Result<(), Error> {
    let credential = checks::credential(What::Services);
    if let Some(problem) = credential.problem {
        return Err(Code::Blocked.error(problem).fix(credential.fix.unwrap_or_default()));
    }
    let host = std::env::var(VPS).unwrap_or_default();
    let checks::HostKey { line, fingerprint, at } = checks::host_key(&host).map_err(|e| Code::Failed.error(e))?;
    if fingerprint != expect {
        return Err(Code::Usage
            .error(format!("the host key of {host} is {fingerprint}, not {expect}; nothing changed"))
            .fix("Compare the fingerprint again, then trust the one that the page shows now."));
    }
    if checks::known(&at) {
        return Err(Code::Usage
            .error(format!("{} holds a host key of {} already; nothing changed", at.file.display(), at.name))
            .fix(format!(
                "When the VPS has another key now, compare it, then remove the old one: `ssh-keygen -R '{}' -f '{}'`.",
                at.name,
                at.file.display()
            )));
    }
    confirm(&format!("Trust the host key {fingerprint} of {host}?"), yes)?;
    append(&at.file, &line).map_err(|e| Code::Failed.error(format!("{}: {e}", at.file.display())))?;
    let trusted = Trusted { fingerprint, file: at.file };
    if json {
        return print_json(&trusted);
    }
    println!("{} holds the host key {} of {host}.", trusted.file.display(), trusted.fingerprint);
    Ok(())
}

/// Append `line` to `file`, which is private to its user, as `ssh` wants it.
fn append(file: &Path, line: &str) -> std::io::Result<()> {
    let mut dir = std::fs::DirBuilder::new();
    dir.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut dir, 0o700);
    dir.create(file.parent().expect("a file in ~/.ssh"))?;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    std::io::Write::write_all(&mut options.open(file)?, format!("{line}\n").as_bytes())
}

/// Why the follower or the tiles Worker at `urls` cannot serve a release of `steps`, each with its
/// fix. An update does not refuse: its run would wait at router-ready.
pub(super) fn problems(urls: &[String; 2], http: &Http, steps: &[Step]) -> Vec<String> {
    let fix = |what: What| {
        format!("Deploy {} first: Services on the screen of prod, or `obc data deploy {}`.", what.label(), what.name())
    };
    let mut problems = Vec::new();
    match ask(http, &urls[0]) {
        Err(e) => problems.push(format!(
            "No follower answers ({e}): the update would wait 24 hours at router-ready, and then leave the target \
             as it was. {}",
            fix(What::Services)
        )),
        Ok(version) => {
            if version.manifest != crate::release::FORMAT {
                problems.push(format!(
                    "The follower reads release manifests of format {}, and this checkout writes format {}: it \
                     cannot start the planner release. {}",
                    version.manifest,
                    crate::release::FORMAT,
                    fix(What::Services)
                ));
            }
            for (layer, reads) in &version.formats {
                let step = steps.iter().find(|step| step.layer.name == layer && step.layer.format != *reads);
                problems.extend(step.map(|step| {
                    format!(
                        "The follower reads {layer} format {reads}, and the release has format {}: it cannot start \
                         the planner release. {}",
                        step.layer.format,
                        fix(What::Services)
                    )
                }));
            }
        }
    }
    match ask(http, &urls[1]) {
        Err(e) => problems.push(format!(
            "No tiles Worker of the layout of `obc data` answers ({e}): riders would get no tiles of the \
             release. {}",
            fix(What::Tiles)
        )),
        Ok(worker) if !worker.grids.contains(&GRID) => problems.push(format!(
            "The tiles Worker does not read grid format {GRID}, which the release has: riders would get no \
             tiles of it. {}",
            fix(What::Tiles)
        )),
        Ok(_) => {}
    }
    problems
}

/// The format of `public/grid.json` of a release: `GRID_FORMAT` of `tools/planner_runtime.py`.
const GRID: u32 = 3;

/// The URLs at which the follower and the tiles Worker say which commit they run.
fn urls_of(origins: &crate::runtime::Origins) -> [String; 2] {
    [
        format!("{}/planner-api/version", origins.api_origin.trim_end_matches('/')),
        format!("{}/version", origins.tiles_origin.trim_end_matches('/')),
    ]
}

/// The review of an update of prod: [`problems`] with the origins of `data/planner-runtime.toml`
/// of `root`. A file that cannot be read is a warning too.
pub(super) fn review(root: &Path, http: &Http, steps: &[Step]) -> Vec<String> {
    match crate::runtime::load(root) {
        Ok(runtime) => problems(&urls_of(&runtime.publication), http, steps),
        Err(e) => vec![format!("{e}: the follower and the tiles Worker cannot be checked")],
    }
}

/// What the code at `url` says that it runs. Only the answer of the code of this repository serves:
/// 200 with `commit`.
fn ask(http: &Http, url: &str) -> Result<Version, String> {
    let (status, body) = http.answer(url, "application/json")?;
    let answer: Value = serde_json::from_slice(&body).unwrap_or_default();
    match answer.get("commit") {
        Some(_) if status == 200 => serde_json::from_value(answer).map_err(|e| format!("GET {url}: {e}")),
        _ => Err(format!("{url} does not answer (HTTP {status}): built before /version, or not running")),
    }
}

/// The credentials of the deploys, for `obc data setup`.
pub(super) fn credentials() -> Vec<CredentialState> {
    [What::Services, What::Tiles].map(credential).into()
}

/// The credential of `what`, and the command that sets it.
fn credential(what: What) -> CredentialState {
    let credential = what.credential();
    CredentialState {
        name: credential.describe(),
        needed_for: format!("deploy {} of prod", what.label()),
        state: if credential.present() { Readiness::Ready } else { Readiness::Optional },
        set: Some(["deploy", "credential", what.name()].map(String::from).to_vec()),
        fields: credential.env.clone(),
        uses: Vec::new(),
        register: credential.register,
        hint: credential.hint,
    }
}

/// The output of `git ARGS` in `root`, trimmed.
fn git(root: &Path, args: &[&str]) -> Result<String, Error> {
    let output = Command::new("git").args(args).current_dir(root).stderr(Stdio::inherit()).output();
    match output {
        Ok(output) if output.status.success() => Ok(String::from_utf8_lossy(&output.stdout).trim().to_string()),
        _ => Err(Code::Failed
            .error(format!("`git {}` failed in {}", args.join(" "), root.display()))
            .fix("A deploy deploys a commit: run it in a git checkout.")),
    }
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(12)]
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, RwLock};

    use super::*;
    use crate::engine::tests::{step, TEST};
    use crate::engine::Run;

    /// A follower that reads `formats` and manifests of `manifest`, and a tiles Worker that reads
    /// `grids`, on a port of this machine.
    fn services(runtime: &tokio::runtime::Runtime, formats: &[(&str, u32)], manifest: u32, grids: &[u32]) -> String {
        let grids = grids.to_vec();
        let formats = formats.iter().map(|(layer, format)| (layer.to_string(), *format)).collect();
        let version = Version { commit: Some("c".repeat(40)), formats, manifest, grids: Vec::new() };
        let worker =
            axum::routing::get(move || async move { axum::Json(serde_json::json!({"commit": null, "grids": grids})) });
        let router = crate::serve::http::router(Arc::new(RwLock::new(Default::default())), None, version);
        let listener = runtime.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
        let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        runtime.spawn(async move { axum::serve(listener, router.route("/version", worker)).await.unwrap() });
        origin
    }

    #[test]
    fn the_review_names_a_follower_or_a_worker_that_cannot_serve_the_release() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let steps = [step("test/a", Vec::new(), "a", Run::Command(Vec::new()))];
        let http = Http::new();
        let urls = |origin: &str| [format!("{origin}/planner-api/version"), format!("{origin}/version")];
        let origin = services(&runtime, &[(TEST.name, TEST.format)], crate::release::FORMAT, &[2, GRID]);
        assert_eq!(problems(&urls(&origin), &http, &steps), Vec::<String>::new());

        // A Worker built before block indexes.
        let origin = services(&runtime, &[(TEST.name, TEST.format)], crate::release::FORMAT, &[]);
        let found = problems(&urls(&origin), &http, &steps);
        assert!(
            matches!(&found[..], [one] if one.starts_with("The tiles Worker does not read grid format 3")),
            "{found:?}"
        );

        // A follower built before it said which manifests it reads.
        let origin = services(&runtime, &[(TEST.name, TEST.format)], 0, &[GRID]);
        let found = problems(&urls(&origin), &http, &steps);
        assert!(
            matches!(&found[..], [one] if one.starts_with("The follower reads release manifests of format 0")),
            "{found:?}"
        );

        let origin = services(&runtime, &[(TEST.name, TEST.format + 1)], crate::release::FORMAT, &[GRID]);
        let found = problems(&urls(&origin), &http, &steps);
        assert!(
            matches!(&found[..], [one] if one.starts_with("The follower reads test format 2, and the release has format 1")),
            "{found:?}"
        );

        // The site answers the paths that no follower or Worker answers.
        let found = problems(&[format!("{origin}/planner-api/x"), format!("{origin}/x")], &http, &steps);
        assert!(found[0].starts_with("No follower answers") && found[1].starts_with("No tiles Worker"), "{found:?}");
    }
}
