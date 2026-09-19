//! `obc inspect <file>` — name a binary artefact's format and summarise what is inside it.
//!
//! Every format is read through the crate that owns it, so this tool cannot disagree with the
//! device. It opens a file read-only, it reports what is there, and it repairs nothing. A file it
//! cannot read gives a report of the damage and a non-zero exit.

mod card;
mod obcm;
mod obcr;
mod obct;
mod report;
mod settings;
mod source;

use std::path::Path;
use std::process::ExitCode;

use obc_formats::io::ByteSource;

use report::Report;
use source::FileSource;

const USAGE: &str = "usage: obc inspect <file> [--json]";

/// The formats this tool names, and how a file is recognised as one.
#[derive(Clone, Copy)]
enum Format {
    Map,
    Route,
    Terrain,
    Card,
    Trip,
    Settings,
}

impl Format {
    fn name(self) -> &'static str {
        match self {
            Format::Map => "obcm",
            Format::Route => "obcr",
            Format::Terrain => "obct",
            Format::Card => "card",
            Format::Trip => "obt",
            Format::Settings => "settings",
        }
    }
}

fn main() -> ExitCode {
    let mut json = false;
    let mut path = None;
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--json" => json = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ if path.is_none() => path = Some(argument),
            _ => return fail(&format!("one file at a time. {USAGE}")),
        }
    }
    let Some(path) = path else { return fail(USAGE) };
    let path = Path::new(&path);

    let mut out = Report::new();
    out.put("path", path.display().to_string());
    let outcome = inspect(path, &mut out);
    if let Err(damage) = &outcome {
        out.put("damage", damage.clone());
    }
    print!("{}", if json { out.json() } else { out.text() });
    if outcome.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Name the format, then hand the file to the crate that owns it. Both halves add to `out`, so a
/// damaged file still reports which format it was read as.
fn inspect(path: &Path, out: &mut Report) -> Result<(), String> {
    let source = FileSource::open(path).map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let format = match classify(&source, path) {
        Some(format) => format,
        None => sniff(&source, path)?,
    };
    out.put("format", format.name());

    let body = match format {
        Format::Map => obcm::report(&source),
        Format::Route => obcr::route(&source),
        Format::Terrain => obct::report(&source),
        Format::Trip => obcr::trip(&source),
        Format::Card => card::report(&source),
        Format::Settings => {
            let blob = std::fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
            settings::report(&blob)
        }
    }?;
    out.extend(body);
    Ok(())
}

/// The magic bytes name four of the six formats. The trip object and the settings blob carry none
/// — both begin with a version byte — so the file name selects those two.
fn classify(source: &FileSource, path: &Path) -> Option<Format> {
    let mut magic = [0u8; 4];
    if source.read_at(0, &mut magic).is_ok() {
        if magic == obc_formats::obcm::MAGIC {
            return Some(Format::Map);
        }
        if magic == *obc_formats::obcr::MAGIC {
            return Some(Format::Route);
        }
        if magic == obc_formats::obct::MAGIC {
            return Some(Format::Terrain);
        }
        if magic == obc_storage::flat::SUPERBLOCK_MAGIC {
            return Some(Format::Card);
        }
    }
    let name = path.file_name().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
    if name.ends_with(".obt") {
        return Some(Format::Trip);
    }
    if name == "obc-settings.bin" {
        return Some(Format::Settings);
    }
    None
}

/// No magic and no name to go by: offer the file to the two decoders that can refuse a stranger.
/// A trip object's length is fixed by its own header, and a settings blob is CRC-covered, so
/// neither accepts bytes that are not its own.
fn sniff(source: &FileSource, path: &Path) -> Result<Format, String> {
    if obcr::trip(source).is_ok() {
        return Ok(Format::Trip);
    }
    if std::fs::read(path).is_ok_and(|blob| settings::report(&blob).is_ok()) {
        return Ok(Format::Settings);
    }
    let mut magic = [0u8; 4];
    let prefix = match source.read_at(0, &mut magic) {
        Ok(()) => format!(" and begins {magic:02x?}"),
        Err(_) => String::new(),
    };
    Err(format!("no format recognised. The file is {} bytes{prefix}", source.len()))
}

fn fail(message: &str) -> ExitCode {
    eprintln!("obc inspect: {message}");
    ExitCode::FAILURE
}
