//! The commands that `tools/landmark_capture.py` calls back (`--select-with BINARY`): offline
//! discovery and selection with the compiler's own rules, so capture and compile cannot drift
//! apart. `obc-bake` and `obc data` both answer them.
//!
//! ```text
//! landmark-candidates --osm FILE --out FILE
//! landmark-content --snapshot FILE --boundary GEOJSON --out DIR
//! landmark-photo-requests --snapshot FILE --boundary GEOJSON --out DIR [--qids FILE]
//! peak-candidates --osm FILE --boundary GEOJSON --out FILE
//! peaks --snapshot FILE --boundary GEOJSON --out DIR
//! ```

use std::collections::BTreeSet;
use std::path::Path;

/// Run the command `args[0]`, or `None` when it is not one of these commands.
pub fn run(args: &[String]) -> Option<Result<(), String>> {
    let (command, rest) = args.split_first()?;
    let result = match command.as_str() {
        "landmark-candidates" => Flags::parse(command, rest, &["osm", "out"])
            .and_then(|flags| super::discover::discover(Path::new(flags.get("osm")?), Path::new(flags.get("out")?))),
        "landmark-content" => landmark_content(command, rest),
        "landmark-photo-requests" => landmark_photo_requests(command, rest),
        "peak-candidates" => Flags::parse(command, rest, &["osm", "boundary", "out"]).and_then(|flags| {
            super::peaks::discover(
                Path::new(flags.get("osm")?),
                Path::new(flags.get("boundary")?),
                Path::new(flags.get("out")?),
            )
        }),
        "peaks" => peaks(command, rest),
        _ => return None,
    };
    Some(result)
}

fn landmark_content(command: &str, args: &[String]) -> Result<(), String> {
    let flags = Flags::parse(command, args, &["snapshot", "boundary", "out"])?;
    let content = super::compile(
        Path::new(flags.get("snapshot")?),
        Path::new(flags.get("boundary")?),
        Path::new(flags.get("out")?),
        false,
    )?;
    println!(
        "{} candidates, {} texts, {} photos ({} RGB222 bytes); {} omissions",
        content.counts.candidates,
        content.counts.texts,
        content.counts.images,
        content.counts.photo_bytes,
        content.omissions.len()
    );
    Ok(())
}

fn landmark_photo_requests(command: &str, args: &[String]) -> Result<(), String> {
    let flags = Flags::parse(command, args, &["snapshot", "boundary", "out", "qids"])?;
    let qids = flags
        .values
        .iter()
        .find(|(name, _)| name == "qids")
        .map(|(_, path)| {
            let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
            let values: Vec<String> = serde_json::from_slice(&bytes).map_err(|e| format!("{path}: {e}"))?;
            let qids: BTreeSet<_> = values.iter().cloned().collect();
            if qids.len() != values.len() || !qids.iter().all(|qid| super::is_qid(qid)) {
                return Err(String::from("photo request QIDs must be unique Q followed by digits"));
            }
            Ok(qids)
        })
        .transpose()?;
    let result = super::photo_requests(
        Path::new(flags.get("snapshot")?),
        Path::new(flags.get("boundary")?),
        Path::new(flags.get("out")?),
        qids.as_ref(),
    )?;
    println!("{} photo request(s)", result.requests.len());
    Ok(())
}

fn peaks(command: &str, args: &[String]) -> Result<(), String> {
    let flags = Flags::parse(command, args, &["snapshot", "boundary", "out"])?;
    let content = super::peaks::compile(
        Path::new(flags.get("snapshot")?),
        Path::new(flags.get("boundary")?),
        Path::new(flags.get("out")?),
        false,
    )?;
    println!(
        "{} peak candidates, {} articles, {} photos, {} associations; {} omissions",
        content.counts.candidates,
        content.counts.texts,
        content.counts.images,
        content.associations.len(),
        content.omissions.len()
    );
    Ok(())
}

/// `--NAME VALUE` for each of `values`; nothing positional.
struct Flags<'a> {
    command: &'a str,
    values: Vec<(String, String)>,
}

impl<'a> Flags<'a> {
    fn parse(command: &'a str, args: &[String], values: &[&str]) -> Result<Self, String> {
        let mut flags = Flags { command, values: Vec::new() };
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let name = arg.strip_prefix("--").ok_or(format!("{command} accepts named flags only"))?;
            if !values.contains(&name) {
                return Err(format!("{command}: unknown flag --{name}"));
            }
            let value = args.next().ok_or(format!("{command}: --{name} needs a value"))?;
            flags.values.push((name.into(), value.clone()));
        }
        Ok(flags)
    }

    fn get(&self, name: &str) -> Result<&str, String> {
        let value = self.values.iter().rev().find(|(n, _)| n == name).map(|(_, value)| value.as_str());
        value.ok_or(format!("{} requires --{name}", self.command))
    }
}

#[cfg(test)]
mod tests {
    use super::run;

    #[test]
    fn an_unknown_flag_is_refused_and_other_commands_pass_on() {
        let args = |words: &[&str]| words.iter().map(|word| word.to_string()).collect::<Vec<_>>();
        let refused = run(&args(&["peak-candidates", "--osm", "a", "--box", "1"])).unwrap().unwrap_err();
        assert!(refused.contains("unknown flag --box"), "{refused}");
        assert!(run(&args(&["bake", "--out", "x"])).is_none());
    }
}
