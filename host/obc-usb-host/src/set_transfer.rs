//! Sending a **volume set** to a device (issue #1039, `OBCA_Spec.md` §5).
//!
//! An assembled set is a directory: `MS{id}S{kk}.OBM` for each shard and `MS{id}.OBS` for the
//! manifest. Getting it onto a device is not "upload a directory" — §5.4 makes the order load-bearing
//! (**the manifest is written last**, so a half-uploaded set has no manifest and is invisible as a
//! map), and §5.3 puts an obligation on the *host* the device is allowed to skip:
//!
//! > A device MAY defer the SHA-256 check […] and a **host** writing a set MUST verify every digest
//! > before the manifest is written.
//!
//! So this module is two halves, and the split is deliberate:
//!
//! - [`plan`] reads the directory, parses the manifest, and **proves the set** — every shard present,
//!   at the recorded byte length, with the recorded SHA-256 — before a single byte is offered to a
//!   device. It also computes each file's CRC-32, because the transfer descriptor has to announce one
//!   before the first byte moves (spec §4.2). Nothing here talks to a device.
//! - [`send`] walks that plan over a [`SetLink`]: shards in index order, manifest last, one whole-file
//!   CRC per transfer. It knows nothing about USB. That is what lets the same driver run against a
//!   real cable and against the device's own receive logic in a test, which is the round trip #1039's
//!   acceptance asks for.
//!
//! ## Resume
//!
//! Per **file**, yes, and it costs nothing: shards are independent files (§5.4), so a shard whose
//! whole-object CRC came back wrong is re-sent on its own — [`Options::retries`] — while the
//! gigabytes already committed beside it stand. That is the granularity the protocol's
//! restart-don't-resume rule (spec §1 principle 4) leaves available, and it is the useful one: the
//! unit that fails is the unit that repeats.
//!
//! Across a **disconnect**, no, and deliberately not faked: the device deletes the whole set when its
//! link drops mid-transfer, because a set with no manifest is unmountable anyway and leaving
//! gigabytes of it would strand the card. Resuming across a reconnect would need the device to say
//! which shards of which set it already holds — a new query, not a new descriptor — and that is
//! written down as a follow-up rather than approximated here. A host that guessed would silently
//! build a set out of one upload's shards and another's manifest.

use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use obc_ble::{Crc32, ObjectType, Op, SetPart, TransferControl, TransferResult, TransferStatus};
use obc_formats::obcs;

/// Bytes read per pass when hashing / checksumming a shard. A shard is up to `4 GiB − 1`, so this is
/// never held whole.
const READ_CHUNK: usize = 1024 * 1024;

/// Which file of the set a planned transfer is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// Shard `index` of the set — an ordinary OBCM file (`OBCA_Spec.md` §5.1).
    Shard(SetPart),
    /// The OBCS manifest. Always last (§5.4).
    Manifest,
}

impl Part {
    /// The object type this part rides as.
    pub fn object_type(self) -> ObjectType {
        match self {
            Part::Shard(_) => ObjectType::MapShard,
            Part::Manifest => ObjectType::MapSet,
        }
    }

    /// The descriptor's `object_id` field: a packed [`SetPart`] for a shard, and `0xFFFF` ("new")
    /// for the manifest, which is new-only exactly as a single map is.
    pub fn object_id(self) -> u16 {
        match self {
            Part::Shard(part) => part.encode(),
            Part::Manifest => TransferControl::NEW_OBJECT_ID,
        }
    }
}

/// One file of the set, ready to send.
#[derive(Debug, Clone)]
pub struct PlannedFile {
    /// The derived 8.3 name the device will store it under (`OBCA_Spec.md` §5.2).
    pub filename: String,
    /// Where the bytes are on this host.
    pub path: PathBuf,
    pub part: Part,
    pub len: u64,
    /// The whole-file CRC-32/IEEE the descriptor announces (spec §4.2, §6).
    pub crc32: u32,
}

impl PlannedFile {
    /// The 12-byte transfer descriptor for this file.
    pub fn descriptor(&self) -> TransferControl {
        TransferControl {
            op: Op::Upload,
            ty: self.part.object_type(),
            object_id: self.part.object_id(),
            total_len: self.len as u32,
            crc32: self.crc32,
        }
    }
}

/// A verified set, in the order §5.4 requires it be sent: every shard by index, then the manifest.
#[derive(Debug, Clone)]
pub struct SetPlan {
    /// The card id the assembler used — the `{id}` of the derived filenames on *this host*. The
    /// device mints its **own** id when it receives the set (filenames are derived, not stored, so
    /// nothing carries this across the wire).
    pub card_id: u16,
    pub shard_count: usize,
    /// The set's display name from the manifest, if it carries a printable one.
    pub name: Option<String>,
    /// Shards in index order, then the manifest. Never empty, and the last element is always the
    /// manifest — that invariant is the whole point of the type.
    pub files: Vec<PlannedFile>,
}

impl SetPlan {
    /// The set's total bytes on the wire.
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.len).sum()
    }

    /// The manifest — the last file, always.
    pub fn manifest(&self) -> &PlannedFile {
        self.files.last().expect("a plan always ends with its manifest")
    }
}

/// Why a directory did not yield a sendable set. Every one of these is a refusal to *start*: a set
/// that cannot be proven here is one a device would refuse at the end, after the whole transfer.
#[derive(Debug)]
pub enum PlanError {
    Io(PathBuf, io::Error),
    /// The manifest did not parse or validate against `OBCA_Spec.md` §5.3.
    Manifest(obcs::ManifestError),
    /// A shard the manifest names is not the size it records.
    ShardSize {
        filename: String,
        expected: u32,
        found: u64,
    },
    /// A shard's SHA-256 is not the digest the manifest records — §5.3's host obligation, and the
    /// one check that catches a corrupted download before it becomes a corrupted map.
    ShardDigest {
        filename: String,
    },
    /// The card id is outside the range the derived 8.3 names can express.
    CardId(u16),
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanError::Io(path, e) => write!(f, "{}: {e}", path.display()),
            PlanError::Manifest(e) => write!(f, "the set manifest is not valid (OBCA §5.3): {e:?}"),
            PlanError::ShardSize { filename, expected, found } => {
                write!(f, "{filename} is {found} bytes; the manifest records {expected}")
            }
            PlanError::ShardDigest { filename } => {
                write!(f, "{filename} does not match the SHA-256 the manifest records")
            }
            PlanError::CardId(id) => write!(f, "card id {id} has no derived 8.3 filename (OBCA §5.2 caps it at 999)"),
        }
    }
}

impl std::error::Error for PlanError {}

/// Read and **prove** the set with card id `card_id` in `dir`, returning it in send order.
///
/// Reads every byte of every shard twice-over-once (one pass computing both SHA-256 and CRC-32), so
/// the cost is one linear read of the set. That is minutes for a DACH-shaped set and it is the right
/// trade: §5.3 requires the digest check of a host, the CRC has to be announced before the transfer
/// anyway, and the alternative is discovering a bad download after uploading it.
pub fn plan(dir: &Path, card_id: u16) -> Result<SetPlan, PlanError> {
    let manifest_name = obcs::manifest_name(card_id).ok_or(PlanError::CardId(card_id))?;
    let manifest_path = dir.join(manifest_name.as_str());
    let manifest_bytes = std::fs::read(&manifest_path).map_err(|e| PlanError::Io(manifest_path.clone(), e))?;
    let manifest = obcs::parse(&manifest_bytes).map_err(PlanError::Manifest)?;

    let mut files = Vec::with_capacity(manifest.shard_count() + 1);
    for (index, shard) in manifest.shards().iter().enumerate() {
        let derived = obcs::shard_name(card_id, index).ok_or(PlanError::CardId(card_id))?;
        let filename = derived.as_str().to_string();
        let path = dir.join(&filename);
        let (len, crc32, digest) = fingerprint(&path)?;
        if len != shard.bytes as u64 {
            return Err(PlanError::ShardSize { filename, expected: shard.bytes, found: len });
        }
        let recorded =
            obcs::shard_digest(&manifest_bytes, index).ok_or(PlanError::Manifest(obcs::ManifestError::Layout))?;
        if &digest != recorded {
            return Err(PlanError::ShardDigest { filename });
        }
        // `shard_count` is `1..=32` and every index is `< 32`, so the part is always well formed —
        // the same range `obcs::parse` already enforced.
        let part = SetPart { shard_count: manifest.shard_count() as u8, index: index as u8 };
        files.push(PlannedFile { filename, path, part: Part::Shard(part), len, crc32 });
    }

    // The manifest goes last, and it goes last *here* as well as on the wire: a plan whose order is
    // built by construction cannot be sent in the wrong one by a caller who iterates it.
    let mut crc = Crc32::new();
    crc.update(&manifest_bytes);
    files.push(PlannedFile {
        filename: manifest_name.as_str().to_string(),
        path: manifest_path,
        part: Part::Manifest,
        len: manifest_bytes.len() as u64,
        crc32: crc.finalize(),
    });

    Ok(SetPlan { card_id, shard_count: manifest.shard_count(), name: manifest.name().map(str::to_string), files })
}

/// One linear pass over a file yielding `(length, CRC-32, SHA-256)` — the three facts a planned
/// transfer needs, none of which is worth a second read of a gigabyte.
fn fingerprint(path: &Path) -> Result<(u64, u32, [u8; 32]), PlanError> {
    use sha2::{Digest, Sha256};

    let mut file = File::open(path).map_err(|e| PlanError::Io(path.to_path_buf(), e))?;
    let mut crc = Crc32::new();
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; READ_CHUNK];
    let mut len = 0u64;
    loop {
        let n = file.read(&mut buf).map_err(|e| PlanError::Io(path.to_path_buf(), e))?;
        if n == 0 {
            break;
        }
        crc.update(&buf[..n]);
        hasher.update(&buf[..n]);
        len += n as u64;
    }
    Ok((len, crc.finalize(), hasher.finalize().into()))
}

/// The byte pipe a set is sent over: write one descriptor, stream that file's bytes, read the
/// device's terminal `transferResult`.
///
/// One method rather than three because the protocol has exactly one shape here — §4.1's
/// one-transfer-at-a-time rule means a descriptor, its bytes, and its result are indivisible, and an
/// implementation that could interleave them would be implementing a protocol this is not.
pub trait SetLink {
    /// Announce `desc`, stream `bytes` (exactly `desc.total_len` of them), and return what the
    /// device answered.
    fn send_object(&mut self, desc: &TransferControl, bytes: &mut dyn Read) -> Result<TransferResult, LinkError>;
}

/// A transport failure — the link is gone, or it misbehaved. Distinct from a *device* answer, which
/// is a [`TransferResult`] and a perfectly ordinary thing to receive.
#[derive(Debug)]
pub struct LinkError(pub String);

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LinkError {}

/// How to drive a send.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Extra attempts per file after the first. A shard is an independent file, so re-sending one
    /// costs that shard rather than the set — the only resume granularity the protocol offers, and
    /// the one that matches what actually fails.
    pub retries: u8,
}

impl Default for Options {
    fn default() -> Self {
        Options { retries: 1 }
    }
}

/// Where a send got to, reported per file so a caller can render a set-wide bar.
#[derive(Debug, Clone, Copy)]
pub struct Progress {
    /// Index into [`SetPlan::files`].
    pub file: usize,
    pub files_total: usize,
    /// Bytes of the whole set that have committed.
    pub sent: u64,
    pub total: u64,
}

/// Why a send stopped short.
#[derive(Debug)]
pub enum SendError {
    Io(PathBuf, io::Error),
    Link(LinkError),
    /// The device refused a file. `filename` names which, so the message can say "shard 3 of 8" and
    /// not "the upload".
    Refused {
        filename: String,
        status: TransferStatus,
    },
    /// A shard's `transferResult` did not echo the part that was sent — spec §4.1's correlation
    /// rule for a set. The transfer is stopped rather than counted, because a result that names a
    /// different file cannot be read as "this file committed", and going on would build a manifest
    /// over a set whose contents the device and the host disagree about.
    Uncorrelated {
        filename: String,
        expected: u16,
        echoed: u16,
    },
}

impl fmt::Display for SendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SendError::Io(path, e) => write!(f, "{}: {e}", path.display()),
            SendError::Link(e) => write!(f, "the link failed: {e}"),
            SendError::Refused { filename, status } => write!(f, "the device refused {filename}: {status:?}"),
            SendError::Uncorrelated { filename, expected, echoed } => {
                write!(f, "the device answered {filename} with part {echoed:#06X}, not the {expected:#06X} it was sent")
            }
        }
    }
}

impl std::error::Error for SendError {}

/// What a completed set upload became on the device.
#[derive(Debug, Clone, Copy)]
pub struct Sent {
    /// The set id the **device** minted, as reported in the manifest's `transferResult`. Not the
    /// host's `card_id`: filenames are derived from the id the storing device chose (§5.2).
    pub device_set_id: u16,
    pub bytes: u64,
}

/// Send a planned set: shards in index order, manifest last.
///
/// The order is not enforced by a check here because it cannot be got wrong — [`plan`] builds the
/// vector in it. What *is* enforced is that a refused file stops the run: a set whose shard 3 was
/// rejected must not go on to send a manifest naming it, and the device would refuse that manifest
/// anyway (`OBCA_Spec.md` §5.4, enforced device-side), so continuing would only waste the rider's
/// remaining shards.
pub fn send(
    link: &mut dyn SetLink,
    plan: &SetPlan,
    options: Options,
    progress: &mut dyn FnMut(Progress),
) -> Result<Sent, SendError> {
    let total = plan.total_bytes();
    let files_total = plan.files.len();
    let mut sent = 0u64;
    let mut device_set_id = 0u16;

    for (index, planned) in plan.files.iter().enumerate() {
        progress(Progress { file: index, files_total, sent, total });
        let mut attempt = 0u8;
        let result = loop {
            let mut file = File::open(&planned.path).map_err(|e| SendError::Io(planned.path.clone(), e))?;
            let outcome = link.send_object(&planned.descriptor(), &mut file).map_err(SendError::Link)?;
            // A CRC mismatch is the one refusal worth repeating: it says the bytes arrived wrong,
            // not that the device declined the object. Everything else — a full card, a manifest
            // out of order, an unknown part — would answer identically however many times it is
            // asked, so retrying it only costs time.
            if outcome.status == TransferStatus::CrcMismatch && attempt < options.retries {
                attempt += 1;
                continue;
            }
            break outcome;
        };
        if result.status != TransferStatus::Committed {
            return Err(SendError::Refused { filename: planned.filename.clone(), status: result.status });
        }
        // §4.1 rule 4: a shard's result echoes its **part**, and that is what a host correlates its
        // slot against. Checking it is the difference between "a file committed" and "*this* file
        // committed" — and the only thing standing between a device that answers out of step and a
        // manifest written over a set neither side agrees on.
        if let Part::Shard(part) = planned.part {
            if result.object_id != part.encode() {
                return Err(SendError::Uncorrelated {
                    filename: planned.filename.clone(),
                    expected: part.encode(),
                    echoed: result.object_id,
                });
            }
        } else {
            // The manifest's result carries the device-assigned set id — the one moment a set's
            // identity crosses the wire, and the answer to "what did my upload become".
            device_set_id = result.object_id;
        }
        sent += planned.len;
    }
    progress(Progress { file: files_total, files_total, sent, total });
    Ok(Sent { device_set_id, bytes: sent })
}
