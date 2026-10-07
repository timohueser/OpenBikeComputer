//! What every command shares: the error with its code and exit status, the question before a
//! change to live, and the JSON output whose schemas `specs/obc-data.md` holds.

use std::io::{BufRead, IsTerminal};
use std::process::ExitCode;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
    /// Another admitted operation owns the environment. No work waits for it.
    Busy,
    /// An argument is not valid, an id names nothing, the command runs outside the repository, or
    /// another command must run first.
    Usage,
    /// A command that changes live has no terminal to ask in, and no `--yes`.
    NoTerminal,
    /// The person did not answer yes.
    NotConfirmed,
    /// A file under `data/` is not valid.
    InvalidData,
    /// A fetch or an upstream check failed.
    FetchFailed,
    /// A credential is missing: a fetch failed without the credential of its source, or the R2
    /// variables are not set. Or `build` has no product that suits the environment.
    Blocked,
    /// R2 or rclone failed, or refused a key.
    R2Failed,
    /// A release failed its check before an apply, or after an upload the object in the bucket is
    /// not the file.
    VerifyFailed,
    /// A run failed: the build, or the run that `runs RUN --follow` shows.
    RunFailed,
    /// The plan file is not the plan of now: live, the steps or the store changed after it was made.
    PlanOutdated,
    /// The store or the file system failed.
    Failed,
}

impl Code {
    pub fn exit(self) -> u8 {
        match self {
            Code::Busy | Code::Usage | Code::NoTerminal => 2,
            Code::PlanOutdated => 3,
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
            Code::Busy => "Observe the current run or retry after it drains. No work is queued.",
            Code::Usage => "Correct the command. `obc data --help` lists the commands and their arguments.",
            Code::NoTerminal => "Show the plan to a person. When they agree, run the command again with `--yes`.",
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
            Code::PlanOutdated => {
                "Make the plan again with `obc data plan ENV --json`, read it, and pass the new file."
            }
            Code::Failed => "Correct the file or the directory that the message names, then run again.",
        }
    }

    pub fn error(self, message: impl Into<String>) -> Error {
        Error { code: self, message: message.into(), fix: self.fix().into(), run: None }
    }
}

impl Error {
    pub(super) fn with_run(mut self, run: &str) -> Self {
        self.run = Some(run.into());
        self
    }
}

/// Finish the caller's journal once, while retaining the original operation failure.
pub(super) fn finish_run<T>(
    run: crate::engine::runs::Run,
    mut result: Result<T, Error>,
    incomplete: Option<&str>,
) -> Result<T, Error> {
    let id = run.id().to_string();
    let finished = run.finish(result.as_ref().err().map(|error| error.message.as_str()).or(incomplete));
    if let Err(message) = finished {
        match &mut result {
            Err(error) => error.message += &format!("; the run journal could not finish: {message}"),
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
                eprintln!("run {run}; `obc data runs {run}` shows its events");
            }
        }
        ExitCode::from(self.code.exit())
    }
}

pub(super) fn start_run(store: &crate::store::Store, command: &str) -> Result<crate::engine::runs::Run, Error> {
    let run = match super::operation_cli::resume(store, command)? {
        Some(run) => run,
        None => crate::engine::runs::Run::create(store, command)?,
    };
    let id = run.id();
    eprintln!("obc data: run {id}; `obc data runs {id} --follow` shows its events");
    Ok(run)
}

/// The rule of every command that changes live: it asks in a terminal. Without a terminal it
/// needs `--yes`, or a plan file where the command takes one.
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
    use crate::cli::{apply_cli, build_cli, edit_cli, r2_cli, regions_cli, runs_cli, status_cli};
    use crate::engine::runs::{Details, Event};

    const UPDATE: &str = "OBC_UPDATE_DATA_SPEC";

    /// The commands that write each schema with `--json`.
    fn outputs(generator: &mut SchemaGenerator) -> Vec<(&'static str, Value)> {
        let schema = |commands, schema: schemars::Schema| (commands, schema.to_value());
        vec![
            schema("`sources`", generator.subschema_for::<crate::cli::Sources>()),
            schema("`versions SOURCE`", generator.subschema_for::<crate::cli::versions::Versions>()),
            schema("`fetch`", generator.subschema_for::<crate::cli::Fetched>()),
            schema("`policy`", generator.subschema_for::<crate::sources::Source>()),
            schema("`region`, `region list`", generator.subschema_for::<crate::cli::RegionList>()),
            schema("`region show`", generator.subschema_for::<crate::cli::RegionDetail>()),
            schema("`region areas`", generator.subschema_for::<regions_cli::Suggestions>()),
            schema("`region create`", generator.subschema_for::<crate::regions::Region>()),
            schema("`region delete`", generator.subschema_for::<regions_cli::Deletion>()),
            schema("`region ENV ID`, `layer`, `undo`", generator.subschema_for::<edit_cli::Edited>()),
            schema("`config review`", generator.subschema_for::<crate::cli::config_cli::Review>()),
            schema("`config commit`", generator.subschema_for::<crate::cli::config_cli::Committed>()),
            schema("`status`, and `obc data` without a terminal", generator.subschema_for::<status_cli::Status>()),
            schema("`clean`, `clean --apply`", generator.subschema_for::<crate::cli::CleanPlan>()),
            schema("`plan`, `dev --check`", generator.subschema_for::<build_cli::EnvPlan>()),
            schema(
                "`prepare`, `build`, `apply`, `dev --prepare`",
                generator.subschema_for::<crate::cli::operation_cli::Handle>(),
            ),
            schema("`dev --start`, `dev --stop`, `dev --status`", generator.subschema_for::<crate::dev::Observed>()),
            schema("`dev --logs`", generator.subschema_for::<crate::dev::Logs>()),
            schema("`dev`, completed dev preparation", generator.subschema_for::<crate::dev::Prepared>()),
            schema("Completed prepare output, `dev --inputs`", generator.subschema_for::<build_cli::Prepared>()),
            schema("Completed build output", generator.subschema_for::<build_cli::Built>()),
            schema("Completed apply output", generator.subschema_for::<apply_cli::Applied>()),
            schema("`auto` admission", generator.subschema_for::<crate::cli::auto_cli::Started>()),
            schema("Completed auto output", generator.subschema_for::<crate::cli::auto_cli::Result>()),
            schema("Live timer state", generator.subschema_for::<crate::schedule::State>()),
            schema("`schedule live --setup-budget`", generator.subschema_for::<crate::operation::budget::Budget>()),
            schema("`runs`", generator.subschema_for::<runs_cli::RunList>()),
            schema("`runs RUN` for a detached operation", generator.subschema_for::<crate::cli::operation_cli::View>()),
            schema("`runs RUN` for other journals", generator.subschema_for::<Details>()),
            schema("`runs RUN --follow`, one per line", generator.subschema_for::<Event>()),
            schema("`r2 list`, `r2 stat`, `r2 delete`", generator.subschema_for::<r2_cli::Objects>()),
            schema("`r2 get`", generator.subschema_for::<r2_cli::Downloaded>()),
            schema("`r2 put`", generator.subschema_for::<r2_cli::Uploaded>()),
            schema("Every command that fails", generator.subschema_for::<Failure>()),
        ]
    }

    fn schemas() -> String {
        let mut generator = SchemaSettings::draft2020_12().for_serialize().into_generator();
        let outputs = outputs(&mut generator);
        let mut text = format!(
            "`--json` writes one document of the schema in this table to standard output. The schemas\n\
             come from the Rust types in `host/obc-data`. A test fails when this section is not the one\n\
             that they give; `{UPDATE}=1 cargo test -p obc-data` writes it again.\n\n\
             | Command | Schema |\n| --- | --- |\n",
        );
        for (commands, schema) in &outputs {
            let name = schema["$ref"].as_str().expect("each output has a name").trim_start_matches("#/$defs/");
            text += &format!("| {commands} | `{name}` |\n");
        }
        let document = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$defs": generator.take_definitions(true),
        });
        let document = serde_json::to_string_pretty(&crate::engine::sorted(document)).unwrap();
        text + "\n```json\n" + &document + "\n```\n"
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
    fn the_spec_has_the_current_error_codes_and_schemas() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../specs/obc-data.md");
        let spec = std::fs::read_to_string(&path).unwrap();
        let current = replace_section(&spec, "### Error codes", &codes());
        let current = replace_section(&current, "## JSON schemas", &schemas());
        if std::env::var_os(UPDATE).is_some() {
            std::fs::write(&path, &current).unwrap();
        } else {
            assert!(spec == current, "specs/obc-data.md is not current: run `{UPDATE}=1 cargo test -p obc-data`");
        }
    }
}
