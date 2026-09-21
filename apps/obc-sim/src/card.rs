//! Native session composition. File inputs are imported once; runtime readers share one card.

use crate::{
    map_file::{LoadedMap, MapSource},
    Args, Injection,
};
use obc_host_core::{
    flat_store::HostStore, FlatRideRecorder, FlatRideStore, FlatRouteStore, FlatTripStore, RouteRepository, TrackStore,
};
use std::path::{Path, PathBuf};

pub struct Session {
    pub map: LoadedMap,
    pub routes: FlatRouteStore,
    pub trips: FlatTripStore,
    pub rides: FlatRideStore,
    pub tracks: TrackStore,
}

pub fn persistent(path: &str, create: bool) -> Result<HostStore, String> {
    if create { HostStore::create_file(path) } else { HostStore::open_file(path) }
        .map_err(|error| format!("card {path}: {error}"))
}

impl Session {
    pub fn load(args: &mut Args) -> Result<Self, String> {
        let owner = match (&args.card, &args.create_card) {
            (Some(path), _) => persistent(path, false)?,
            (_, Some(path)) => persistent(path, true)?,
            _ => HostStore::temporary().map_err(|error| error.to_string())?,
        };
        let recorder = FlatRideRecorder::new(owner.clone()).map_err(|error| format!("ride recovery: {error:?}"))?;
        let map = if args.card.is_some() {
            LoadedMap::reopen(&owner).map_err(|error| format!("reopen map: {error}"))?
        } else {
            let source = MapSource::load_single(&args.map).map_err(|error| error.to_string())?;
            LoadedMap::open_in(source, &owner).map_err(|error| error.to_string())?
        };
        let mut routes = FlatRouteStore::new(owner.clone(), &[]).map_err(|error| error.to_string())?;
        let mut trips = FlatTripStore::new(owner.clone()).map_err(|error| format!("trips: {error:?}"))?;
        if args.card.is_none() {
            let files = input_files(Path::new(&args.routes_dir()), args.routes_dir.is_some())?;
            let mut route_ids = Vec::new();
            for path in files.iter().filter(|path| extension(path, "obcr")) {
                let bytes = std::fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
                route_ids.push(routes.import(&bytes).map_err(|error| format!("import {}: {error}", path.display()))?);
            }
            let mut trip_ids = Vec::new();
            for path in files.iter().filter(|path| extension(path, "obt")) {
                let input = std::fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
                let bytes = crate::trips::remap(&input, &route_ids)?;
                let id = trips.import(&bytes).map_err(|error| format!("import {}: {error}", path.display()))?;
                if let Some(number) = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .and_then(|stem| stem.strip_prefix("TP").or_else(|| stem.strip_prefix("tp")))
                    .and_then(|digits| digits.parse::<u64>().ok())
                {
                    if let Some(old) = number.checked_add(obc_host_core::TRIP_ID_BASE) {
                        trip_ids.push((old, id));
                    }
                }
            }
            args.inject = match args.inject {
                Some(Injection::Upload { id, replaced }) => Some(Injection::Upload {
                    id: usize::try_from(id)
                        .ok()
                        .and_then(|index| route_ids.get(index))
                        .copied()
                        .ok_or("upload fixture does not name an imported route")?,
                    replaced,
                }),
                Some(Injection::TripUpload { id }) => Some(Injection::TripUpload {
                    id: trip_ids
                        .iter()
                        .find(|(old, _)| *old == id)
                        .map(|(_, id)| *id)
                        .ok_or("trip upload fixture does not name an imported trip")?,
                }),
                other => other,
            };
        }
        let mut rides = FlatRideStore::new(owner.clone()).map_err(|error| format!("rides: {error:?}"))?;
        if args.card.is_none() {
            crate::rides::import(Path::new(&args.tracks_dir()), &mut rides)?;
        }
        let tracks = TrackStore::new(recorder, owner.clone(), args.tracks_dir());
        routes.refresh_metadata().map_err(|error| format!("route metadata: {error:?}"))?;
        Ok(Self { map, routes, trips, rides, tracks })
    }
}

fn extension(path: &Path, expected: &str) -> bool {
    path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case(expected))
}

fn input_files(directory: &Path, required: bool) -> Result<Vec<PathBuf>, String> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if !required && error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("read {}: {error}", directory.display())),
    };
    let mut files = entries
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    files.sort();
    Ok(files)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use obc_formats::io::ByteSource;
    use obc_host_core::TripCatalog;
    const ROUTE: &[u8] = include_bytes!("../../../fixtures/sources/sim-grimsel/routes/grimsel-climb.obcr");
    const TRIP: &[u8] = include_bytes!("../../../fixtures/sources/sim-grimsel/routes/TP1.OBT");

    #[test]
    fn persistent_session_reopens_exact_sources_references_and_metadata_without_importing_again() {
        let directory = obcm_testkit::scratch::scratch_dir("native-card", "reopen");
        std::fs::write(directory.join("a.obcr"), ROUTE).unwrap();
        std::fs::write(directory.join("b.obcr"), ROUTE).unwrap();
        std::fs::write(directory.join("TP1.OBT"), TRIP).unwrap();
        let card = directory.join("card.obc").to_string_lossy().into_owned();
        let mut args = Args {
            map: concat!(env!("CARGO_MANIFEST_DIR"), "/assets/grimsel-demo.obcm").into(),
            create_card: Some(card.clone()),
            routes_dir: Some(directory.to_string_lossy().into_owned()),
            ..Args::default()
        };
        let mut session = Session::load(&mut args).unwrap();
        let map = session.map.map_source();
        let identity = (map.store_id(), map.id(), map.revision());
        let ids = session.routes.ids().to_vec();
        let trip_id = session.trips.inputs()[0].id;
        assert_eq!(session.trips.inputs()[0].stage_ids, &[ids[0], ids[1], 0]);
        assert_eq!(session.routes.store_scope(), session.trips.store_scope());
        assert_eq!(session.routes.source(ids[0]).unwrap().store_id(), identity.0);
        let old = session.routes.source(ids[0]).unwrap();
        session.routes.replace(ids[0], ROUTE).unwrap();
        assert!(!old.is_current());
        let mut old_bytes = vec![0; old.len() as usize];
        old.read_at(0, &mut old_bytes).unwrap();
        assert_eq!(old_bytes, ROUTE);
        assert!(Session::load(&mut args).is_err(), "create refuses an existing path");
        drop(session);
        drop(map);
        assert!(persistent(&card, false).is_err(), "the old route lease retains the card lock");
        drop(old);
        let mut reopen = Args { card: Some(card.clone()), ..Args::default() };
        let session = Session::load(&mut reopen).unwrap();
        let map = session.map.map_source();
        assert_eq!((map.store_id(), map.id(), map.revision()), identity);
        assert_eq!(session.routes.ids(), ids);
        assert_eq!(session.routes.source(ids[0]).unwrap().revision().0, 2);
        assert_eq!(session.trips.inputs()[0].id, trip_id);
        assert_eq!(session.trips.inputs()[0].stage_ids, &[ids[0], ids[1], 0]);
        assert_eq!(std::fs::read(directory.join("a.obcr")).unwrap(), ROUTE);
        assert_eq!(std::fs::read(directory.join("TP1.OBT")).unwrap(), TRIP);
        drop(map);
        drop(session);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn failed_create_leaves_evidence_and_reopen_never_initializes_it() {
        let directory = obcm_testkit::scratch::scratch_dir("native-card", "invalid");
        let card = directory.join("card.obc").to_string_lossy().into_owned();
        let mut args = Args {
            map: directory.join("missing.obcm").to_string_lossy().into_owned(),
            create_card: Some(card.clone()),
            ..Args::default()
        };
        assert!(Session::load(&mut args).is_err());
        let owner = persistent(&card, false).unwrap();
        let id = owner.store_id().unwrap();
        drop(owner);
        assert!(
            Session::load(&mut Args { card: Some(card.clone()), ..Args::default() }).is_err(),
            "a map-free card is not repaired"
        );
        let owner = persistent(&card, false).unwrap();
        assert_eq!(owner.store_id().unwrap(), id);
        drop(owner);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
