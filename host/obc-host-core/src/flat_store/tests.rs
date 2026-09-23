use super::*;
use crate::{
    flat_map::{FlatMap, MapError},
    FlatRouteStore, RouteRepository,
};
use std::io::Write;

const ROUTE: &[u8] = include_bytes!("../../../../fixtures/sources/sim-grimsel/routes/grimsel-climb.obcr");
const MAP: &[u8] = include_bytes!("../../../../apps/obc-sim/assets/grimsel-demo.obcm");

fn input(bytes: &[u8]) -> std::fs::File {
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(bytes).unwrap();
    file
}

fn bytes(source: &ObjectSource) -> Vec<u8> {
    let mut bytes = vec![0; source.len() as usize];
    source.read_at(0, &mut bytes).unwrap();
    bytes
}

#[test]
fn native_map_and_routes_share_persistent_identity_and_revision_leases() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("card.obc");
    let owner = HostStore::create_file(&path).unwrap();
    let identity = owner.store_id().unwrap();
    let map = FlatMap::from_file_in(&owner, input(MAP)).unwrap();
    let map_id = map.source().id();
    let mut routes = FlatRouteStore::new(HostStore(owner.0.clone()), &[ROUTE]).unwrap();
    let route_id = routes.write_nav_route(ROUTE).unwrap();
    let old = owner.open(ObjectId(route_id), Revision(1)).unwrap();
    assert_eq!(old.store_id(), map.source().store_id());
    assert_eq!(routes.write_nav_route(ROUTE), Some(route_id));
    assert_eq!(old.revision(), Revision(1));
    assert_eq!(bytes(&old), ROUTE);
    let new_map = map.replace_from_file(input(MAP)).unwrap();
    assert_eq!(new_map.source().id(), map_id);
    assert_eq!(new_map.source().revision(), Revision(2));
    assert_eq!(bytes(&map.source()), MAP);
    assert!(matches!(map.replace_from_file(input(MAP)), Err(MapError::Storage(StoreError::NotFound))));
    assert_eq!(routes.delete_by_id(map_id.0), Err(obc_app::catalog_state::CatalogError::Unsupported));
    let removed = routes.ids()[0];
    assert_eq!(routes.delete_by_id(removed), Ok(true));
    drop((owner, routes, old, map, new_map));

    let reopened = HostStore::open_file(&path).unwrap();
    assert_eq!(reopened.store_id().unwrap(), identity);
    assert!(matches!(reopened.open(ObjectId(removed), Revision(1)), Err(StoreError::NotFound)));
    let map = FlatMap::open_in(&reopened, identity, map_id, Revision(2)).unwrap();
    assert_eq!(bytes(&map.source()), MAP);
    let source = reopened.open(ObjectId(route_id), Revision(2)).unwrap();
    let routes = FlatRouteStore::new(reopened, &[]).unwrap();
    assert_eq!(routes.ids(), &[route_id]);
    assert_eq!(bytes(&source), ROUTE);
    assert!(path.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let payload_bytes = (2 * MAP.len() + 3 * ROUTE.len()) as u64;
        let allocated = std::fs::metadata(path).unwrap().blocks() * 512;
        assert!(allocated < payload_bytes + 16 * 1024 * 1024, "sparse card allocated {allocated} bytes");
    }
}

/// Every name the store left behind. An `._` sidecar belongs to a FAT volume itself,
/// not to the store, and a temporary card is `.tmp` and six characters.
fn names(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with("._"))
        .collect();
    names.sort();
    names
}

#[test]
fn creation_publishes_one_card_name_and_refuses_an_occupied_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("card.obc");
    let identity = HostStore::create_file(&path).unwrap().store_id().unwrap();
    // The card is built under a temporary sibling. Only the published name survives.
    assert_eq!(names(dir.path()), ["card.obc"]);
    assert!(
        matches!(HostStore::create_file(&path), Err(ImportError::Io(e)) if e.kind() == io::ErrorKind::AlreadyExists)
    );
    let absent = dir.path().join("absent").join("card.obc");
    assert!(matches!(HostStore::create_file(absent), Err(ImportError::Io(e)) if e.kind() == io::ErrorKind::NotFound));
    assert_eq!(names(dir.path()), ["card.obc"]);
    assert_eq!(HostStore::open_file(&path).unwrap().store_id().unwrap(), identity);
}

/// The exclusive rename that `publish` prefers is unavailable on exFAT and on many network
/// mounts, so the reserved-name path publishes those cards.
#[cfg(unix)]
#[test]
fn a_reserved_card_name_takes_the_temporary_and_still_refuses_an_occupied_name() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("card.obc");
    let mut temporary = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
    temporary.write_all(b"published bytes").unwrap();
    reserve_and_rename(temporary.into_temp_path(), &path).unwrap();
    assert_eq!(names(dir.path()), ["card.obc"]);
    assert_eq!(std::fs::read(&path).unwrap(), b"published bytes");

    let refused = tempfile::NamedTempFile::new_in(dir.path()).unwrap().into_temp_path();
    assert_eq!(reserve_and_rename(refused, &path).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(names(dir.path()), ["card.obc"]);
    assert_eq!(std::fs::read(&path).unwrap(), b"published bytes");
}

#[test]
fn final_reader_holds_exclusive_file_lock_and_new_cards_have_distinct_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("card.obc");
    let owner = HostStore::create_file(&path).unwrap();
    let map = FlatMap::from_file_in(&owner, input(MAP)).unwrap();
    let reader = map.source();
    let identity = reader.store_id();
    drop((owner, map));
    assert!(matches!(HostStore::open_file(&path), Err(ImportError::Io(e)) if e.kind() == io::ErrorKind::WouldBlock));
    assert_eq!(bytes(&reader), MAP);
    drop(reader);
    assert_eq!(HostStore::open_file(&path).unwrap().store_id().unwrap(), identity);
    assert!(
        matches!(HostStore::create_file(&path), Err(ImportError::Io(e)) if e.kind() == io::ErrorKind::AlreadyExists)
    );
    let other = HostStore::create_file(dir.path().join("new-card.obc")).unwrap();
    assert_ne!(other.store_id().unwrap(), identity);
    assert!(matches!(
        FlatMap::open_in(&other, identity, ObjectId(1), Revision(1)),
        Err(MapError::Storage(StoreError::Invalid))
    ));
}

#[test]
fn invalid_and_truncated_cards_are_not_initialized_or_resized() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing");
    assert!(matches!(HostStore::open_file(&missing), Err(ImportError::Io(e)) if e.kind() == io::ErrorKind::NotFound));
    assert!(!missing.exists());
    let path = dir.path().join("card.obc");
    std::fs::write(&path, b"keep these bytes").unwrap();
    assert!(matches!(HostStore::open_file(&path), Err(ImportError::Io(e)) if e.kind() == io::ErrorKind::InvalidData));
    assert_eq!(std::fs::read(&path).unwrap(), b"keep these bytes");
    std::fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(CARD_BYTES).unwrap();
    assert!(matches!(HostStore::open_file(&path), Err(ImportError::Mount(Mode::Unformatted))));
    let mut prefix = [0; 16];
    std::fs::File::open(&path).unwrap().read_exact(&mut prefix).unwrap();
    assert_eq!(&prefix, b"keep these bytes");
    let truncated = dir.path().join("truncated.obc");
    drop(HostStore::create_file(&truncated).unwrap());
    std::fs::OpenOptions::new().write(true).open(&truncated).unwrap().set_len(CARD_BYTES - BLOCK).unwrap();
    assert!(
        matches!(HostStore::open_file(&truncated), Err(ImportError::Io(e)) if e.kind() == io::ErrorKind::InvalidData)
    );
    assert_eq!(std::fs::metadata(truncated).unwrap().len(), CARD_BYTES - BLOCK);
}

#[test]
fn malformed_map_and_short_input_preserve_existing_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("card.obc");
    let owner = HostStore::create_file(&path).unwrap();
    let map = FlatMap::from_file_in(&owner, input(MAP)).unwrap();
    let before = owner.entries().unwrap();
    assert!(matches!(map.replace_from_file(input(b"not a map")), Err(MapError::Format(_))));
    assert!(matches!(FlatMap::from_file_in(&owner, input(b"not a map")), Err(MapError::Format(_))));
    assert!(
        matches!(owner.import(ObjectKind::Route, None, &mut &ROUTE[..], ROUTE.len() as u64 + 1, DisplayName::default()),
        Err(ImportError::Io(e)) if e.kind() == io::ErrorKind::UnexpectedEof)
    );
    assert_eq!(owner.entries().unwrap(), before);
    assert_eq!(bytes(&map.source()), MAP);
    drop((owner, map));
    assert_eq!(HostStore::open_file(path).unwrap().entries().unwrap(), before);
}

#[test]
fn exhausted_counter_mount_remains_readable_and_refuses_writes() {
    for exhausted in [Mode::RevisionSpaceExhausted, Mode::SequenceSpaceExhausted, Mode::ReadWrite] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("card.obc");
        let owner = HostStore::create_file(&path).unwrap();
        let identity = owner.store_id().unwrap();
        let meta =
            owner.import(ObjectKind::Route, None, &mut &ROUTE[..], ROUTE.len() as u64, DisplayName::default()).unwrap();
        drop(owner);

        // FLAT_Store_Format §§5.1–5.4: the first commit publishes copy B. Construct
        // an exhausted, checksummed catalog because the writer refuses counter wrap.
        use std::io::{Seek, SeekFrom};
        let mut file = std::fs::OpenOptions::new().read(true).write(true).open(&path).unwrap();
        let body_offset = 576 * BLOCK;
        let gate_offset = body_offset + 480 * BLOCK;
        let mut body = [0; 512 + 128];
        let mut gate = [0; 512];
        file.seek(SeekFrom::Start(body_offset)).unwrap();
        file.read_exact(&mut body).unwrap();
        file.seek(SeekFrom::Start(gate_offset)).unwrap();
        file.read_exact(&mut gate).unwrap();
        let revision = if exhausted == Mode::RevisionSpaceExhausted {
            body[512 + 16..512 + 24].copy_from_slice(&u64::MAX.to_le_bytes());
            Revision(u64::MAX)
        } else {
            let sequence = if exhausted == Mode::ReadWrite { u64::MAX - 1 } else { u64::MAX };
            body[24..32].copy_from_slice(&sequence.to_le_bytes());
            gate[24..32].copy_from_slice(&sequence.to_le_bytes());
            meta.revision
        };
        gate[36..40].copy_from_slice(&obc_crc::crc32(&body).to_le_bytes());
        let crc = obc_crc::crc32(&gate[..504]);
        gate[504..508].copy_from_slice(&crc.to_le_bytes());
        file.seek(SeekFrom::Start(body_offset)).unwrap();
        file.write_all(&body).unwrap();
        file.seek(SeekFrom::Start(gate_offset)).unwrap();
        file.write_all(&gate).unwrap();
        file.sync_all().unwrap();
        drop(file);

        let owner = HostStore::open_file(path).unwrap();
        assert_eq!(owner.mode().unwrap(), exhausted);
        assert_eq!(owner.store_id().unwrap(), identity);
        assert_eq!(bytes(&owner.open(meta.id, revision).unwrap()), ROUTE);
        assert!(matches!(owner.import_computed_route(ROUTE, None), Err(ImportError::Storage(StoreError::ReadOnly))));
        assert_eq!(owner.entries().unwrap().len(), 1);
        if exhausted == Mode::ReadWrite {
            // One slot remains for ordinary writes, but not publication plus compensation.
            assert!(owner
                .import(ObjectKind::Route, None, &mut &ROUTE[..], ROUTE.len() as u64, DisplayName::default())
                .is_ok());
        } else {
            assert!(matches!(
                owner.import(ObjectKind::Route, None, &mut &ROUTE[..], ROUTE.len() as u64, DisplayName::default()),
                Err(ImportError::Storage(StoreError::ReadOnly))
            ));
        }
    }
}

#[test]
fn failed_commit_barrier_blocks_reuse_until_explicit_remount() {
    for failure_sync in [1, 4] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("card.obc");
        let owner = HostStore::create_file(&path).unwrap();
        let mut routes = FlatRouteStore::new(HostStore(owner.0.clone()), &[]).unwrap();
        let id = routes.write_nav_route(ROUTE).unwrap();
        let old = owner.open(ObjectId(id), Revision(1)).unwrap();
        {
            let state = owner.0.lock().unwrap();
            let HostMedia::File(card) = state.card.device() else { unreachable!() };
            card.borrow().fail_sync_after.set(Some(failure_sync));
        }
        assert_eq!(routes.write_nav_route(ROUTE), None);
        assert_eq!(routes.ids(), &[id]);
        assert_eq!(bytes(&old), ROUTE);
        assert!(owner.entries().is_err());
        assert!(owner.open(ObjectId(id), Revision(1)).is_err());
        assert!(matches!(
            owner.import(ObjectKind::Route, None, &mut &ROUTE[..], ROUTE.len() as u64, DisplayName::default()),
            Err(ImportError::RemountRequired)
        ));
        assert!(owner.remove(ObjectKind::Route, ObjectId(id), Revision(1)).is_err());
        drop((routes, owner, old));
        let reopened = HostStore::open_file(path).unwrap();
        let revision = if failure_sync == 4 { Revision(2) } else { Revision(1) };
        assert_eq!(reopened.entries().unwrap()[0].revision, revision);
        assert_eq!(bytes(&reopened.open(ObjectId(id), revision).unwrap()), ROUTE);
    }
}

#[test]
fn committed_map_identity_survives_reader_slot_exhaustion() {
    let dir = tempfile::tempdir().unwrap();
    let owner = HostStore::create_file(dir.path().join("card.obc")).unwrap();
    let _routes =
        FlatRouteStore::new(HostStore(owner.0.clone()), &[ROUTE; obc_storage::flat::store::MAX_OPEN_OBJECTS]).unwrap();
    let mut readers: Vec<_> =
        owner.entries().unwrap().iter().map(|meta| owner.open(meta.id, meta.revision).unwrap()).collect();
    let error = FlatMap::from_file_in(&owner, input(MAP)).err().unwrap();
    let MapError::CommittedWithoutReader { store_id, id, revision, error: StoreError::Busy } = error else {
        panic!("{error}")
    };
    assert_eq!(owner.entries().unwrap().len(), obc_storage::flat::store::MAX_OPEN_OBJECTS + 1);
    readers.pop();
    let map = FlatMap::open_in(&owner, store_id, id, revision).unwrap();
    assert_eq!(bytes(&map.source()), MAP);
}
