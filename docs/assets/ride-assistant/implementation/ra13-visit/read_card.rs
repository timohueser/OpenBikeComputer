//! Read-only inspection of an interrupted simulator ride and Assistant checkpoint.
use obc_formats::io::ByteSource;
use obc_storage::flat::{BlockDevice, EntryFlags, FlatStore, ObjectKind, Store};
use std::{
    fs::File,
    io::{self, Write},
    os::unix::fs::FileExt,
};

struct ReadOnly(File);
impl BlockDevice for ReadOnly {
    type Error = io::Error;
    fn block_count(&self) -> io::Result<u64> {
        Ok(self.0.metadata()?.len() / 512)
    }
    fn read(&self, lba: u64, bytes: &mut [u8]) -> io::Result<()> {
        self.0.read_exact_at(bytes, lba * 512)
    }
    fn write(&self, _: u64, _: &[u8]) -> io::Result<()> {
        Err(io::Error::from(io::ErrorKind::PermissionDenied))
    }
    fn sync(&self) -> io::Result<()> {
        self.0.sync_all()
    }
}
fn main() {
    let path = std::env::args().nth(1).expect("read_card CARD > evidence.log");
    let store = FlatStore::mount(ReadOnly(File::open(path).unwrap()));
    assert!(store.mode().readable());
    println!("store {:?}; mode {:?}", store.store_id(), store.mode());
    let checkpoint = obc_storage::flat::metadata::read_checkpoint(&store).unwrap();
    println!("checkpoint {checkpoint:?}");
    if let Some(path) = std::env::args().nth(2) {
        let checkpoint = checkpoint.expect("accepted Assistant checkpoint required for motion CSV");
        let mut output = File::create_new(path).unwrap();
        writeln!(output, "m,lon,lat,ele,stop").unwrap();
        store
            .with_source(
                obc_storage::flat::ObjectId(checkpoint.route.object),
                Some(obc_storage::flat::Revision(checkpoint.route.revision)),
                |source| {
                    let index = obc_route::RouteIndex::read(source).unwrap();
                    let route = obc_route::RouteReader::new(&index, source);
                    let visit = route.visit_descriptor().unwrap().expect("Visit descriptor required");
                    println!("visit {visit:?}; total_m {}", route.total_distance_m);
                    let mut positions: Vec<u32> = (0..route.total_distance_m).step_by(5).collect();
                    positions.push(route.total_distance_m);
                    positions.extend(visit.accepted_anchors_m);
                    positions.sort();
                    positions.dedup();
                    for m in positions {
                        let point = route.position_at(m).unwrap();
                        writeln!(
                            output,
                            "{m},{},{},{},{}",
                            point.lon,
                            point.lat,
                            route.elevation_at(m).map(|e| e.to_string()).unwrap_or_default(),
                            u8::from(m == visit.accepted_anchors_m[1])
                        )
                        .unwrap();
                    }
                },
            )
            .unwrap();
    }
    let mut rides = 0;
    for entry in store.entries() {
        if entry.kind == ObjectKind::Ride {
            rides += 1;
            println!("ride {entry:?}");
            if entry.flags == EntryFlags::NONE {
                store
                    .with_source(entry.id, Some(entry.revision), |source| {
                        let info = obc_route::RideInfo::read(source).unwrap();
                        println!("saved {info:?}");
                        let mut crc = samples(info.point_count as u64, |offset, bytes| {
                            source.read_at(offset, bytes).unwrap();
                        });
                        let mut footer = [0; obc_formats::ride::FOOTER_LEN];
                        source.read_at(source.len() - footer.len() as u64, &mut footer).unwrap();
                        crc.update(&footer);
                        assert_eq!(crc.finalize(), entry.payload_crc);
                    })
                    .unwrap();
            }
        }
    }
    assert!(store.entries_ok());
    println!("ride_objects {rides}");
    if let Some(ride) = store.recovered_ride() {
        println!(
            "recording {:?} {:?}; sequence {}; bytes {}; crc {:08x}",
            ride.id,
            ride.revision,
            ride.checkpoint_sequence,
            ride.payload_len(),
            ride.payload_crc
        );
        println!("continuation {:?}", obc_app::recorder::continuation::decode(&ride.resume));
        assert_eq!(ride.payload_len() % obc_formats::track::RECORD_LEN as u64, 0);
        let crc = samples(ride.payload_len() / obc_formats::track::RECORD_LEN as u64, |offset, bytes| {
            assert_eq!(store.read_recovered(offset, bytes).unwrap(), bytes.len());
        });
        assert_eq!(crc.finalize(), ride.payload_crc);
    }
}

fn samples(count: u64, mut read: impl FnMut(u64, &mut [u8])) -> obc_crc::Crc32 {
    let mut previous = None;
    let mut first = None;
    let mut last = None;
    let mut segments = 0;
    let mut max_gap = 0;
    let mut crc = obc_crc::Crc32::new();
    for index in 0..count {
        let mut bytes = [0; obc_formats::track::RECORD_LEN];
        read(index * bytes.len() as u64, &mut bytes);
        crc.update(&bytes);
        let point = obc_formats::track::decode_record(&bytes);
        if let Some(prior) = previous {
            assert!(point.t_ms >= prior, "recorded time went backwards");
            max_gap = max_gap.max(point.t_ms - prior);
        }
        previous = Some(point.t_ms);
        first.get_or_insert(point);
        last = Some(point);
        segments += usize::from(point.segment_start);
    }
    println!("samples {}; segments {segments}; max_gap_ms {max_gap}; first {first:?}; last {last:?}", count);
    crc
}
