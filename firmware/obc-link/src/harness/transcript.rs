//! The checked-in semantic transcripts, read back from `specs/vectors/device-object-v2/`.
//!
//! `Device_Object_Vectors_v2.md` §1: "A transcript is an ordered event list. Each event names
//! actor/principal, link and connection generation, request or stream bytes, injected
//! disconnect/reset/cut ... and expected response." The files are producer-generated and their
//! shape is pinned by the fixture guard, so the reader here is a small scanner over that shape
//! rather than a general JSON parser this crate has no dependency for.
//!
//! What the harness does with them is deliberately layered, because a transcript is a *script* and
//! not a byte oracle for an engine's own output:
//!
//! 1. **Framing.** Every record is pushed through both fake links and must come back byte
//!    identical. That is §14's claim — "The common frame bytes above are identical on both links" —
//!    checked rather than asserted.
//! 2. **Dispatch.** Every client record must decode to a typed request or stream frame and every
//!    device record to a typed response, through the same codec the engine dispatches on.
//! 3. **Drive.** A transcript that starts at Hello and stays inside the restart-only profile is
//!    replayed *through the engine* on both links, with each device event's opcode and
//!    success/error class compared against what the engine produced.
//!
//! Layer 3 cannot apply to a transcript that opens mid-flow against device state no fixture
//! carries, or that exercises a profile this device does not ship (resume) or a slice this engine
//! does not have yet (drafts). Those are named in [`DRIVEN`] rather than skipped silently.

use std::fs;
use std::path::PathBuf;
use std::string::{String, ToString};
use std::vec::Vec;

/// Which transcripts layer 3 drives through the engine, and why the rest are framing/dispatch only.
///
/// The reason strings are not decoration: each one names the device state or the profile the
/// transcript needs, so a later slice can move a row from `false` to `true` by supplying it.
pub const DRIVEN: [(&str, bool, &str); 11] = [
    ("create-upload-publish-and-download", true, "starts at Hello and stays inside the restart-only profile"),
    ("abort-session-retains-work-abort-operation-abandons-it", false, "opens on a session issued before the fixture"),
    (
        "delete-lost-result-and-pinned-reader-continuity",
        false,
        "opens on a head and a session issued before the fixture",
    ),
    ("disconnect-reboot-and-resume", false, "resume is the profile this device does not ship (§6.1)"),
    ("download-pin-survives-replace-and-delete", false, "opens on a head published before the fixture"),
    ("draft-begin-parts-finalize-and-paging", false, "drafts are a later DOS3 slice"),
    ("lost-result-then-query-operation", false, "opens on a session issued before the fixture"),
    ("replace-conflict-at-the-commit-lock", false, "opens on a head published before the fixture"),
    ("result-window-eviction-boundary", false, "opens on a committed result and 63 injected terminals"),
    ("set-metadata-compare-and-swap-and-lost-result", false, "opens on a head published before the fixture"),
    ("wrong-owner-cannot-advance-or-release-a-session", false, "opens on a session issued before the fixture"),
];

/// Which record channel an event's bytes belong to, or that it carries none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// A §2 control frame.
    Control,
    /// A §13 stream frame.
    Stream,
    /// An injected disconnect, reset, or crash cut: the event carries no record.
    Injected,
}

/// One transcript event.
#[derive(Debug, Clone)]
pub struct Event {
    /// `"client"` or `"device"`.
    pub actor: String,
    /// The principal scope's name.
    pub principal: String,
    /// The link kind's name.
    pub link: String,
    /// The connection generation.
    pub generation: u32,
    /// Which channel the record belongs to.
    pub channel: Channel,
    /// What the event proves.
    pub note: String,
    /// The record, empty for an injected event.
    pub record: Vec<u8>,
}

impl Event {
    /// True when the client sent this record.
    pub fn is_client(&self) -> bool {
        self.actor == "client"
    }
}

/// One checked-in transcript.
#[derive(Debug, Clone)]
pub struct Transcript {
    /// The fixture's stable name.
    pub name: String,
    /// What the flow proves.
    pub description: String,
    /// The ordered events.
    pub events: Vec<Event>,
}

impl Transcript {
    /// Whether the harness drives this transcript through the engine, and the reason if not.
    pub fn drive_note(&self) -> (bool, &'static str) {
        DRIVEN
            .iter()
            .find(|(name, _, _)| *name == self.name)
            .map(|(_, driven, reason)| (*driven, *reason))
            .unwrap_or((false, "not in the checked-in inventory"))
    }
}

/// The suite directory.
pub fn directory() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../specs/vectors/device-object-v2/transcripts")
}

/// Every checked-in transcript, in name order.
pub fn load() -> Vec<Transcript> {
    let mut paths: Vec<PathBuf> = fs::read_dir(directory())
        .expect("the transcript directory is checked in")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "json"))
        .collect();
    paths.sort();
    paths.iter().map(|path| parse(&fs::read_to_string(path).expect("a readable fixture"))).collect()
}

/// Parses one transcript file.
pub fn parse(text: &str) -> Transcript {
    let name = string_field(text, "name").expect("every transcript names itself");
    let description = string_field(text, "description").unwrap_or_default();
    let events = objects(text).into_iter().map(|object| parse_event(&object)).collect();
    Transcript { name, description, events }
}

fn parse_event(object: &str) -> Event {
    let channel = match string_field(object, "channel").unwrap_or_default().as_str() {
        "control" => Channel::Control,
        "stream" => Channel::Stream,
        _ => Channel::Injected,
    };
    Event {
        actor: string_field(object, "actor").unwrap_or_default(),
        principal: string_field(object, "principal").unwrap_or_default(),
        link: string_field(object, "link").unwrap_or_default(),
        generation: string_field(object, "connectionGeneration")
            .or_else(|| number_field(object, "connectionGeneration"))
            .and_then(|value| value.parse().ok())
            .unwrap_or(1),
        channel,
        note: string_field(object, "note").unwrap_or_default(),
        record: unhex(&string_field(object, "record").unwrap_or_default()),
    }
}

/// The objects of the `events` array, as raw text.
fn objects(text: &str) -> Vec<String> {
    let Some(start) = text.find("\"events\":") else { return Vec::new() };
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut current = String::new();
    let mut out = Vec::new();
    for &byte in &bytes[start..] {
        match byte {
            b'{' => {
                depth += 1;
                current.push('{');
            }
            b'}' if depth > 0 => {
                current.push('}');
                depth -= 1;
                if depth == 0 {
                    out.push(core::mem::take(&mut current));
                }
            }
            _ if depth > 0 => current.push(byte as char),
            _ => {}
        }
    }
    out
}

fn string_field(text: &str, key: &str) -> Option<String> {
    let needle = std::format!("\"{key}\": \"");
    let start = text.find(&needle)? + needle.len();
    let rest = &text[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn number_field(text: &str, key: &str) -> Option<String> {
    let needle = std::format!("\"{key}\": ");
    let start = text.find(&needle)? + needle.len();
    let rest = &text[start..];
    let end = rest.find(|character: char| !character.is_ascii_digit())?;
    Some(rest[..end].to_string())
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|index| u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).unwrap_or_default())
        .collect()
}
