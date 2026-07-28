//! The bulk plane by file path: a map goes disk → endpoint without touching the webview.
//!
//! ## Why this exists at all
//!
//! #894 is explicit about the split — control plane over IPC, bulk plane native — and the number
//! behind it is that `docs/content/software/architecture.md`'s big case is a **300 MB** map. On the
//! hosted tier those bytes have nowhere else to be: the tab fetched them, so the tab streams them.
//! Here the file is already on the same disk as the process that owns the endpoint, and routing it
//! through the webview would mean copying every byte into JavaScript and straight back out again
//! for no reason at all.
//!
//! ## What this does *not* know
//!
//! Not one byte of protocol. The transfer descriptor — op, type, object id, length, CRC — is
//! encoded by the TS client and written on the *control* plane before this runs, and the device's
//! verdict is read there afterwards. This function's entire contract is "put the contents of this
//! file on that endpoint, in order, and tell me how far you got". That is the line #902 drew and
//! the reason there is one protocol implementation rather than two.
//!
//! The one thing it does compute is the object's CRC-32 ([`digest`]) — because the descriptor has
//! to announce it *before* the first byte moves (spec §4.2) and the alternative is reading 300 MB
//! through IPC just to checksum it. It is not a second implementation either: it is
//! [`obc_ble::Crc32`], the same code the device runs and the one `lib/usb/crc32.ts` was ported
//! from, linked in as a library.
//!
//! ## Backpressure and memory
//!
//! A fixed number of transfers of a fixed size are kept in flight, and a new chunk is read off the
//! disk only when one completes. So the file is read at exactly the rate the device drains the
//! endpoint (high hundreds of KB/s — the SD card, not USB) and the process's peak is
//! [`IN_FLIGHT`] × [`CHUNK`], **flat**, whether the object is 1 MB or 1 GB.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use nusb::transfer::{Buffer, Completion};
use serde::Serialize;
use tauri::ipc::Channel;
use tokio::io::AsyncReadExt;

use super::link::{OpenLink, PipeFault, PROGRESS_INTERVAL};

/// Bytes per bulk OUT transfer. 128 packets on a high-speed endpoint.
const CHUNK: usize = 64 * 1024;

/// Transfers queued on the endpoint at once.
///
/// This is the whole backpressure story, so it is worth being concrete: 4 × 64 KB is 256 KB of
/// buffered data, about half a second of a device draining at 500 KB/s. Enough that the endpoint
/// never idles between transfers, small enough that a cancel takes effect within one chunk and the
/// resident cost is a rounding error next to the artifact.
const IN_FLIGHT: usize = 4;

/// Bytes read per pass when fingerprinting a file.
const DIGEST_CHUNK: usize = 1024 * 1024;

/// What a file is, to a transfer descriptor: how long, and its whole-object CRC-32.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDigest {
    pub len: u64,
    pub crc32: u32,
}

/// One progress report from a running file send.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendProgress {
    pub sent: u64,
    pub total: u64,
}

/// Length and CRC-32 of a file, without holding it.
///
/// Blocking on purpose — every caller runs it on the blocking pool. A 300 MB file is a couple of
/// seconds of disk, and this must not sit on the async runtime's worker while it happens.
pub fn digest(path: &Path) -> Result<FileDigest, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut crc = obc_ble::Crc32::new();
    let mut buf = vec![0u8; DIGEST_CHUNK];
    let mut len = 0u64;
    loop {
        let n = file.read(&mut buf).map_err(|e| format!("{}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        crc.update(&buf[..n]);
        len += n as u64;
    }
    Ok(FileDigest { len, crc32: crc.finalize() })
}

/// Stream `path` into the link's bulk OUT endpoint, reporting progress on `on_progress`.
///
/// Returns the number of bytes the device accepted, which the client checks against the length its
/// descriptor announced — a short send is a protocol error there, not something to paper over here.
///
/// Cancelled by [`super::link::Pipe::cancel`] on the bulk OUT direction, which is what both the
/// caller's `AbortSignal` and the client's "the device already rejected this transfer" check reach
/// through.
pub async fn send(link: Arc<OpenLink>, path: PathBuf, on_progress: Channel<SendProgress>) -> Result<u64, PipeFault> {
    let mut file =
        tokio::fs::File::open(&path).await.map_err(|e| PipeFault::device(format!("{}: {e}", path.display())))?;
    let total = file.metadata().await.map_err(|e| PipeFault::device(format!("{}: {e}", path.display())))?.len();

    let (ep_out, mut cancel) = link.bulk.out_for_streaming();
    let mut ep = ep_out.lock().await;
    cancel.borrow_and_update();

    let mut queued: u64 = 0;
    let mut sent: u64 = 0;
    let mut last_report = Instant::now();
    let _ = on_progress.send(SendProgress { sent: 0, total });

    while sent < total {
        // Top the queue up first, so the endpoint never waits on the disk.
        while queued < total && ep.pending() < IN_FLIGHT {
            let n = ((total - queued) as usize).min(CHUNK);
            let mut buf = Buffer::new(n);
            // `extend_fill` hands back the bytes it just added, so the file is read straight into
            // the transfer buffer — no staging Vec and no second copy per chunk.
            let slot = buf.extend_fill(n, 0);
            file.read_exact(slot).await.map_err(|e| {
                PipeFault::device(format!("{} could not be read at offset {queued}: {e}", path.display()))
            })?;
            ep.submit(buf);
            queued += n as u64;
        }
        if ep.pending() == 0 {
            // The file got shorter than its metadata said. Report what actually moved and let the
            // client's length check turn it into an error the user can read.
            break;
        }

        let mut done: Option<Completion> = None;
        tokio::select! {
            biased;
            completion = ep.next_complete() => done = Some(completion),
            _ = cancel.changed() => {}
        }
        let Some(completion) = done else {
            ep.cancel_all();
            while ep.pending() > 0 {
                let _ = ep.next_complete().await;
            }
            return Err(PipeFault::aborted("The transfer was cancelled."));
        };
        let submitted = completion.buffer.len();
        completion.status.map_err(|e| PipeFault::from_transfer("transfer", e))?;
        sent += completion.actual_len as u64;
        if completion.actual_len != submitted {
            return Err(PipeFault::device(format!(
                "the device took {} of {submitted} bytes at offset {sent}.",
                completion.actual_len
            )));
        }
        // Throttled, because this is the frontend's only chance to notice that the device rejected
        // the descriptor mid-stream — it re-checks its status mailbox on every report. Often enough
        // to stop a doomed 300 MB push in well under a second, rare enough not to flood the IPC.
        if last_report.elapsed() >= PROGRESS_INTERVAL || sent == total {
            last_report = Instant::now();
            let _ = on_progress.send(SendProgress { sent, total });
        }
    }
    Ok(sent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_digest_is_the_crc_the_device_will_compute() {
        let dir = std::env::temp_dir().join(format!("obc-usb-digest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("check.bin");
        // The CRC-32/IEEE check value, which is also what `specs/vectors/manifest.json` and
        // `lib/usb/crc32.test.ts` pin: crc32("123456789") == 0xCBF43926. Same constant on both
        // sides of the wire is the whole claim.
        std::fs::write(&path, b"123456789").expect("write");
        let d = digest(&path).expect("digest");
        assert_eq!(d.len, 9);
        assert_eq!(d.crc32, 0xCBF4_3926);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_digest_of_an_empty_file_is_the_empty_crc() {
        let dir = std::env::temp_dir().join(format!("obc-usb-digest-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("empty.bin");
        std::fs::write(&path, b"").expect("write");
        let d = digest(&path).expect("digest");
        assert_eq!((d.len, d.crc32), (0, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn chunking_never_reads_past_the_end() {
        // The arithmetic the send loop uses, checked without a device: every chunk is at most
        // CHUNK, they tile the file exactly, and the last one is the remainder.
        for total in [0u64, 1, CHUNK as u64 - 1, CHUNK as u64, CHUNK as u64 + 1, 300 * 1024 * 1024] {
            let mut queued = 0u64;
            let mut chunks = 0u64;
            while queued < total {
                let n = ((total - queued) as usize).min(CHUNK);
                assert!(n > 0 && n <= CHUNK);
                queued += n as u64;
                chunks += 1;
            }
            assert_eq!(queued, total);
            assert_eq!(chunks, total.div_ceil(CHUNK as u64));
        }
    }
}
