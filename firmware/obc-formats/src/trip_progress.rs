//! The device's trip progress records (`obc-ble-interface-spec.md` §7.7). The Metadata singleton
//! holds them; `Ride_Archive_Metadata.md` has the bytes.

use crate::io::{put_u32, rd_u16, rd_u32};

/// The days a record can date. It is also the device's resident cap on a trip's days.
pub const MAX_DAYS: usize = 32;
/// The records the Metadata object holds.
pub const MAX_RECORDS: usize = 16;
pub const RECORD_LEN: usize = 96;

/// "No day" in the last-finished field.
const NO_DAY: u16 = u16::MAX;

/// The device's progress records, in write order.
pub type Records = heapless::Vec<TripProgress, MAX_RECORDS>;

/// A route object as the store holds it. A replace keeps the id and bumps the revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteVersion {
    pub id: u64,
    pub revision: u64,
}

/// The device's own progress through one trip. It is keyed on the trip key, so it survives a
/// re-upload of the same trip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TripProgress {
    pub key: u64,
    /// The day that contains the position.
    pub day: u16,
    /// That day's route when the record was written.
    pub day_route: RouteVersion,
    /// Metres into that day's route.
    pub metres: u32,
    /// The last finished day; `None` before the first Finish.
    pub last_finished: Option<u16>,
    /// The date each day was finished, in days since 1970-01-01; 0 = none. A finish without a
    /// trusted clock records no date.
    pub dates: [u16; MAX_DAYS],
}

impl TripProgress {
    pub fn encode(&self) -> [u8; RECORD_LEN] {
        let mut b = [0; RECORD_LEN];
        b[0..8].copy_from_slice(&self.key.to_le_bytes());
        b[8..16].copy_from_slice(&self.day_route.id.to_le_bytes());
        b[16..24].copy_from_slice(&self.day_route.revision.to_le_bytes());
        put_u32(&mut b, 24, self.metres);
        b[28..30].copy_from_slice(&self.day.to_le_bytes());
        b[30..32].copy_from_slice(&self.last_finished.unwrap_or(NO_DAY).to_le_bytes());
        for (out, date) in b[32..].as_chunks_mut::<2>().0.iter_mut().zip(self.dates) {
            *out = date.to_le_bytes();
        }
        b
    }

    /// `None` for key 0, which means "no trip".
    pub fn decode(b: &[u8; RECORD_LEN]) -> Option<TripProgress> {
        let key = u64::from_le_bytes(b[0..8].try_into().ok()?);
        let last = rd_u16(b, 30);
        let mut dates = [0; MAX_DAYS];
        for (date, bytes) in dates.iter_mut().zip(b[32..].as_chunks::<2>().0) {
            *date = u16::from_le_bytes(*bytes);
        }
        (key != 0).then(|| TripProgress {
            key,
            day: rd_u16(b, 28),
            day_route: RouteVersion {
                id: u64::from_le_bytes(b[8..16].try_into().unwrap()),
                revision: u64::from_le_bytes(b[16..24].try_into().unwrap()),
            },
            metres: rd_u32(b, 24),
            last_finished: (last != NO_DAY).then_some(last),
            dates,
        })
    }
}

/// Write `new` into `records` by the bound rules: drop each record whose key no stored trip holds,
/// then the first record when the list is full; `new` goes to the end and replaces its key's record.
pub fn record(records: &mut Records, new: TripProgress, stored: impl Fn(u64) -> bool) {
    records.retain(|r| r.key != new.key && stored(r.key));
    if records.is_full() {
        records.remove(0);
    }
    let _ = records.push(new);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(key: u64) -> TripProgress {
        let mut dates = [0; MAX_DAYS];
        dates[1] = 20_361;
        TripProgress {
            key,
            day: 2,
            day_route: RouteVersion { id: 30, revision: 4 },
            metres: 20_000,
            last_finished: Some(1),
            dates,
        }
    }

    #[test]
    fn a_record_round_trips() {
        let p = at(0xA1);
        assert_eq!(TripProgress::decode(&p.encode()), Some(p.clone()));
        let none = TripProgress { last_finished: None, ..p };
        assert_eq!(TripProgress::decode(&none.encode()), Some(none));
        assert_eq!(TripProgress::decode(&at(0).encode()), None, "key 0 is no trip");
    }

    #[test]
    fn a_write_moves_its_record_to_the_end_and_drops_the_oldest_when_full() {
        let mut records = Records::new();
        for key in 1..=MAX_RECORDS as u64 {
            record(&mut records, at(key), |_| true);
        }
        record(&mut records, at(3), |_| true);
        assert_eq!(records.last().map(|r| r.key), Some(3));
        assert_eq!(records.len(), MAX_RECORDS);
        record(&mut records, at(99), |_| true);
        assert_eq!(records.first().map(|r| r.key), Some(2), "the first record goes");
        assert_eq!(records.len(), MAX_RECORDS);
    }

    #[test]
    fn a_write_drops_the_records_of_deleted_trips_first() {
        let mut records = Records::new();
        for key in 1..=MAX_RECORDS as u64 {
            record(&mut records, at(key), |_| true);
        }
        record(&mut records, at(99), |key| key != 5);
        assert!(records.iter().all(|r| r.key != 5));
        assert_eq!(records.first().map(|r| r.key), Some(1), "no live record goes");
    }
}
