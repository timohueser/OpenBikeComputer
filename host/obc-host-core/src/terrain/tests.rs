use super::*;
use crate::flat_store::HostStore;
use obc_elevation::{TerrainReader, DEFAULT_TILE_SLOTS};
use obc_formats::io::{ByteSource, SliceSource};

#[test]
fn embedded_samples_match_reader_and_reuse_one_bounded_cache() {
    let bytes = obcm_testkit::terrain::map(0);
    let map = FlatMap::from_bytes(&bytes).unwrap();
    let mut elevation = FlatElevation::open(&map).unwrap().unwrap();
    assert!(elevation.source.same_revision(&map.source()));
    let region = map.tables().terrain().unwrap();
    let original = SliceSource(&bytes);
    let window = WindowSource::new(&original, region.offset, region.len).unwrap();
    let reader = TerrainReader::parse(&window).unwrap();
    let mut cache = Box::new(TileCache::<DEFAULT_TILE_SLOTS>::new());
    for (lat, lon) in [(0, 0), (512, 512), (8192, 8192), (9300, 8700), (20_000, 100)] {
        assert_eq!(elevation.sample(lat, lon), reader.sample(&mut cache, lat, lon));
    }
    assert_eq!(elevation.sample(0, 0), Some(0));
    assert_eq!(elevation.sample(8192, 8192), Some(80));
    let misses = elevation.cache.stats().1;
    assert_eq!(elevation.sample(8192, 8192), Some(80));
    assert_eq!(elevation.cache.stats().1, misses);
    let other = FlatMap::from_bytes(&obcm_testkit::terrain::map(300)).unwrap();
    let mut other_elevation = FlatElevation::open(&other).unwrap().unwrap();
    assert_eq!(elevation.source.id(), other_elevation.source.id());
    assert_ne!(elevation.source.store_id(), other_elevation.source.store_id());
    assert_eq!(other_elevation.sample(8192, 8192), Some(380));
    assert_eq!(elevation.sample(8192, 8192), Some(80));
}

#[test]
fn absent_terrain_is_distinct_from_unsupported_or_invalid_terrain() {
    let bytes = obcm_testkit::build_file(
        (0, 0, 16384, 16384),
        &[],
        &[obcm_testkit::LodSpec { max_mpp: f32::INFINITY, index: vec![], chunks: vec![], chunk_size: 4096 }],
    );
    assert!(FlatElevation::open(&FlatMap::from_bytes(&bytes).unwrap()).unwrap().is_none());
    let mut terrain = obcm_testkit::terrain::plane(0);
    terrain[4] = 99;
    let map = FlatMap::from_bytes(&obcm_testkit::splice_terrain(&bytes, &terrain)).unwrap();
    assert!(matches!(FlatElevation::open(&map), Err(Error::BadVersion)));
    terrain[4] = 1;
    terrain[32..36].copy_from_slice(&u32::MAX.to_le_bytes());
    let map = FlatMap::from_bytes(&obcm_testkit::splice_terrain(&bytes, &terrain)).unwrap();
    assert!(matches!(FlatElevation::open(&map), Err(Error::BadOffset)));
}

#[test]
fn persisted_terrain_reopens_and_owns_exact_revisions_until_last_drop() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card.obc");
    let input = dir.path().join("map.obcm");
    std::fs::write(&input, obcm_testkit::terrain::map(0)).unwrap();
    let store = HostStore::create_file(&card).unwrap();
    let map = FlatMap::from_file_in(&store, std::fs::File::open(&input).unwrap()).unwrap();
    let mut old = FlatElevation::open(&map).unwrap().unwrap();
    let identity = (old.source.store_id(), old.source.id(), old.source.revision());
    std::fs::remove_file(&input).unwrap();
    drop(map);
    drop(store);
    assert_eq!(old.sample(8192, 8192), Some(80));
    assert!(HostStore::open_file(&card).is_err(), "terrain retains the card lock");
    drop(old);
    let store = HostStore::open_file(&card).unwrap();
    let map = FlatMap::open_only_in(&store).unwrap();
    let mut old = FlatElevation::open(&map).unwrap().unwrap();
    assert_eq!((old.source.store_id(), old.source.id(), old.source.revision()), identity);
    assert_eq!(old.sample(8192, 8192), Some(80));
    std::fs::write(&input, obcm_testkit::terrain::map(100)).unwrap();
    let replacement = map.replace_from_file(std::fs::File::open(&input).unwrap()).unwrap();
    let mut new = FlatElevation::open(&replacement).unwrap().unwrap();
    assert!(!old.source.is_current());
    assert_eq!(old.sample(8192, 8192), Some(80));
    assert_eq!(new.sample(8192, 8192), Some(180));
    assert_eq!(new.source.store_id(), identity.0);
    assert_eq!(new.source.id(), identity.1);
    assert_ne!(new.source.revision(), identity.2);
    drop(map);
    drop(replacement);
    drop(store);
    drop(new);
    assert!(HostStore::open_file(&card).is_err());
    drop(old);
    assert!(HostStore::open_file(&card).is_ok());
}

#[test]
fn unreadable_card_never_becomes_a_height_or_successful_mount() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card.obc");
    let store = HostStore::create_file(&card).unwrap();
    let map = FlatMap::from_bytes_in(&store, &obcm_testkit::terrain::map(0)).unwrap();
    let mut elevation = FlatElevation::open(&map).unwrap().unwrap();
    std::fs::OpenOptions::new().write(true).open(&card).unwrap().set_len(0).unwrap();
    assert_eq!(elevation.sample(512, 512), None);
    assert_eq!(elevation.sample(512, 512), None, "failed tiles must not become cache hits");
    assert!(matches!(FlatElevation::open(&map), Err(Error::Io)));
    assert!(elevation.source.len() > 0, "the lease keeps its immutable declared bounds");
}
