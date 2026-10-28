//! Where the credential of a source lives on this machine. An `env` credential is a line of
//! `~/.config/openbikecomputer/credentials.env`, which `obc data` and `obc bake publish` load at
//! start; a `file` credential is the file that its reader takes. A value never goes to argv, to a log or to the
//! output of a command.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::sources::Credential;

/// The `NAME=VALUE` lines of the `env` credentials.
const ENV_FILE: &str = "~/.config/openbikecomputer/credentials.env";

/// The files that a `file` credential can name: the separator of their `NAME<separator>VALUE`
/// lines, and the names that their reader takes.
const FILES: [(&str, &str, &[&str]); 2] = [
    // `tools/planner_climate.py` reads `key: TOKEN`.
    ("~/.cdsapirc", ": ", &["key"]),
    // `tools/planner_snow.py` reads `NAME=VALUE`, without spaces or quotes.
    ("~/.config/openbikecomputer/cdse-s3.env", "=", &["CDSE_S3_ACCESS_KEY", "CDSE_S3_SECRET_KEY"]),
];

/// The files that a person saves whole, as the `hint` of their source says. No writer here
/// writes them.
const SAVED: [&str; 1] = [
    // `tools/clms.py` reads the service key of the Copernicus Land Monitoring Service.
    "~/.config/openbikecomputer/clms-key.json",
];

/// The file of a credential under the home directory, and the names of its values in the order
/// that `obc data source credential` reads them.
pub struct Place {
    pub file: &'static str,
    separator: &'static str,
    pub names: Vec<String>,
}

/// The place of `credential`; `None` for a file that no writer here knows.
pub fn place(credential: &Credential) -> Option<Place> {
    let Some(file) = &credential.file else {
        return Some(Place { file: ENV_FILE, separator: "=", names: credential.env.clone() });
    };
    let (file, separator, names) = FILES.iter().find(|(known, ..)| known == file)?;
    Some(Place { file, separator, names: names.iter().map(|name| name.to_string()).collect() })
}

/// Whether a writer here writes the file of `credential`, or a person saves it whole.
pub fn known(credential: &Credential) -> bool {
    place(credential).is_some() || credential.file.as_deref().is_some_and(|file| SAVED.contains(&file))
}

/// The files that a `file` credential can name.
pub fn files() -> String {
    FILES.iter().map(|(file, ..)| *file).chain(SAVED).collect::<Vec<_>>().join(", ")
}

/// `path`, which starts with `~/`, under `home`.
fn under(home: &Path, path: &str) -> PathBuf {
    home.join(path.trim_start_matches("~/"))
}

/// The variables of the credentials file under `home`.
pub fn env_file(home: &Path) -> BTreeMap<String, String> {
    let text = std::fs::read_to_string(under(home, ENV_FILE)).unwrap_or_default();
    let line = |line: &str| line.split_once('=').map(|(name, value)| (name.trim().to_string(), value.to_string()));
    text.lines().filter(|line| !line.trim_start().starts_with('#')).filter_map(line).collect()
}

/// Set each variable of the credentials file under `home` that the environment does not set: a
/// variable from `tools/obc.local` or the shell wins.
pub fn load(home: &Path) {
    for (name, value) in unset(env_file(home), |name| std::env::var_os(name).is_some_and(|v| !v.is_empty())) {
        std::env::set_var(name, value);
    }
}

fn unset(file: BTreeMap<String, String>, set: impl Fn(&str) -> bool) -> Vec<(String, String)> {
    file.into_iter().filter(|(name, value)| !value.is_empty() && !set(name)).collect()
}

/// Write `values`, in the order of `place.names`, to the file of `place` under `home`. The file
/// keeps its other lines and gets mode 0600; a directory that this makes gets mode 0700.
pub fn write(home: &Path, place: &Place, values: &[String]) -> Result<PathBuf, String> {
    let names = place.names.join(", ");
    if values.len() != place.names.len() {
        return Err(format!("the credential takes {} values, one per line: {names}", place.names.len()));
    }
    if values.iter().any(|value| value.is_empty() || value.chars().any(char::is_control)) {
        return Err(format!("each value of {names} is one line of text, and not empty"));
    }
    let path = under(home, place.file);
    let dir = path.parent().ok_or("the credential file has no directory")?;
    let failed = |e: std::io::Error| format!("{}: {e}", path.display());
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir).map_err(failed)?;
    let old = match std::fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        old => old.map_err(failed)?,
    };
    let text = merge(&old, place.separator, &place.names, values);
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let temporary = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let written = options.open(&temporary).and_then(|mut file| {
        file.write_all(text.as_bytes())?;
        file.sync_all()
    });
    written.and_then(|()| std::fs::rename(&temporary, &path)).map_err(|e| {
        let _ = std::fs::remove_file(&temporary);
        failed(e)
    })?;
    Ok(path)
}

/// `text` without the lines of `names`, then a line for each name with its value.
fn merge(text: &str, separator: &str, names: &[String], values: &[String]) -> String {
    let named = |line: &str| {
        line.split_once(separator.trim()).is_some_and(|(name, _)| names.contains(&name.trim().to_string()))
    };
    let kept = text.lines().filter(|line| !named(line)).map(str::to_string);
    let set = names.iter().zip(values).map(|(name, value)| format!("{name}{separator}{value}"));
    kept.chain(set).map(|line| line + "\n").collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::Scratch;

    fn env(names: &[&str]) -> Credential {
        Credential { env: names.iter().map(|name| name.to_string()).collect(), ..Default::default() }
    }

    #[cfg(unix)]
    #[test]
    fn a_credential_is_written_private_beside_the_other_lines_of_its_file() {
        use std::os::unix::fs::PermissionsExt;
        let home = Scratch::new("credentials-write");
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        let token = place(&env(&["OBC_TEST_TOKEN"])).unwrap();
        let path = write(&home.0, &token, &["fake-token-1".into()]).unwrap();
        assert_eq!((mode(&path), mode(path.parent().unwrap())), (0o600, 0o700));
        let pair = place(&env(&["OBC_TEST_USER", "OBC_TEST_PASSWORD"])).unwrap();
        write(&home.0, &pair, &["fake-user".into(), "fake password".into()]).unwrap();
        write(&home.0, &token, &["fake-token-2".into()]).unwrap();
        let expected = [
            ("OBC_TEST_PASSWORD", "fake password"),
            ("OBC_TEST_TOKEN", "fake-token-2"),
            ("OBC_TEST_USER", "fake-user"),
        ];
        let expected = expected.map(|(name, value)| (name.to_string(), value.to_string())).into();
        assert_eq!(env_file(&home.0), expected);
        assert_eq!(mode(&path), 0o600);

        let cds = Credential { file: Some("~/.cdsapirc".into()), ..Default::default() };
        std::fs::write(home.0.join(".cdsapirc"), "url: https://cds.example/api\nkey: old-fake\n").unwrap();
        write(&home.0, &place(&cds).unwrap(), &["new-fake".into()]).unwrap();
        let cdsapirc = std::fs::read_to_string(home.0.join(".cdsapirc")).unwrap();
        assert_eq!(cdsapirc, "url: https://cds.example/api\nkey: new-fake\n");
        assert_eq!(mode(&home.0.join(".cdsapirc")), 0o600);

        for values in [&[][..], &["a".into(), "b".into()], &["two\nlines".into()], &["".into()]] {
            assert!(write(&home.0, &token, values).is_err(), "{values:?}");
        }
        assert_eq!(env_file(&home.0)["OBC_TEST_TOKEN"], "fake-token-2");
    }

    #[test]
    fn a_variable_of_the_environment_wins_over_the_file() {
        let file = [("OBC_TEST_SET", "from-file"), ("OBC_TEST_UNSET", "from-file"), ("OBC_TEST_EMPTY", "")];
        let file = file.map(|(name, value)| (name.to_string(), value.to_string())).into();
        let loaded = unset(file, |name| name == "OBC_TEST_SET");
        assert_eq!(loaded, [("OBC_TEST_UNSET".to_string(), "from-file".to_string())]);
    }
}
