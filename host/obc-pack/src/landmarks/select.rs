//! The commands that `tools/landmark_capture.py` calls back (`--select-with BINARY`): offline
//! discovery and selection with the compiler's own rules, so capture and compile cannot drift
//! apart. `obc-bake` and `obc data` both answer them.
//!
//! ```text
//! landmark-candidates --osm FILE --out FILE
//! landmark-content --snapshot FILE --boundary GEOJSON --out DIR [--photo-requests]
//! landmark-photo-requests --snapshot FILE --boundary GEOJSON --out DIR [--qids FILE]
//! peak-candidates --osm FILE --boundary GEOJSON --out FILE
//! peaks --snapshot FILE --boundary GEOJSON --out DIR [--photo-requests]
//! boundary --poly FILE --out FILE
//! ```

use std::collections::BTreeSet;
use std::path::Path;

/// Run the command `args[0]`, or `None` when it is not one of these commands.
pub fn run(args: &[String]) -> Option<Result<(), String>> {
    let expected = std::env::var("OBC_CAPTURE_CODE");
    run_bound(args, expected.as_deref().ok())
}

fn run_bound(args: &[String], expected: Option<&str>) -> Option<Result<(), String>> {
    let (command, rest) = args.split_first()?;
    if !matches!(
        command.as_str(),
        "landmark-candidates"
            | "landmark-content"
            | "landmark-photo-requests"
            | "peak-candidates"
            | "peaks"
            | "boundary"
    ) {
        return None;
    }
    let before = match crate::step::capture_code() {
        Ok(code) if expected.is_none_or(|expected| expected == code) => code,
        Ok(_) => return Some(Err("capture code changed; prepare the capture with a fresh worker".into())),
        Err(error) => return Some(Err(error)),
    };
    let result = match command.as_str() {
        "landmark-candidates" => Flags::parse(command, rest, &["osm", "out"], &[])
            .and_then(|flags| super::discover::discover(Path::new(flags.get("osm")?), Path::new(flags.get("out")?))),
        "landmark-content" => landmark_content(command, rest),
        "landmark-photo-requests" => landmark_photo_requests(command, rest),
        "peak-candidates" => Flags::parse(command, rest, &["osm", "boundary", "out"], &[]).and_then(|flags| {
            super::peaks::discover(
                Path::new(flags.get("osm")?),
                Path::new(flags.get("boundary")?),
                Path::new(flags.get("out")?),
            )
        }),
        "peaks" => peaks(command, rest),
        "boundary" => Flags::parse(command, rest, &["poly", "out"], &[]).and_then(|flags| {
            let poly = std::fs::read_to_string(flags.get("poly")?).map_err(|e| format!("--poly: {e}"))?;
            std::fs::write(flags.get("out")?, crate::catalog::boundary::geojson(&poly)?).map_err(|e| e.to_string())
        }),
        _ => unreachable!("recognized selector"),
    };
    Some(result.and_then(|()| {
        if crate::step::capture_code()? != before {
            return Err("capture code changed during selection; start a fresh worker".into());
        }
        Ok(())
    }))
}

fn landmark_content(command: &str, args: &[String]) -> Result<(), String> {
    let flags = Flags::parse(command, args, &["snapshot", "boundary", "out"], &["photo-requests"])?;
    let content = super::compile(
        Path::new(flags.get("snapshot")?),
        Path::new(flags.get("boundary")?),
        Path::new(flags.get("out")?),
        flags.has("photo-requests"),
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
    let flags = Flags::parse(command, args, &["snapshot", "boundary", "out", "qids"], &[])?;
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
    let flags = Flags::parse(command, args, &["snapshot", "boundary", "out"], &["photo-requests"])?;
    let content = super::peaks::compile(
        Path::new(flags.get("snapshot")?),
        Path::new(flags.get("boundary")?),
        Path::new(flags.get("out")?),
        flags.has("photo-requests"),
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

/// `--NAME VALUE` for each of `values`, and `--NAME` for each of `switches`; nothing positional.
struct Flags<'a> {
    command: &'a str,
    values: Vec<(String, String)>,
    switches: Vec<String>,
}

impl<'a> Flags<'a> {
    fn parse(command: &'a str, args: &[String], values: &[&str], switches: &[&str]) -> Result<Self, String> {
        let mut flags = Flags { command, values: Vec::new(), switches: Vec::new() };
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let name = arg.strip_prefix("--").ok_or(format!("{command} accepts named flags only"))?;
            if switches.contains(&name) {
                flags.switches.push(name.into());
            } else if !values.contains(&name) {
                return Err(format!("{command}: unknown flag --{name}"));
            } else {
                let value = args.next().ok_or(format!("{command}: --{name} needs a value"))?;
                flags.values.push((name.into(), value.clone()));
            }
        }
        Ok(flags)
    }

    fn get(&self, name: &str) -> Result<&str, String> {
        let value = self.values.iter().rev().find(|(n, _)| n == name).map(|(_, value)| value.as_str());
        value.ok_or(format!("{} requires --{name}", self.command))
    }

    fn has(&self, name: &str) -> bool {
        self.switches.iter().any(|switch| switch == name)
    }
}

#[cfg(test)]
mod tests {
    use super::{run, run_bound};

    #[test]
    fn a_capture_selector_requires_its_requested_implementation_before_output() {
        let dir = obcm_testkit::scratch::scratch_dir("select", "capture-code");
        let (poly, out) = (dir.join("area.poly"), dir.join("boundary.geojson"));
        let text = "area\n1\n 7.79 47.99\n 7.82 47.99\n 7.82 48.02\n 7.79 47.99\nEND\nEND\n";
        std::fs::write(&poly, text).unwrap();
        let args = vec![
            "boundary".into(),
            "--poly".into(),
            poly.to_str().unwrap().into(),
            "--out".into(),
            out.to_str().unwrap().into(),
        ];
        let error = run_bound(&args, Some("old implementation")).unwrap().unwrap_err();
        assert!(error.contains("capture code changed"), "{error}");
        assert!(!out.exists());
        let code = crate::step::capture_code().unwrap();
        run_bound(&args, Some(&code)).unwrap().unwrap();
        assert_eq!(std::fs::read_to_string(out).unwrap(), crate::catalog::boundary::geojson(text).unwrap());
    }

    #[test]
    fn the_capture_gets_its_boundary_from_the_poly_and_other_commands_pass_on() {
        let dir = obcm_testkit::scratch::scratch_dir("select", "boundary");
        let (poly, out) = (dir.join("area.poly"), dir.join("boundary.geojson"));
        let text = "area\n1\n 7.79 47.99\n 7.82 47.99\n 7.82 48.02\n 7.79 47.99\nEND\nEND\n";
        std::fs::write(&poly, text).unwrap();
        let args = |words: &[&str]| words.iter().map(|word| word.to_string()).collect::<Vec<_>>();
        let (poly, out) = (poly.to_str().unwrap(), out.to_str().unwrap());
        run(&args(&["boundary", "--poly", poly, "--out", out])).unwrap().unwrap();
        let written = std::fs::read_to_string(out).unwrap();
        assert_eq!(written, crate::catalog::boundary::geojson(text).unwrap());
        let refused = run(&args(&["boundary", "--poly", poly, "--box", "1"])).unwrap().unwrap_err();
        assert!(refused.contains("unknown flag --box"), "{refused}");
        assert!(run(&args(&["bake", "--out", out])).is_none());
    }
}
