//! Durable ride bytes and recovery rules. Storage adapters execute a pending write and acknowledge
//! it only after success. Until then, its bytes, checksum and continuation remain unchanged.

use obc_crc::Crc32;
use obc_formats::ride::{decode_footer, Name, FOOTER_LEN, SAMPLE_LEN};
use obc_ports::TrackPoint;
use obc_route::RideStats;

use super::{continuation, RideContinuation, RideDamage};

pub const RESERVE_BYTES: u64 = 32 * 1024 * 1024;
pub const DELTA_SAMPLES: usize = 16;
pub const DELTA_BYTES: usize = DELTA_SAMPLES * SAMPLE_LEN + FOOTER_LEN;

/// The board places these bytes in static memory. A storage request borrows them until its reply.
pub struct WriteBuffers {
    pub append: [u8; DELTA_BYTES],
    pub resume: [u8; continuation::RIDE_RESUME_LEN],
}

impl WriteBuffers {
    pub const ZERO: Self = Self { append: [0; DELTA_BYTES], resume: [0; continuation::RIDE_RESUME_LEN] };
}

pub struct Recovery<'a> {
    pub payload_len: u64,
    pub payload_crc: u32,
    pub checkpoint_sequence: u64,
    pub resume: &'a [u8; continuation::RIDE_RESUME_LEN],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppendResult {
    Accepted,
    NeedsCheckpoint,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Write {
    Journal { append_len: usize, payload_crc: u32 },
    Publish { name: Name, payload_len: u64, payload_crc: u32 },
    Remove,
}

#[derive(Clone, Copy)]
struct Live {
    name: Name,
    session: Option<u32>,
    points: u32,
    first: Option<u32>,
    last: Option<u32>,
    start: Option<u32>,
    same_boot: bool,
    rebase: Option<(u32, u32)>,
    continuation: RideContinuation,
    crc: Crc32,
    len: usize,
    pending: bool,
}

impl Live {
    fn stable_start(&self, stats: &RideStats) -> Option<u32> {
        self.start.or_else(|| {
            (self.same_boot && stats.clock_trusted).then(|| {
                stats
                    .unix_at_anchor
                    .wrapping_sub(stats.anchor_ms.wrapping_sub(self.first.unwrap_or(stats.anchor_ms)) / 1000)
            })
        })
    }
}

#[derive(Clone, Copy)]
enum CloseStep {
    Repair,
    Journal,
    Publish,
}

#[derive(Clone, Copy)]
enum State {
    Idle,
    Live(Live),
    Closing(Live, CloseStep),
    Damaged(RideDamage),
    Discarding,
}

pub struct RideWriter {
    state: State,
}

impl RideWriter {
    pub const fn idle() -> Self {
        Self { state: State::Idle }
    }

    pub fn start(name: &str, session: Option<u32>) -> Self {
        Self {
            state: State::Live(Live {
                name: Name::new(name),
                session,
                points: 0,
                first: None,
                last: None,
                start: None,
                same_boot: true,
                rebase: None,
                continuation: RideContinuation::default(),
                crc: Crc32::new(),
                len: 0,
                pending: false,
            }),
        }
    }

    pub const fn damaged(damage: RideDamage) -> Self {
        Self { state: State::Damaged(damage) }
    }

    /// Read failures remain storage failures; only successfully read, invalid bytes are damage.
    pub fn recover<E>(
        name: &str,
        recovered: Recovery<'_>,
        mut read: impl FnMut(u64, &mut [u8]) -> Result<usize, E>,
    ) -> Result<Self, E> {
        let total = recovered.payload_len;
        let mut writer = Self::start(name, None);
        let State::Live(mut live) = writer.state else { unreachable!() };
        live.same_boot = false;
        live.crc = Crc32::from_checksum(recovered.payload_crc);
        if total >= FOOTER_LEN as u64 && (total - FOOTER_LEN as u64).is_multiple_of(SAMPLE_LEN as u64) {
            let mut bytes = [0; FOOTER_LEN];
            let at = total - FOOTER_LEN as u64;
            if read(at, &mut bytes)? == FOOTER_LEN {
                if let Ok(footer) = decode_footer(&bytes) {
                    if u64::from(footer.point_count) == at / SAMPLE_LEN as u64 {
                        live.name = Name::new(footer.name());
                        live.points = footer.point_count;
                        writer.state = State::Closing(live, CloseStep::Publish);
                        return Ok(writer);
                    }
                }
            }
        }
        if !total.is_multiple_of(SAMPLE_LEN as u64) || total > RESERVE_BYTES - FOOTER_LEN as u64 {
            return Ok(Self::damaged(RideDamage::Payload));
        }
        if total != 0 {
            let mut sample = [0; SAMPLE_LEN];
            if read(0, &mut sample)? != SAMPLE_LEN {
                return Ok(Self::damaged(RideDamage::Payload));
            }
            live.first = Some(obc_formats::track::decode_record(&sample).t_ms);
            if read(total - SAMPLE_LEN as u64, &mut sample)? != SAMPLE_LEN {
                return Ok(Self::damaged(RideDamage::Payload));
            }
            live.last = Some(obc_formats::track::decode_record(&sample).t_ms);
        }
        let decoded =
            if total == 0 && recovered.checkpoint_sequence == 0 && recovered.resume.iter().all(|byte| *byte == 0) {
                Some((RideContinuation::default(), None))
            } else {
                continuation::decode(recovered.resume)
            };
        let Some((continuation, start)) = decoded else { return Ok(Self::damaged(RideDamage::Metadata)) };
        live.points = (total / SAMPLE_LEN as u64) as u32;
        live.continuation = continuation;
        live.start = start;
        writer.state = State::Live(live);
        Ok(writer)
    }

    pub fn is_idle(&self) -> bool {
        matches!(self.state, State::Idle)
    }

    pub fn is_closing(&self) -> bool {
        matches!(self.state, State::Closing(..))
    }

    pub fn session(&self) -> Option<u32> {
        match self.state {
            State::Live(live) => live.session,
            _ => None,
        }
    }

    pub fn recovered_continuation(&self) -> Option<RideContinuation> {
        match self.state {
            State::Live(Live { session: None, continuation, .. }) => Some(continuation),
            _ => None,
        }
    }

    pub fn recovery_damage(&self) -> Option<RideDamage> {
        match self.state {
            State::Damaged(damage) => Some(damage),
            _ => None,
        }
    }

    pub fn attach(&mut self, session: u32, name: &str, now_ms: u32) -> bool {
        let State::Live(live) = &mut self.state else { return false };
        if live.session.is_none() {
            live.session = Some(session);
            live.rebase = live.last.map(|last| (now_ms, last));
            if live.name.as_str().is_empty() {
                live.name = Name::new(name);
            }
        }
        live.session == Some(session)
    }

    /// Admission is atomic: a refusal cannot change the accepted sample or observation boundary.
    pub fn append(
        &mut self,
        buffers: &mut WriteBuffers,
        points: &[TrackPoint],
        continuation: RideContinuation,
    ) -> AppendResult {
        let State::Live(live) = &mut self.state else { return AppendResult::Failed };
        if live.session.is_none() || points.len() > DELTA_SAMPLES {
            return AppendResult::Failed;
        }
        let Some(count) = live.points.checked_add(points.len() as u32) else { return AppendResult::Failed };
        if live.pending || live.len + points.len() * SAMPLE_LEN + FOOTER_LEN > DELTA_BYTES {
            return AppendResult::NeedsCheckpoint;
        }
        if u64::from(count) * SAMPLE_LEN as u64 + FOOTER_LEN as u64 > RESERVE_BYTES {
            return AppendResult::Failed;
        }
        for point in points {
            let mut point = *point;
            if let Some((source, logical)) = live.rebase {
                point.t_ms = logical.wrapping_add(point.t_ms.wrapping_sub(source));
            }
            let bytes = obc_formats::track::encode_record(&point);
            buffers.append[live.len..live.len + SAMPLE_LEN].copy_from_slice(&bytes);
            live.len += SAMPLE_LEN;
            live.crc.update(&bytes);
            live.first.get_or_insert(point.t_ms);
            live.last = Some(point.t_ms);
        }
        live.points = count;
        live.continuation = continuation;
        AppendResult::Accepted
    }

    pub fn checkpoint(
        &mut self,
        buffers: &mut WriteBuffers,
        stats: &RideStats,
        boundary: Option<RideContinuation>,
    ) -> bool {
        let State::Live(live) = &mut self.state else { return false };
        if !live.pending {
            live.start = live.stable_start(stats);
            live.continuation = boundary.unwrap_or(live.continuation);
            buffers.resume = continuation::encode(live.continuation, live.start);
            live.pending = true;
        }
        true
    }

    pub fn finish(&mut self, buffers: &mut WriteBuffers, stats: &RideStats) {
        let State::Live(mut live) = self.state else { return };
        let start = live.stable_start(stats);
        let mut footer_stats = *stats;
        footer_stats.unix_at_anchor = start.unwrap_or(0);
        footer_stats.anchor_ms = live.first.unwrap_or(0);
        footer_stats.clock_trusted = start.is_some();
        let footer = obc_route::encode_summary_footer(live.name.as_str(), &footer_stats, live.points, live.first);
        buffers.append[live.len..live.len + FOOTER_LEN].copy_from_slice(&footer);
        let step = if live.pending {
            // The staged footer sits outside the failed append until that exact checkpoint succeeds.
            CloseStep::Repair
        } else {
            live.start = start;
            buffers.resume = continuation::encode(live.continuation, start);
            live.len += FOOTER_LEN;
            live.crc.update(&footer);
            CloseStep::Journal
        };
        self.state = State::Closing(live, step);
    }

    pub fn discard(&mut self) {
        if !self.is_idle() {
            self.state = State::Discarding;
        }
    }

    pub fn pending_write(&self) -> Option<Write> {
        match self.state {
            State::Live(live) if live.pending => {
                Some(Write::Journal { append_len: live.len, payload_crc: live.crc.finalize() })
            }
            State::Closing(live, CloseStep::Repair | CloseStep::Journal) => {
                Some(Write::Journal { append_len: live.len, payload_crc: live.crc.finalize() })
            }
            State::Closing(live, CloseStep::Publish) => Some(Write::Publish {
                name: live.name,
                payload_len: u64::from(live.points) * SAMPLE_LEN as u64 + FOOTER_LEN as u64,
                payload_crc: live.crc.finalize(),
            }),
            State::Discarding => Some(Write::Remove),
            _ => None,
        }
    }

    /// Call exactly once for a successful pending write. Failure leaves the writer untouched.
    pub fn acknowledge(&mut self, buffers: &mut WriteBuffers) {
        match self.state {
            State::Live(mut live) if live.pending => {
                live.len = 0;
                live.pending = false;
                self.state = State::Live(live);
            }
            State::Closing(mut live, CloseStep::Repair) => {
                buffers.append.copy_within(live.len..live.len + FOOTER_LEN, 0);
                live.len = FOOTER_LEN;
                live.crc.update(&buffers.append[..FOOTER_LEN]);
                self.state = State::Closing(live, CloseStep::Journal);
            }
            State::Closing(live, CloseStep::Journal) => self.state = State::Closing(live, CloseStep::Publish),
            State::Closing(_, CloseStep::Publish) | State::Discarding => self.state = State::Idle,
            _ => {}
        }
    }
}
