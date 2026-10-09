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
            | "content-query"
            | "content-photo"
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
        "content-query" => content_query(command, rest),
        "content-photo" => Flags::parse(command, rest, &["input", "out"], &[]).and_then(|flags| {
            let bytes = std::fs::read(flags.get("input")?).map_err(|e| e.to_string())?;
            let pixels = super::photo::prepare(&bytes).map_err(str::to_owned)?;
            std::fs::write(flags.get("out")?, pixels).map_err(|e| e.to_string())
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

fn content_query(command: &str, args: &[String]) -> Result<(), String> {
    use serde_json::{json, Value};
    use std::collections::BTreeMap;
    let flags = Flags::parse(command, args, &["files", "roots", "phase", "out"], &[])?;
    let read = |path: &str| -> Result<Value, String> {
        serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    };
    let files: BTreeMap<String, std::path::PathBuf> =
        serde_json::from_value(read(flags.get("files")?)?).map_err(|e| e.to_string())?;
    let roots = read(flags.get("roots")?)?;
    let mut ids: Vec<String> = serde_json::from_value(roots["entities"].clone()).map_err(|e| e.to_string())?;
    let facts = super::shared::facts(&files)?;
    for link in roots["links"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        if let Some(id) = facts.get(&("link".into(), link.into())).and_then(|fact| fact["identity"].as_str()) {
            if super::is_qid(id) {
                ids.push(id.into());
            }
        }
    }
    let mut subjects = super::shared::selected(&facts, &ids);
    for peak in roots["peaks"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        if super::is_qid(peak) {
            subjects.insert(peak.to_owned());
        } else if let Some(link) = facts.get(&("link".into(), peak.into())) {
            if let Some(id) = link["identity"].as_str() {
                subjects.insert(id.into());
            }
        }
    }
    let mut query = json!({"entities":roots["entities"],"links":roots["links"],"articles":[],"commons":[],"categories":[],"files":[]});
    let mut articles = BTreeSet::new();
    let mut categories = BTreeSet::new();
    let mut commons = BTreeSet::new();
    for id in &subjects {
        if let Some(entity) = super::shared::subject(&facts, id) {
            for (wiki, link) in entity["sitelinks"].as_object().into_iter().flatten() {
                if let Some(language) = wiki.strip_suffix("wiki") {
                    if let Some(title) = link["title"].as_str() {
                        articles.insert((language.to_owned(), title.to_owned()));
                    }
                }
            }
            categories.extend(super::shared::categories(&entity));
            for claim in entity["claims"]["P18"].as_array().into_iter().flatten() {
                if let Some(name) = claim["mainsnak"]["datavalue"]["value"].as_str() {
                    commons.insert(name.to_owned());
                }
            }
        }
    }
    query["articles"] = json!(articles
        .into_iter()
        .map(|(language, title)| json!({"language":language,"title":title}))
        .collect::<Vec<_>>());
    query["categories"] = json!(categories);
    let phase = flags.get("phase")?.parse::<u8>().map_err(|e| e.to_string())?;
    if phase > 0 {
        for fact in facts.values() {
            if fact["kind"] == "article" {
                if let Some(name) = super::shared::lead_image(fact) {
                    commons.insert(name);
                }
            } else if fact["kind"] == "category" {
                commons.extend(
                    fact["members"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|member| member["title"].as_str())
                        .filter_map(|name| name.strip_prefix("File:"))
                        .map(str::to_owned),
                );
            }
        }
        query["commons"] = json!(commons);
    }
    if phase > 1 {
        let out = Path::new(flags.get("out")?);
        let view = out.with_extension("view");
        if view.exists() {
            std::fs::remove_dir_all(&view).map_err(|e| e.to_string())?;
        }
        std::fs::create_dir_all(&view).map_err(|e| e.to_string())?;
        let boundary = view.join("boundary.json");
        std::fs::write(
            &boundary,
            br#"{"type":"Polygon","coordinates":[[[-180,-90],[180,-90],[180,90],[-180,90],[-180,-90]]]}"#,
        )
        .map_err(|e| e.to_string())?;
        let landmarks = super::shared::selected(&facts, &ids).into_iter().collect::<Vec<_>>();
        super::shared::view(&files, &view, &landmarks)?;
        let mut requested =
            super::photo_requests(&view.join("manifest.json"), &boundary, &view.join("landmarks"), None)?.requests;
        if roots["summits"].as_array().is_some_and(|values| !values.is_empty()) {
            std::fs::write(
                view.join("summits.json"),
                serde_json::to_vec(&json!({"schema":1,"osm_sha256":roots["osm_sha256"],"summits":roots["summits"]}))
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            super::shared::peak_view(&files, &view)?;
            requested.extend(
                super::peaks::compile(&view.join("manifest.json"), &boundary, &view.join("peaks"), true)?
                    .photo_requests,
            );
        }
        let mut wanted = facts
            .values()
            .filter(|fact| fact["kind"] == "file")
            .filter_map(|fact| fact["filename"].as_str())
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        wanted.extend(requested.into_iter().map(|request| request.filename));
        query["files"] = json!(wanted);
    }
    std::fs::write(flags.get("out")?, serde_json::to_vec(&query).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
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
