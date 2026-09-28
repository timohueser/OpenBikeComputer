//! Shared-cache, fixed-profile benchmark service. Cache exhaustion is an error; no eviction.
use crate::model::{Pace, Point, Profile, Road, Totals};
use crate::search::{Progress, Search};
use crate::storage::{self, Cache, Seed, MAX_PAGE_BYTES, NODES_PER_PAGE, ROADS_PER_PAGE};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::mem::size_of;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct Config {
    pub root: PathBuf,
    pub profile: Profile,
    pub ch_cache_bytes: usize,
    pub geometry_cache_bytes: usize,
    pub max_deadline: Duration,
    pub max_labels: usize,
    pub max_geometry_points: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteRequest {
    pub start: Seed,
    pub end: Seed,
    #[serde(default)]
    pub pace: Pace,
    pub deadline_ms: Option<u64>,
    pub max_labels: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct Failure {
    pub kind: &'static str,
    pub message: String,
}

impl Failure {
    fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }
}

type Result<T> = std::result::Result<T, Failure>;

#[derive(Debug, Default, Serialize)]
pub struct QueryMetrics {
    pub elapsed_ms: f64,
    pub search_ms: f64,
    pub ch_misses: usize,
    pub geometry_misses: usize,
    pub disk_bytes: usize,
    pub ch_cache_bytes: usize,
    pub geometry_cache_bytes: usize,
    pub labels: usize,
}

#[derive(Debug, Serialize)]
pub struct RouteResponse {
    pub kind: &'static str,
    pub profile: String,
    pub cost: u64,
    /// The incoming seed road is not traversed and is omitted.
    pub roads: Vec<u32>,
    /// Integer microdegrees and elevation meters; i16::MIN denotes missing elevation.
    pub geometry: Vec<Point>,
    pub totals: Totals,
    pub metrics: QueryMetrics,
    pub alternatives_supported: bool,
    pub eta_model: &'static str,
}

#[derive(Default)]
struct GeometryCache {
    pages: HashMap<u32, Arc<Vec<Road>>>,
    bytes: usize,
}

pub struct Router {
    pub config: Config,
    node_count: u64,
    ch: RwLock<Cache>,
    geometry: RwLock<GeometryCache>,
}

impl Router {
    pub fn open(config: Config) -> Result<Self> {
        if config.max_deadline.is_zero() || config.max_labels == 0 || config.max_geometry_points == 0 {
            return Err(Failure::new("invalid", "Service limits must be positive"));
        }
        let manifest: serde_json::Value = serde_json::from_slice(&read_page(&config.root.join("ch/manifest.json"))?)
            .map_err(|e| Failure::new("data_error", e.to_string()))?;
        let node_count =
            manifest["nodes"].as_u64().ok_or_else(|| Failure::new("data_error", "Missing graph node count"))?;
        if manifest["format"] != 1 || manifest["nodes_per_page"] != NODES_PER_PAGE || node_count > u32::MAX as u64 {
            return Err(Failure::new("data_error", "Unsupported graph manifest"));
        }
        if !config.root.join("geometry").is_dir() {
            return Err(Failure::new("data_error", "Geometry directory is missing"));
        }
        Ok(Self {
            config,
            node_count,
            ch: RwLock::new(Cache::default()),
            geometry: RwLock::new(GeometryCache::default()),
        })
    }

    pub fn route(&self, request: &RouteRequest, cancelled: impl Fn() -> bool) -> Result<RouteResponse> {
        let started = Instant::now();
        let duration = request.deadline_ms.map(Duration::from_millis).unwrap_or(self.config.max_deadline);
        let max_labels = request.max_labels.unwrap_or(self.config.max_labels);
        if duration.is_zero()
            || duration > self.config.max_deadline
            || max_labels == 0
            || max_labels > self.config.max_labels
        {
            return Err(Failure::new("invalid", "Requested limits exceed server limits or are zero"));
        }
        if request.start.node as u64 >= self.node_count
            || request.end.node as u64 >= self.node_count
            || request.start.road as u64 >= self.node_count
            || request.end.road as u64 >= self.node_count
            || request.start.cost != 0
            || request.end.cost != 0
        {
            return Err(Failure::new("invalid", "Endpoint states must exist and seed costs must be zero"));
        }
        request.pace.validate().map_err(|e| Failure::new("invalid", e))?;
        let checkpoint = || {
            if cancelled() {
                Err(Failure::new("cancelled", "Client cancelled the query"))
            } else if started.elapsed() >= duration {
                Err(Failure::new("deadline", "Query deadline exceeded"))
            } else {
                Ok(())
            }
        };
        let mut metrics = QueryMetrics::default();
        let mut search = Search::new(&[request.start], &[request.end], max_labels);
        let (cost, mut roads) = loop {
            checkpoint()?;
            let progress = search.poll(&self.ch.read().unwrap(), 512);
            match progress {
                Progress::Working { .. } => (),
                Progress::NeedPages { pages } => {
                    for id in pages {
                        checkpoint()?;
                        let mut cache = self.ch.write().unwrap();
                        checkpoint()?;
                        if cache.pages.contains_key(&id) {
                            continue;
                        }
                        let bytes = read_page(&self.config.root.join(format!("ch/{id}.bin")))?;
                        cache
                            .insert_page(id, &bytes, self.config.ch_cache_bytes)
                            .map_err(|e| Failure::new(if e.contains("budget") { "limit" } else { "data_error" }, e))?;
                        metrics.ch_misses += 1;
                        metrics.disk_bytes += bytes.len();
                    }
                }
                Progress::Done { cost, roads, labels } => {
                    metrics.labels = labels;
                    break (cost, roads);
                }
                Progress::NoPath => return Err(Failure::new("no_path", "No path connects these states")),
                Progress::Limit => return Err(Failure::new("limit", "Search or path budget exceeded")),
                Progress::Cancelled => return Err(Failure::new("cancelled", "Search cancelled")),
                Progress::Invalid { message } => return Err(Failure::new("data_error", message)),
            }
        };
        metrics.search_ms = started.elapsed().as_secs_f64() * 1000.0;
        if roads.first() != Some(&request.start.road) || roads.last() != Some(&request.end.road) {
            return Err(Failure::new("data_error", "Route witness disagrees with endpoint road IDs"));
        }
        roads.remove(0);
        let mut geometry: Vec<Point> = Vec::new();
        let mut totals = Totals::default();
        let mut previous_node = None;
        for &id in &roads {
            checkpoint()?;
            let page = self.geometry_page(id / ROADS_PER_PAGE, &mut metrics)?;
            let road = page
                .get((id % ROADS_PER_PAGE) as usize)
                .ok_or_else(|| Failure::new("data_error", "Geometry page does not contain requested road"))?;
            if road.shape.len() < 2 || previous_node.is_some_and(|node| road.from != node) {
                return Err(Failure::new("data_error", "Missing or disconnected route geometry"));
            }
            let skip = geometry.last().is_some_and(|p| {
                let next = road.shape[0];
                p.lat == next.lat && p.lon == next.lon && p.elevation == next.elevation
            }) as usize;
            if geometry.len() + road.shape.len() - skip > self.config.max_geometry_points {
                return Err(Failure::new("limit", "Response geometry point budget exceeded"));
            }
            geometry.extend_from_slice(&road.shape[skip..]);
            totals.add(road, &request.pace, self.config.profile.walking);
            previous_node = Some(road.to);
        }
        checkpoint()?;
        if !totals.seconds.is_finite() {
            return Err(Failure::new("invalid", "Pace causes non-finite travel time"));
        }
        metrics.ch_cache_bytes = self.ch.read().unwrap().decoded_bytes;
        metrics.geometry_cache_bytes = self.geometry.read().unwrap().bytes;
        metrics.elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        Ok(RouteResponse {
            kind: "done",
            profile: self.config.profile.name.clone(),
            cost,
            roads,
            geometry,
            totals,
            metrics,
            alternatives_supported: false,
            eta_model: "demonstration_gradient_curve",
        })
    }

    fn geometry_page(&self, id: u32, metrics: &mut QueryMetrics) -> Result<Arc<Vec<Road>>> {
        if let Some(page) = self.geometry.read().unwrap().pages.get(&id) {
            return Ok(page.clone());
        }
        let mut cache = self.geometry.write().unwrap();
        if let Some(page) = cache.pages.get(&id) {
            return Ok(page.clone());
        }
        let bytes = read_page(&self.config.root.join(format!("geometry/{id}.bin")))?;
        let (first, roads): (u32, Vec<Road>) = storage::decode(&bytes).map_err(|e| Failure::new("data_error", e))?;
        if id.checked_mul(ROADS_PER_PAGE) != Some(first) || roads.len() > ROADS_PER_PAGE as usize {
            return Err(Failure::new("data_error", "Wrong geometry page"));
        }
        let size = size_of::<Vec<Road>>()
            + roads.capacity() * size_of::<Road>()
            + roads.iter().map(|r| r.shape.capacity() * size_of::<Point>()).sum::<usize>();
        if size > self.config.geometry_cache_bytes.saturating_sub(cache.bytes) {
            return Err(Failure::new("limit", "Geometry cache budget exceeded"));
        }
        let page = Arc::new(roads);
        cache.bytes += size;
        cache.pages.insert(id, page.clone());
        metrics.geometry_misses += 1;
        metrics.disk_bytes += bytes.len();
        Ok(page)
    }
}

fn read_page(path: &std::path::Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|f| f.take((MAX_PAGE_BYTES + 1) as u64).read_to_end(&mut bytes))
        .map_err(|e| Failure::new("data_error", format!("{}: {e}", path.display())))?;
    if bytes.len() > MAX_PAGE_BYTES {
        return Err(Failure::new("limit", "Compressed file exceeds page budget"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Surface, BIKE};
    use crate::storage::{Node, Page};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn fixture() -> Fixture {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "trip-router-server-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("ch")).unwrap();
        fs::create_dir_all(root.join("geometry")).unwrap();
        fs::write(root.join("ch/manifest.json"), br#"{"format":1,"nodes":3,"nodes_per_page":128}"#).unwrap();
        let nodes = (0..3)
            .map(|i| Node {
                forward: if i < 2 {
                    vec![storage::Arc { to: i + 1, cost: (i + 1) as u64 * 10, children: None, road: i + 1 }]
                } else {
                    vec![]
                },
                backward: vec![],
            })
            .collect();
        fs::write(root.join("ch/0.bin"), storage::encode(&Page { first: 0, nodes }).unwrap()).unwrap();
        let a = Point { lat: 0, lon: 0, elevation: 0 };
        let b = Point { lat: 0, lon: 1000, elevation: 10 };
        let road = |from, to, length_m, shape| Road {
            from,
            to,
            way: 1,
            length_m,
            ascent_m: 0,
            descent_m: 0,
            surface: Surface::Paved,
            class: 0,
            access: BIKE,
            difficulty: 0,
            hiking_difficulty: None,
            uncertain_access: false,
            shape,
        };
        let roads = vec![road(2, 0, 999, vec![b, a]), road(0, 1, 100, vec![a, b]), road(1, 0, 200, vec![b, a])];
        fs::write(root.join("geometry/0.bin"), storage::encode(&(0u32, roads)).unwrap()).unwrap();
        Fixture(root)
    }

    fn config(f: &Fixture) -> Config {
        Config {
            root: f.0.clone(),
            profile: Profile::presets().remove(0),
            ch_cache_bytes: 1_000_000,
            geometry_cache_bytes: 1_000_000,
            max_deadline: Duration::from_secs(10),
            max_labels: 100,
            max_geometry_points: 100,
        }
    }

    fn request() -> RouteRequest {
        RouteRequest {
            start: Seed { node: 0, road: 0, cost: 0 },
            end: Seed { node: 2, road: 2, cost: 0 },
            pace: Pace::default(),
            deadline_ms: None,
            max_labels: None,
        }
    }

    #[test]
    fn shared_cache_preserves_routes_and_pace_only_changes_eta() {
        let fixture = fixture();
        let router = Arc::new(Router::open(config(&fixture)).unwrap());
        let cold = router.route(&request(), || false).unwrap();
        assert_eq!(cold.roads, [1, 2]);
        assert_eq!(cold.cost, 30);
        assert_eq!(cold.totals.distance_m, 300);
        assert_eq!(cold.geometry.iter().map(|p| p.lon).collect::<Vec<_>>(), [0, 1000, 0]);
        assert_eq!((cold.metrics.ch_misses, cold.metrics.geometry_misses), (1, 1));
        let cold = serde_json::to_value(&cold).unwrap();
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let router = router.clone();
                std::thread::spawn(move || router.route(&request(), || false).unwrap())
            })
            .collect();
        for handle in handles {
            let warm = handle.join().unwrap();
            assert_eq!((warm.metrics.ch_misses, warm.metrics.geometry_misses), (0, 0));
            let mut warm = serde_json::to_value(warm).unwrap();
            let mut expected = cold.clone();
            warm.as_object_mut().unwrap().remove("metrics");
            expected.as_object_mut().unwrap().remove("metrics");
            assert_eq!(warm, expected);
        }
        let mut slower = request();
        slower.pace.personal_multiplier = 2.0;
        let slower = router.route(&slower, || false).unwrap();
        assert_eq!(slower.cost, 30);
        assert_eq!(slower.roads, [1, 2]);
        assert_eq!(slower.totals.seconds, cold["totals"]["seconds"].as_f64().unwrap() * 2.0);
    }

    #[test]
    fn missing_pages_cancellation_and_budgets_are_failures() {
        let fixture = fixture();
        let mut missing = config(&fixture);
        missing.root = fixture.0.join("absent");
        assert_eq!(Router::open(missing).err().unwrap().kind, "data_error");
        let router = Router::open(config(&fixture)).unwrap();
        assert_eq!(router.route(&request(), || true).unwrap_err().kind, "cancelled");
        let mut limited = request();
        limited.max_labels = Some(2);
        assert_eq!(router.route(&limited, || false).unwrap_err().kind, "limit");
        let mut c = config(&fixture);
        c.ch_cache_bytes = 1;
        assert_eq!(Router::open(c).unwrap().route(&request(), || false).unwrap_err().kind, "limit");
        let mut c = config(&fixture);
        c.geometry_cache_bytes = 1;
        assert_eq!(Router::open(c).unwrap().route(&request(), || false).unwrap_err().kind, "limit");
        let mut c = config(&fixture);
        c.max_geometry_points = 2;
        assert_eq!(Router::open(c).unwrap().route(&request(), || false).unwrap_err().kind, "limit");
        let mut deadline = request();
        deadline.deadline_ms = Some(1);
        assert_eq!(
            router
                .route(&deadline, || {
                    std::thread::sleep(Duration::from_millis(2));
                    false
                })
                .unwrap_err()
                .kind,
            "deadline"
        );
        fs::remove_file(fixture.0.join("geometry/0.bin")).unwrap();
        assert_eq!(router.route(&request(), || false).unwrap_err().kind, "data_error");
        fs::remove_file(fixture.0.join("ch/0.bin")).unwrap();
        assert_eq!(Router::open(config(&fixture)).unwrap().route(&request(), || false).unwrap_err().kind, "data_error");
    }
}
