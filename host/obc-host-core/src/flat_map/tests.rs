use super::*;
use crate::flat_store::{HostMedia, Lease, MountedStore, ObjectSource, IMPORT_BUFFER_BYTES, PAGE};
use crate::test_support::CountedSource;
use embedded_graphics::{pixelcolor::Rgb888, prelude::*};
use obc_formats::io::SliceSource;
use obc_formats::io::{ByteSource, Error};
use obc_storage::flat::{EntryFlags, EntryMeta, FlatStore, Mutation, ObjectId, PutSource, Revision, Store, StoreId};
use obcm_testkit::{build_file, pack_line, seal, LodSpec};
use std::{
    cell::RefCell,
    io::Read,
    sync::{Arc, Mutex},
};

fn map_bytes() -> Vec<u8> {
    let chunk = seal(pack_line(1, 100, 100, &[(50, 50), (50, -50)]), 4096);
    build_file(
        (0, 0, 4000, 4000),
        &[(1, 0, 0x07E0, 1, 1, false, None)],
        &[LodSpec { max_mpp: f32::INFINITY, index: vec![0], chunks: vec![chunk], chunk_size: 4096 }],
    )
}

fn render(source: &dyn ByteSource) -> (Vec<u8>, usize, usize) {
    let source = CountedSource::new(source);
    let tables = MapTables::parse(&source).unwrap();
    let cache = MapCache::new_boxed();
    let reader = Reader::new(&source, &tables, &cache);
    let mut frame = crate::RgbaFrame::new(64, 64);
    let mut scratch = Box::new(obc_render::RenderScratch::new());
    let stats = scratch.render(
        &mut frame,
        &reader,
        &obc_render::Viewport::new(64.0, 64.0, 150, 125, 0.2),
        Rgb888::BLACK,
        obc_render::RenderConfig::default(),
        |color| {
            let (r, g, b) = obc_reader::rgb565_to_rgb888(color);
            Rgb888::new(r, g, b)
        },
    );
    assert_eq!(stats.features_drawn, 1);
    assert!(frame.as_rgba().as_chunks::<4>().0.iter().any(|pixel| pixel[..3] == [0, 255, 0]));
    (frame.as_rgba().to_vec(), source.reads(), source.bytes())
}

#[test]
fn native_and_memory_maps_render_and_read_like_the_original_bytes() {
    let bytes = map_bytes();
    let memory = FlatMap::from_bytes(&bytes).unwrap();
    let mut input = tempfile::NamedTempFile::new().unwrap();
    std::io::Write::write_all(&mut input, &bytes).unwrap();
    let native = FlatMap::from_file(std::fs::File::open(input.path()).unwrap()).unwrap();
    let oracle = render(&SliceSource(&bytes));
    assert_eq!(render(&memory.source()), oracle);
    assert_eq!(render(&native.source()), oracle);
    assert_ne!(
        memory.source.0.owner.lock().unwrap().card.store_id(),
        native.source.0.owner.lock().unwrap().card.store_id()
    );
    assert_eq!(memory.source.id(), ObjectId(1));
    assert_eq!(memory.source.revision(), Revision(1));
    assert_eq!(std::fs::read(input.path()).unwrap(), bytes);
}

fn publish(store: &FlatStore<HostMedia>, id: ObjectId, revision: Revision, bytes: &[u8]) {
    let mut allocation = store.allocate(bytes.len() as u64).unwrap();
    store.write(&mut allocation, bytes).unwrap();
    let put = Mutation::Put {
        meta: EntryMeta {
            added_at_utc: 0,
            id,
            revision,
            kind: ObjectKind::MapShard,
            flags: EntryFlags::NONE,
            payload_len: bytes.len() as u64,
            payload_crc: obc_crc::crc32(bytes),
            name: DisplayName::default(),
        },
        source: PutSource::Fresh(allocation),
    };
    if revision == Revision(1) {
        store.commit(&[put]).unwrap();
    } else {
        store.commit(&[Mutation::Remove { id, revision: Revision(revision.0 - 1) }, put]).unwrap();
    }
}

#[test]
fn clones_pin_the_full_identity_and_revision_until_the_last_drop() {
    let identity = StoreId([0x91, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 0xfa]);
    let store = FlatStore::initialize(HostMedia::Memory(RefCell::default()), identity).unwrap();
    let id = ObjectId((1 << 48) + 7);
    publish(&store, id, Revision(1), b"old map");
    let owner = Arc::new(Mutex::new(MountedStore::new(store, false)));
    let first = ObjectSource::open(owner.clone(), id, None).unwrap();
    let worker = first.clone();
    assert_eq!(owner.lock().unwrap().card.store_id(), identity);
    assert_eq!(worker.id(), id);
    let free = owner.lock().unwrap().card.free_extents();
    publish(&owner.lock().unwrap().card, id, Revision(2), b"new");
    let head = ObjectSource::open(owner.clone(), id, None).unwrap();
    assert_eq!(head.revision(), Revision(2));
    let mut new = [0; 3];
    head.read_at(0, &mut new).unwrap();
    assert_eq!(&new, b"new");
    drop(first);
    let mut old = [0; 7];
    worker.read_at(0, &mut old).unwrap();
    assert_eq!(&old, b"old map");
    assert_eq!(worker.revision(), Revision(1));
    assert_eq!(owner.lock().unwrap().card.free_extents(), free - 1);
    drop(worker);
    assert_eq!(owner.lock().unwrap().card.free_extents(), free);
    owner.lock().unwrap().card.commit(&[Mutation::Remove { id, revision: Revision(2) }]).unwrap();
    head.read_at(0, &mut new).unwrap();
    drop(head);
    assert_eq!(owner.lock().unwrap().card.free_extents(), free + 1);
}

#[test]
fn background_reader_owns_the_temporary_card_and_exact_bounds() {
    let bytes = map_bytes();
    let mut input = tempfile::NamedTempFile::new().unwrap();
    std::io::Write::write_all(&mut input, &bytes).unwrap();
    let map = FlatMap::from_file(std::fs::File::open(input.path()).unwrap()).unwrap();
    let path = match map.source.0.owner.lock().unwrap().card.device() {
        HostMedia::File(file) => file.borrow()._temporary.as_ref().unwrap().to_path_buf(),
        HostMedia::Memory(_) => unreachable!(),
    };
    let worker = map.source();
    drop(map);
    assert!(path.exists());
    let worker_path = path.clone();
    std::thread::spawn(move || {
        let len = worker.len();
        assert_eq!(worker.read_at(len, &mut []), Ok(()));
        assert_eq!(worker.read_at(len + 1, &mut []), Err(Error::BadOffset));
        assert_eq!(worker.read_at(len, &mut [0]), Err(Error::BadOffset));
        assert_eq!(worker.read_at(u64::MAX, &mut [0]), Err(Error::BadOffset));
        let mut prefix = [0; 4];
        worker.read_at(0, &mut prefix).unwrap();
        assert_eq!(&prefix, b"OBCM");
        std::fs::OpenOptions::new().write(true).open(worker_path).unwrap().set_len(0).unwrap();
        assert_eq!(worker.read_at(0, &mut prefix), Err(Error::Io));
        assert_eq!(worker.read_at(len, &mut [0]), Err(Error::BadOffset));
    })
    .join()
    .unwrap();
    assert!(!path.exists());
    assert!(input.path().exists());
}

#[test]
fn import_is_bounded_and_refuses_short_or_growing_inputs() {
    struct Bounded<'a>(&'a [u8]);
    impl Read for Bounded<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            assert!(buffer.len() <= IMPORT_BUFFER_BYTES);
            self.0.read(buffer)
        }
    }
    let mut bytes = map_bytes();
    bytes.resize(IMPORT_BUFFER_BYTES * 3 + 7, 0);
    let owner = HostStore::memory().unwrap();
    let free = owner.0.lock().unwrap().card.free_extents();
    for _ in 0..12 {
        for (len, expected) in [
            (bytes.len() as u64 + 1, io::ErrorKind::UnexpectedEof),
            (bytes.len() as u64 - 1, io::ErrorKind::InvalidData),
        ] {
            assert!(matches!(
                owner.import(ObjectKind::MapShard, None, &mut Bounded(&bytes), len, DisplayName::default()),
                Err(ImportError::Io(error)) if error.kind() == expected
            ));
            assert_eq!(owner.0.lock().unwrap().card.free_extents(), free, "failed import cancels its allocation");
        }
    }
    let meta = owner
        .import(ObjectKind::MapShard, None, &mut Bounded(&bytes), bytes.len() as u64, DisplayName::default())
        .unwrap();
    assert_eq!(owner.open(meta.id, meta.revision).unwrap().len(), bytes.len() as u64);
    let free = owner.0.lock().unwrap().card.free_extents();
    assert!(owner
        .import(
            ObjectKind::MapShard,
            Some((meta.id, Revision(77))),
            &mut Bounded(&bytes),
            bytes.len() as u64,
            DisplayName::default()
        )
        .is_err());
    assert_eq!(owner.0.lock().unwrap().card.free_extents(), free, "a rejected commit also cancels its allocation");

    assert!(matches!(FlatMap::from_bytes(b"invalid map"), Err(MapError::Format(_))));
}

#[test]
fn browser_card_allocates_written_pages_only() {
    let bytes = include_bytes!("../../../../apps/obc-sim/assets/grimsel-demo.obcm");
    let map = FlatMap::from_bytes(bytes).unwrap();
    let owner = map.source.0.owner.lock().unwrap();
    let store = &owner.card;
    let HostMedia::Memory(pages) = store.device() else { unreachable!() };
    let page_bytes = pages.borrow().len() * PAGE;
    // Two catalog copies, headers and the imported map, independent of the 32 GiB capacity.
    assert!(page_bytes < bytes.len() + 1024 * 1024);
    let fixed =
        std::mem::size_of::<FlatStore<HostMedia>>() + std::mem::size_of::<Lease>() + std::mem::size_of::<FlatMap>();
    eprintln!(
        "flat map memory: input={} pages={} page_bytes={} fixed={} existing_cache={}",
        bytes.len(),
        pages.borrow().len(),
        page_bytes,
        fixed,
        std::mem::size_of::<MapCache>()
    );
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn native_map_snapshot_is_independent_of_later_source_edits() {
    use std::io::{Seek, SeekFrom, Write};
    let bytes = include_bytes!("../../../../apps/obc-sim/assets/grimsel-demo.obcm");
    let mut original = tempfile::tempfile().unwrap();
    original.write_all(bytes).unwrap();
    let snapshot = super::snapshot(original.try_clone().unwrap()).unwrap();
    original.seek(SeekFrom::Start(0)).unwrap();
    original.write_all(b"invalid").unwrap();
    let owner = HostStore::memory().unwrap();
    let map = FlatMap::from_file_in(&owner, snapshot.into_file()).unwrap();
    let mut stored = vec![0; bytes.len()];
    map.source().read_at(0, &mut stored).unwrap();
    assert_eq!(stored, bytes);
    assert!(matches!(FlatMap::from_file_in(&owner, original), Err(super::MapError::Format(_))));
}
