//! Review working bake configuration and commit only those paths through ordinary Git.

use std::{collections::BTreeMap, path::Path, process::Command, time::Duration};

use clap::Subcommand;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{confirm, print_json, Code, Error};
use crate::store::sha256_hex;

const PATHS: [&str; 5] =
    ["data/sources.toml", "data/env/live.toml", "data/planner.toml", "data/planner-runtime.toml", "data/regions/"];

#[derive(Subcommand)]
pub(super) enum Action {
    /// Review changed bake configuration, including new and deleted regions.
    Review,
    /// Commit the exact saved review. Unrelated staged files stay staged; nothing pushes.
    Commit {
        #[arg(long)]
        review: std::path::PathBuf,
        #[arg(short, long)]
        message: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "ConfigFile")]
pub(super) struct File {
    sha256: String,
    mode: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "ConfigReview")]
pub(super) struct Review {
    pub head: String,
    pub files: BTreeMap<String, Option<File>>,
    pub diff: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(rename = "ConfigCommit")]
pub(super) struct Committed {
    pub commit: String,
}

fn git(root: &Path, args: &[&str]) -> Result<String, Error> {
    // The existing finite process transport captures both Git diagnostics and hook output.
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "exec \"$@\" 2>&1", "obc-config", "git", "-C"])
        .arg(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0");
    let (status, output) =
        crate::operation::launch::bounded_status(&mut command, Duration::from_secs(30)).map_err(|message| {
            Code::Failed
                .error(message)
                .fix("Inspect Git state, then refresh the configuration review. Git in a terminal remains available.")
        })?;
    let output = String::from_utf8(output).map_err(|_| Code::Failed.error("Git output is not UTF-8."))?;
    if !status.success() {
        return Err(Code::Failed
            .error(output.trim())
            .fix("Correct the reported Git, hook or signing error, then refresh the review. No push runs."));
    }
    Ok(output)
}

fn read_file(root: &Path, path: &str) -> Result<Option<File>, Error> {
    let file = root.join(path);
    let metadata = match std::fs::symlink_metadata(&file) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{path}: {error}").into()),
    };
    if !metadata.is_file() {
        return Err(Code::Usage.error(format!("{path} must be an ordinary configuration file.")));
    }
    #[cfg(unix)]
    let executable = {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    };
    #[cfg(not(unix))]
    let executable = false;
    Ok(Some(File {
        sha256: sha256_hex(&std::fs::read(&file).map_err(|error| format!("{path}: {error}"))?),
        mode: if executable { "100755" } else { "100644" }.into(),
    }))
}

pub(super) fn review(root: &Path) -> Result<Review, Error> {
    let head = git(root, &["rev-parse", "HEAD"])?.trim().to_string();
    let mut args = vec!["diff", "--relative", "--name-only", "--no-renames", "-z", "HEAD", "--"];
    args.extend(PATHS);
    let tracked = git(root, &args)?;
    let mut args = vec!["ls-files", "--others", "--exclude-standard", "-z", "--"];
    args.extend(PATHS);
    let new = git(root, &args)?;
    let files = tracked
        .split('\0')
        .chain(new.split('\0'))
        .filter(|path| !path.is_empty())
        .map(|path| read_file(root, path).map(|file| (path.to_string(), file)))
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let mut args = vec!["diff", "--relative", "--no-ext-diff", "--no-color", "--no-renames", "HEAD", "--"];
    args.extend(files.keys().map(String::as_str));
    let mut diff = if files.is_empty() { String::new() } else { git(root, &args)? };
    for path in new.split('\0').filter(|path| !path.is_empty()) {
        let text = std::fs::read_to_string(root.join(path)).map_err(|error| format!("{path}: {error}"))?;
        let mode = &files[path].as_ref().ok_or("A new configuration file disappeared.")?.mode;
        diff += &format!("\nNew file: {path} ({mode})\n{text}");
        if !text.ends_with('\n') {
            diff += "\n\\ No newline at end of file\n";
        }
    }
    if git(root, &["rev-parse", "HEAD"])?.trim() != head
        || files.iter().any(|(path, file)| read_file(root, path).map_or(true, |now| &now != file))
    {
        return Err(Code::PlanOutdated
            .error("Configuration changed while its review was read.")
            .fix("Refresh the configuration review."));
    }
    Ok(Review { head, files, diff })
}

pub(super) fn commit(root: &Path, expected: &Review, message: &str) -> Result<Committed, Error> {
    if message.trim().is_empty() || expected.files.is_empty() {
        return Err(Code::Usage.error("A commit needs changed configuration and a message."));
    }
    if &review(root)? != expected {
        return Err(Code::PlanOutdated
            .error("HEAD or the reviewed configuration changed.")
            .fix("Refresh the review before committing."));
    }
    let mut args = vec!["ls-files", "--others", "--exclude-standard", "-z", "--"];
    args.extend(expected.files.keys().map(String::as_str));
    let new = git(root, &args)?;
    let new: Vec<_> = new.split('\0').filter(|path| !path.is_empty()).collect();
    if !new.is_empty() {
        let mut args = vec!["add", "--intent-to-add", "--"];
        args.extend(&new);
        git(root, &args)?;
    }
    let mut args = vec!["commit", "--only", "-m", message, "--"];
    args.extend(expected.files.keys().map(String::as_str));
    let result = git(root, &args);
    let head = git(root, &["rev-parse", "HEAD"])?.trim().to_string();
    if head == expected.head {
        let mut error = result.err().unwrap_or_else(|| Code::Failed.error("Git did not create a commit."));
        if !new.is_empty() {
            let mut args = vec!["update-index", "--force-remove", "--"];
            args.extend(&new);
            if let Err(restore) = git(root, &args) {
                error.message += &format!("; selected new-file intent could not be removed: {}", restore.message);
            }
        }
        return Err(error);
    }
    let outcome = || -> Result<(), Error> {
        if git(root, &["rev-parse", "HEAD^"])?.trim() != expected.head {
            return Err(Code::Failed.error("The new commit has a different parent."));
        }
        let changed =
            git(root, &["diff", "--relative", "--name-only", "--no-renames", "-z", &expected.head, "HEAD", "--"])?;
        let changed: std::collections::BTreeSet<_> = changed.split('\0').filter(|path| !path.is_empty()).collect();
        if changed != expected.files.keys().map(String::as_str).collect() {
            return Err(Code::Failed.error("The commit includes a different set of paths."));
        }
        for (path, file) in &expected.files {
            let entry = git(root, &["ls-tree", "HEAD", "--", path])?;
            match file {
                None if entry.is_empty() => {}
                Some(file) if entry.split_whitespace().next() == Some(file.mode.as_str()) => {
                    let bytes = git(root, &["show", &format!("HEAD:./{path}")])?;
                    if sha256_hex(bytes.as_bytes()) != file.sha256 {
                        return Err(Code::Failed.error(format!("{path} differs from its reviewed bytes.")));
                    }
                }
                _ => return Err(Code::Failed.error(format!("{path} differs from its reviewed mode or deletion."))),
            }
        }
        result.map(|_| ())
    };
    outcome().map_err(|mut error| {
        error.message = format!("Git HEAD is now {head}. {}", error.message);
        error.fix =
            "Inspect the actual commit and refresh the review. Unrelated staged work is not restored or replaced."
                .into();
        error
    })?;
    Ok(Committed { commit: head })
}

pub(super) fn run(root: &Path, action: Action, json: bool) -> Result<(), Error> {
    match action {
        Action::Review => {
            let review = review(root)?;
            if json {
                print_json(&review)
            } else {
                print!("HEAD {}\n{}", review.head, review.diff);
                Ok(())
            }
        }
        Action::Commit { review, message, yes } => {
            let review: Review = serde_json::from_slice(&std::fs::read(&review).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?;
            confirm("Commit the reviewed bake configuration on this machine? Nothing pushes.", yes)?;
            let committed = commit(root, &review, &message)?;
            if json {
                print_json(&committed)
            } else {
                println!("Committed {}", committed.commit);
                Ok(())
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::{engine::tests::write, store::tests::Scratch};

    fn repository(label: &str) -> Scratch {
        let scratch = Scratch::new(label);
        git(&scratch.0, &["init", "-q"]).unwrap();
        for (key, value) in [("user.name", "Test"), ("user.email", "test@example.org"), ("commit.gpgsign", "false")] {
            git(&scratch.0, &["config", key, value]).unwrap();
        }
        write(&scratch.0.join("data/sources.toml"), "original\n");
        write(&scratch.0.join("data/regions/delete.toml"), "delete\n");
        write(&scratch.0.join("unrelated.txt"), "original\n");
        write(&scratch.0.join(".gitignore"), "data/env/local.toml\n");
        git(&scratch.0, &["add", "."]).unwrap();
        git(&scratch.0, &["commit", "-qm", "Initial"]).unwrap();
        scratch
    }

    #[test]
    fn partial_commit_includes_new_and_deleted_configs_but_keeps_unrelated_staged_bytes() {
        let scratch = repository("config-commit");
        let root = &scratch.0;
        write(&root.join("data/sources.toml"), "staged source\n");
        write(&root.join("unrelated.txt"), "staged unrelated\n");
        git(root, &["add", "data/sources.toml", "unrelated.txt"]).unwrap();
        write(&root.join("data/sources.toml"), "reviewed source\n");
        write(&root.join("data/regions/new.toml"), "new\n");
        write(&root.join("data/env/local.toml"), "ignored selection\n");
        std::fs::remove_file(root.join("data/regions/delete.toml")).unwrap();
        let reviewed = review(root).unwrap();
        assert_eq!(
            reviewed.files.keys().map(String::as_str).collect::<Vec<_>>(),
            ["data/regions/delete.toml", "data/regions/new.toml", "data/sources.toml"]
        );
        assert!(reviewed.diff.contains("reviewed source") && reviewed.diff.contains("New file: data/regions/new.toml"));
        let committed = commit(root, &reviewed, "Reviewed bake configuration").unwrap();
        assert_ne!(committed.commit, reviewed.head);
        assert_eq!(git(root, &["show", "HEAD:data/sources.toml"]).unwrap(), "reviewed source\n");
        assert_eq!(git(root, &["show", ":unrelated.txt"]).unwrap(), "staged unrelated\n");
        assert_eq!(git(root, &["show", "HEAD:unrelated.txt"]).unwrap(), "original\n");
        assert_eq!(git(root, &["diff", "--cached", "--name-only"]).unwrap(), "unrelated.txt\n");
    }

    #[test]
    fn stale_reviews_refuse_head_bytes_modes_and_new_paths() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = repository("config-stale");
        let root = &scratch.0;
        let file = root.join("data/sources.toml");
        write(&file, "reviewed\n");
        let reviewed = review(root).unwrap();
        write(&file, "changed\n");
        assert_eq!(commit(root, &reviewed, "Refused").unwrap_err().code, Code::PlanOutdated);
        write(&file, "reviewed\n");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(commit(root, &reviewed, "Refused").unwrap_err().code, Code::PlanOutdated);
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        write(&root.join("data/regions/new.toml"), "new\n");
        assert_eq!(commit(root, &reviewed, "Refused").unwrap_err().code, Code::PlanOutdated);
        std::fs::remove_file(root.join("data/regions/new.toml")).unwrap();
        git(root, &["commit", "--allow-empty", "-qm", "Another commit"]).unwrap();
        assert_eq!(commit(root, &reviewed, "Refused").unwrap_err().code, Code::PlanOutdated);
    }

    #[test]
    fn failed_hook_removes_only_new_intent_and_preserves_the_partial_index() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = repository("config-hook");
        let root = &scratch.0;
        write(&root.join("unrelated.txt"), "staged unrelated\n");
        write(&root.join("data/sources.toml"), "staged source\n");
        git(root, &["add", "unrelated.txt", "data/sources.toml"]).unwrap();
        let before = git(root, &["ls-files", "--stage"]).unwrap();
        write(&root.join("data/sources.toml"), "working source\n");
        write(&root.join("data/regions/new.toml"), "new\n");
        let hook = root.join(".git/hooks/pre-commit");
        write(&hook, "#!/bin/sh\necho 'Hook refuses configuration' >&2\nexit 1\n");
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        let reviewed = review(root).unwrap();
        let error = commit(root, &reviewed, "Refused").unwrap_err();
        assert!(error.message.contains("Hook refuses configuration"), "{}", error.message);
        assert_eq!(git(root, &["rev-parse", "HEAD"]).unwrap().trim(), reviewed.head);
        assert_eq!(git(root, &["ls-files", "--stage"]).unwrap(), before);
        assert_eq!(review(root).unwrap(), reviewed);
        std::fs::remove_file(hook).unwrap();
        git(root, &["config", "commit.gpgsign", "true"]).unwrap();
        git(root, &["config", "gpg.program", "/missing-config-signer"]).unwrap();
        let error = commit(root, &reviewed, "Signing refuses").unwrap_err();
        assert!(error.message.contains("missing-config-signer"), "{}", error.message);
        assert_eq!(git(root, &["rev-parse", "HEAD"]).unwrap().trim(), reviewed.head);
        assert_eq!(git(root, &["ls-files", "--stage"]).unwrap(), before);
    }

    #[test]
    fn a_hook_altered_commit_reports_its_actual_head_without_claiming_reviewed_success() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = repository("config-hook-change");
        let root = &scratch.0;
        write(&root.join("data/sources.toml"), "reviewed source\n");
        let hook = root.join(".git/hooks/pre-commit");
        write(&hook, "#!/bin/sh\nprintf 'hook source\\n' > data/sources.toml\ngit add data/sources.toml\n");
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        let reviewed = review(root).unwrap();
        let error = commit(root, &reviewed, "Hook changes bytes").unwrap_err();
        let head = git(root, &["rev-parse", "HEAD"]).unwrap();
        assert_ne!(head.trim(), reviewed.head);
        assert!(error.message.contains(head.trim()) && error.message.contains("reviewed bytes"), "{}", error.message);
        assert_eq!(git(root, &["show", "HEAD:data/sources.toml"]).unwrap(), "hook source\n");
    }
}
