//! What every command shares: the error with its code and exit status, the question before a
//! change, and the JSON output whose schemas `specs/obc-data.md` holds.

use std::io::{BufRead, IsTerminal};
use std::process::ExitCode;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::engine::runs;

/// Why a command failed, and what to do about it.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct Error {
    pub code: Code,
    pub message: String,
    pub fix: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
}

/// What `--json` writes when a command fails.
#[derive(Serialize, JsonSchema)]
pub struct Failure<'a> {
    pub error: &'a Error,
}

/// The kind of an error. It sets the exit status and the fix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    /// An argument is not valid, an id names nothing, the command runs outside the repository, or
    /// another command must run first.
    Usage,
    /// A command that asks has no terminal to ask in, and no `--yes`.
    NoTerminal,
    /// The person did not answer yes.
    NotConfirmed,
    /// A file under `data/` is not valid.
    InvalidData,
    /// A fetch or an upstream check failed.
    FetchFailed,
    /// A credential is missing: a fetch failed without the credential of its source, or the R2
    /// variables are not set. Or a layer cannot be planned, the target cannot take the command, or
    /// this machine cannot do the work.
    Blocked,
    /// R2 or rclone failed, or refused a key.
    R2Failed,
    /// A release failed its check before a target serves it, or after an upload the object in the
    /// bucket is not the file.
    VerifyFailed,
    /// A run failed: the build, or the run that `runs RUN --follow` shows.
    RunFailed,
    /// The store or the file system failed.
    Failed,
}

impl Code {
    pub fn exit(self) -> u8 {
        match self {
            Code::Usage | Code::NoTerminal => 2,
            Code::Blocked => 4,
            Code::VerifyFailed => 5,
            Code::NotConfirmed
            | Code::InvalidData
            | Code::FetchFailed
            | Code::R2Failed
            | Code::RunFailed
            | Code::Failed => 1,
        }
    }

    /// The fix of an error that gives no other.
    pub(super) fn fix(self) -> &'static str {
        match self {
            Code::Usage => "Correct the command. `obc data --help` lists the commands and their arguments.",
            Code::NoTerminal => "Show the question to a person. When they agree, run the command again with `--yes`.",
            Code::NotConfirmed => "Nothing changed. Run the command again when you want the change.",
            Code::InvalidData => "Correct the file that the message names. `specs/obc-data.md` gives its format.",
            Code::FetchFailed => "Run the command again. A download continues where it stopped.",
            Code::Blocked => {
                "Set the credential that the message or `obc data sources` names, or correct what the message \
                 says a product needs, then run again."
            }
            Code::R2Failed => "Check the key, the `OBC_R2_*` variables and that rclone is on PATH, then run again.",
            Code::VerifyFailed => "Upload the file again.",
            Code::RunFailed => "`obc data runs RUN` shows the step that failed and its error.",
            Code::Failed => "Correct the file or the directory that the message names, then run again.",
        }
    }

    pub fn error(self, message: impl Into<String>) -> Error {
        Error { code: self, message: message.into(), fix: self.fix().into(), run: None }
    }
}

/// Finish the run once, with the error of the command, and keep that error.
pub(super) fn finish_run<T>(run: crate::engine::runs::Run, mut result: Result<T, Error>) -> Result<T, Error> {
    let (id, detached) = (run.id().to_string(), run.detached());
    let error = result.as_ref().err().map(|error| {
        let code = serde_json::to_value(error.code).expect("a code serializes");
        crate::engine::runs::RunError::new(code.as_str().unwrap_or_default(), &error.message, &error.fix)
    });
    if let Err(message) = run.finish(error) {
        // The status cannot say it. A detached run writes no error report, so its log gets it.
        if detached {
            eprintln!("obc data: the run could not finish: {message}");
        }
        match &mut result {
            Err(error) => error.message += &format!("; the run could not finish: {message}"),
            Ok(_) => result = Err(Code::Failed.error(message)),
        }
    }
    result.map_err(|mut error| {
        error.run = Some(id);
        error
    })
}

impl From<String> for Error {
    fn from(message: String) -> Self {
        Code::Failed.error(message)
    }
}

impl From<crate::store::Refused> for Error {
    fn from(refused: crate::store::Refused) -> Self {
        Code::Failed.error(refused.message).fix(refused.fix)
    }
}

impl Error {
    /// Replace the fix of the code with one that fits this error better.
    pub fn fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = fix.into();
        self
    }

    /// Write the error: as one line of JSON on standard output, which also ends a stream of JSON
    /// lines, or as text on standard error.
    pub fn report(&self, json: bool) -> ExitCode {
        if json {
            println!("{}", serde_json::to_string(&Failure { error: self }).expect("an error serializes"));
        } else {
            eprintln!("obc data: {}\n{}", self.message, self.fix);
            if let Some(run) = &self.run {
                eprintln!("run {run}; `obc data runs {run}` shows it");
            }
        }
        ExitCode::from(self.code.exit())
    }
}

/// What a command that starts a detached run writes: the run.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Detached {
    pub run: String,
}

/// In the process that [`runs::detach`] started: do `work` in its run, write what it did with
/// `print`, or exit with the status of its error, which the status and the log of the run have.
/// `None` in another process.
pub(super) fn detached<T>(
    store: &crate::store::Store,
    work: impl FnOnce(&mut runs::Run) -> Result<T, Error>,
    print: impl FnOnce(&T) -> Result<(), Error>,
) -> Option<Result<(), Error>> {
    let id = std::env::var(runs::RUN).ok()?;
    // A program that a step starts is not this run.
    std::env::remove_var(runs::RUN);
    runs::stop_on_signals();
    let run = match runs::Run::attach(store, &id) {
        Ok(run) => run,
        Err(e) => return Some(Err(e.into())),
    };
    Some(match work_in(run, work) {
        Ok(done) => print(&done),
        Err(error) => std::process::exit(error.code.exit().into()),
    })
}

/// Do `work` in `run`, and finish the run. A stop that came while the run started wrote only its
/// file: the run then ends `stopped` before its work.
fn work_in<T>(mut run: runs::Run, work: impl FnOnce(&mut runs::Run) -> Result<T, Error>) -> Result<T, Error> {
    let result = match run.stopping() {
        true => Err(stopped()),
        false => work(&mut run),
    };
    finish_run(run, result)
}

/// The error of a run that `obc data runs RUN --stop` or a signal stopped.
pub(super) fn stopped() -> Error {
    Code::Failed.error("the run stopped on `obc data runs RUN --stop` or a signal").fix("Start it again.")
}

/// Refuse `what`, a run that changes the R2 `target`, which serves riders, without `--expect`, the
/// id of the review that the owner approved and that `dry_run` shows; then ask `question` with the
/// origin of the target.
pub(super) fn approved(
    target: &crate::target::Target,
    (expect, yes): (bool, bool),
    (what, dry_run, question): (&str, &str, &str),
) -> Result<(), Error> {
    let crate::target::Target::R2(remote) = target else { return Ok(()) };
    if !expect {
        return Err(Code::Usage
            .error(format!(
                "{what} starts only with `--expect ID`, the id of the review that the owner approved; nothing changed"
            ))
            .fix(format!("Run `{dry_run}, have the owner approve its id, then run the command that it shows.")));
    }
    confirm(&format!("{question} ({})?", remote.url("")), yes)
}

/// Start the detached run of `args`, the arguments after `obc data` of the review `review`: write
/// its run with `json`, else follow its log until it ends.
pub(super) fn start(store: &crate::store::Store, args: &[String], review: &str, json: bool) -> Result<(), Error> {
    let run = runs::detach(store, args)?;
    if json {
        return print_json(&super::update_cli::Started { run, review: review.into() });
    }
    eprintln!("obc data: run {run} of review {review}");
    super::runs_cli::follow(store, &run, false)
}

pub(super) fn start_run(store: &crate::store::Store, args: &[&str]) -> Result<crate::engine::runs::Run, Error> {
    let run = crate::engine::runs::Run::create(store, args.iter().copied())?;
    let id = run.id();
    eprintln!("obc data: run {id}; `obc data runs {id}` shows it");
    Ok(run)
}

/// Refuse a command that writes the R2 target `name` without the credentials of its bucket in
/// `OBC_R2_*`; with `files`, also without rclone, which uploads, lists and checks the files of a
/// release.
pub(super) fn writable(name: &str, target: &crate::target::Target, files: bool) -> Result<(), Error> {
    let crate::target::Target::R2(remote) = target else { return Ok(()) };
    remote.writer().map_err(|e| {
        Code::Blocked.error(format!("target `{name}` is an R2 bucket: {e}")).fix(
            "Set the `OBC_R2_*` variables of the bucket: `tools/obc.local` holds them on a machine that publishes.",
        )
    })?;
    if !files {
        return Ok(());
    }
    let quiet = std::process::Stdio::null;
    let rclone = std::process::Command::new("rclone").arg("version").stdout(quiet()).stderr(quiet()).status();
    if !rclone.is_ok_and(|status| status.success()) {
        return Err(Code::Blocked
            .error(format!("target `{name}` is an R2 bucket, and rclone is not on PATH"))
            .fix("Install rclone: `obc data setup` shows how."));
    }
    Ok(())
}

/// The rule of every command that asks: it asks in a terminal. Without a terminal it needs `--yes`.
pub fn confirm(question: &str, yes: bool) -> Result<(), Error> {
    if yes {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        return Err(Code::NoTerminal.error("there is no terminal to ask in; nothing changed"));
    }
    eprint!("{question} [y/N] ");
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer).map_err(|e| e.to_string())?;
    if !matches!(answer.trim(), "y" | "Y" | "yes") {
        return Err(Code::NotConfirmed.error("not confirmed; nothing changed"));
    }
    Ok(())
}

/// Write the output of a command. `JsonSchema` is required so that each output has a schema for
/// `specs/obc-data.md`; `tests::outputs` lists them.
pub fn print_json(value: &(impl Serialize + JsonSchema)) -> Result<(), Error> {
    println!("{}", serde_json::to_string_pretty(value).map_err(|e| e.to_string())?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use schemars::generate::SchemaSettings;
    use schemars::SchemaGenerator;
    use serde_json::{json, Value};

    use super::*;
    use crate::cli::{
        overview, prepare_cli, r2_cli, rollback_cli, router_cli, runs_cli, serve_cli, setup_cli, unlock_cli, update_cli,
    };
    use crate::engine::runs::{Status, View};

    const UPDATE: &str = "OBC_UPDATE_DATA_SPEC";

    /// The commands that write each schema with `--json`.
    fn outputs(generator: &mut SchemaGenerator) -> Vec<(&'static str, Value)> {
        let schema = |commands, schema: schemars::Schema| (commands, schema.to_value());
        vec![
            schema("`obc data`", generator.subschema_for::<overview::Overview>()),
            schema("`update`", generator.subschema_for::<update_cli::Started>()),
            schema("`update --dry-run`", generator.subschema_for::<update_cli::Review>()),
            schema("`rollback`", generator.subschema_for::<rollback_cli::RolledBack>()),
            schema("`router`", generator.subschema_for::<router_cli::Router>()),
            schema("`unlock`", generator.subschema_for::<unlock_cli::Unlocked>()),
            schema("`sources`", generator.subschema_for::<crate::cli::Sources>()),
            schema("`source refresh`, `serve --detach`", generator.subschema_for::<Detached>()),
            schema("`source prepare`", generator.subschema_for::<update_cli::Started>()),
            schema("`source prepare --dry-run`", generator.subschema_for::<prepare_cli::PrepareReview>()),
            schema("`fetch`", generator.subschema_for::<crate::cli::Fetched>()),
            schema("`areas`", generator.subschema_for::<crate::cli::Areas>()),
            schema("`clean`, `clean --apply`", generator.subschema_for::<crate::cli::CleanPlan>()),
            schema("`serve`, when it listens", generator.subschema_for::<serve_cli::Listening>()),
            schema("`setup`", generator.subschema_for::<setup_cli::Setup>()),
            schema("`runs`", generator.subschema_for::<runs_cli::RunList>()),
            schema("`runs RUN`, `runs RUN --stop`", generator.subschema_for::<View>()),
            schema("`runs RUN --follow`, one per line", generator.subschema_for::<Status>()),
            schema("`r2 list`, `r2 stat`, `r2 delete`", generator.subschema_for::<r2_cli::Objects>()),
            schema("`r2 get`", generator.subschema_for::<r2_cli::Downloaded>()),
            schema("`r2 put`", generator.subschema_for::<r2_cli::Uploaded>()),
            schema("`r2 legacy`", generator.subschema_for::<r2_cli::Legacy>()),
            schema("Every command that fails", generator.subschema_for::<Failure>()),
        ]
    }

    /// The schema name of each output, and the document with the schemas of all of them.
    fn schemas() -> (Vec<(&'static str, String)>, Value) {
        let mut generator = SchemaSettings::draft2020_12().for_serialize().into_generator();
        let outputs = outputs(&mut generator)
            .into_iter()
            .map(|(commands, schema)| {
                let name = schema["$ref"].as_str().expect("each output has a name").trim_start_matches("#/$defs/");
                (commands, name.to_string())
            })
            .collect();
        let document = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$defs": generator.take_definitions(true),
        });
        (outputs, crate::engine::sorted(document))
    }

    fn schemas_section() -> String {
        let (outputs, document) = schemas();
        let mut text = format!(
            "`--json` writes one document of the schema in this table to standard output. The schemas\n\
             come from the Rust types in `host/obc-data`. A test fails when this section is not the one\n\
             that they give; `{UPDATE}=1 cargo test -p obc-data` writes it again.\n\n\
             | Command | Schema |\n| --- | --- |\n",
        );
        for (commands, name) in &outputs {
            text += &format!("| {commands} | `{name}` |\n");
        }
        text + "\n```json\n" + &serde_json::to_string_pretty(&document).unwrap() + "\n```\n"
    }

    fn codes() -> String {
        let schema = SchemaGenerator::default().into_root_schema_for::<Code>().to_value();
        let mut text = String::from("| Code | Exit | When | Fix |\n| --- | --- | --- | --- |\n");
        for variant in schema["oneOf"].as_array().expect("each code has a description") {
            let code: Code = serde_json::from_value(variant["const"].clone()).unwrap();
            let when = variant["description"].as_str().unwrap();
            text +=
                &format!("| `{}` | {} | {when} | {} |\n", variant["const"].as_str().unwrap(), code.exit(), code.fix());
        }
        text
    }

    /// `text` with the body under `heading` replaced: the lines up to the next heading of the
    /// same or a higher level.
    fn replace_section(text: &str, heading: &str, body: &str) -> String {
        let level = heading.find(' ').unwrap();
        let start =
            text.find(&format!("\n{heading}\n")).unwrap_or_else(|| panic!("no `{heading}`")) + heading.len() + 2;
        let end = text[start..]
            .match_indices('\n')
            .map(|(i, _)| start + i + 1)
            .find(|&i| {
                let hashes = text[i..].bytes().take_while(|&b| b == b'#').count();
                (1..=level).contains(&hashes) && text[i + hashes..].starts_with(' ')
            })
            .unwrap_or(text.len());
        let gap = if end == text.len() { "" } else { "\n" };
        format!("{}\n{body}{gap}{}", &text[..start], &text[end..])
    }

    #[test]
    fn a_run_stopped_while_it_starts_ends_stopped_without_its_work() {
        let fixture = crate::engine::tests::fixture("api-stopped");
        let id = runs::reserve(&fixture.store, &["source".into(), "refresh".into(), "earth".into()]).unwrap();
        runs::stop(&fixture.store, &id).unwrap();
        let run = runs::Run::attach(&fixture.store, &id).unwrap();
        let worked = std::cell::Cell::new(false);
        let ended = work_in(run, |_| {
            worked.set(true);
            Ok(())
        });
        assert_eq!((ended.unwrap_err().run, worked.get()), (Some(id.clone()), false));
        assert_eq!(runs::status(&fixture.store, &id).unwrap().state, runs::State::Stopped);
    }

    #[test]
    fn the_spec_has_the_current_error_codes_and_schemas() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../specs/obc-data.md");
        let spec = std::fs::read_to_string(&path).unwrap();
        let current = replace_section(&spec, "### Error codes", &codes());
        let current = replace_section(&current, "## JSON schemas", &schemas_section());
        if std::env::var_os(UPDATE).is_some() {
            std::fs::write(&path, &current).unwrap();
        } else {
            assert!(spec == current, "specs/obc-data.md is not current: run `{UPDATE}=1 cargo test -p obc-data`");
        }
    }

    /// The operator page's tests read these examples in place of `obc data --json`.
    #[test]
    fn each_example_of_the_page_has_the_schema_of_its_command() {
        const EXAMPLES: [(&str, &str); 11] = [
            ("areas.json", "Areas"),
            ("clean.json", "CleanPlan"),
            ("prepare-review.json", "PrepareReview"),
            ("review.json", "Review"),
            ("router.json", "Router"),
            ("run-failed.json", "View"),
            ("run-running.json", "View"),
            ("runs.json", "RunList"),
            ("setup.json", "Setup"),
            ("sources.json", "Sources"),
            ("status.json", "Overview"),
        ];
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../builder/web/test-support/data");
        let mut files: Vec<_> =
            std::fs::read_dir(&dir).unwrap().map(|entry| entry.unwrap().file_name().into_string().unwrap()).collect();
        files.sort();
        assert_eq!(files, EXAMPLES.map(|(file, _)| file), "each example names its schema here");
        let mut compiler = boon::Compiler::new();
        compiler.add_resource("obc-data.json", schemas().1).unwrap();
        let mut compiled = boon::Schemas::new();
        for (file, name) in EXAMPLES {
            let schema = compiler.compile(&format!("obc-data.json#/$defs/{name}"), &mut compiled).unwrap();
            let example = serde_json::from_str(&std::fs::read_to_string(dir.join(file)).unwrap()).unwrap();
            if let Err(e) = compiled.validate(&example, schema) {
                panic!("{file} does not have the schema `{name}`:\n{e:#}");
            }
        }
    }
}
