use route_engine::{
    directory::Directory,
    package::{digest, Package, Source},
    Control, Request, Router,
};
use serde::{Deserialize, Serialize};
use std::{cell::Cell, path::Path, rc::Rc, time::Instant};

#[derive(Deserialize)]
pub struct Case {
    pub name: String,
    pub request: Request,
}

#[derive(Default)]
struct Reads {
    objects: Cell<u64>,
    bytes: Cell<u64>,
}

struct Counted {
    source: Directory,
    reads: Rc<Reads>,
}

impl Source for Counted {
    fn read(&self, digest: &str) -> route_engine::Result<Vec<u8>> {
        let bytes = self.source.read(digest)?;
        self.reads.objects.set(self.reads.objects.get() + 1);
        self.reads.bytes.set(self.reads.bytes.get() + bytes.len() as u64);
        Ok(bytes)
    }
}

#[derive(Serialize)]
pub struct Sample {
    pub name: String,
    pub profile: String,
    pub iteration: usize,
    pub cache: &'static str,
    pub elapsed_ms: f64,
    pub objects_read: u64,
    pub bytes_read: u64,
    pub route_count: usize,
    pub cost: Option<u64>,
    pub distance_m: Option<u64>,
    pub result_sha256: Option<String>,
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct Report {
    pub package: String,
    pub profiles: Vec<String>,
    pub memory_budget_bytes: usize,
    pub initialization_ms: f64,
    pub samples: Vec<Sample>,
}

#[derive(Default)]
pub struct Options {
    pub retained: bool,
    pub extra_index_memory: bool,
}

/// A cold sample starts a new router. It does not flush the operating system file cache.
pub fn run(
    path: &Path,
    cases: &[Case],
    iterations: usize,
    memory_budget_bytes: usize,
    options: Options,
) -> Result<Report, Box<dyn std::error::Error>> {
    if cases.is_empty() || iterations == 0 {
        return Err("Supply at least one request and iteration".into());
    }
    let started = Instant::now();
    let manifest = std::fs::read(path.join("manifest.json"))?;
    let source = Directory::source(path)?;
    let checked = Package::open(source.clone(), &manifest)?;
    let memory_budget_bytes = memory_budget_bytes.saturating_add(if options.extra_index_memory {
        checked.manifest().landmarks.as_ref().map_or(0, |index| index.decoded_bytes())
    } else {
        0
    });
    let mut report = Report {
        package: checked.identity().into(),
        profiles: checked.manifest().metrics.keys().cloned().collect(),
        memory_budget_bytes,
        initialization_ms: started.elapsed().as_secs_f64() * 1000.0,
        samples: Vec::new(),
    };
    let mut retained = None;
    for case in cases {
        for iteration in 0..iterations {
            let (mut router, reads) = if let Some(router) = retained.take() {
                router
            } else {
                let reads = Rc::new(Reads::default());
                let package = Package::open(Counted { source: source.clone(), reads: reads.clone() }, &manifest)?;
                let router = Router::new(package, memory_budget_bytes);
                (router, reads)
            };
            let mut request = case.request.clone();
            if options.retained {
                // A new coordinate forces a fresh request while the graph and profile caches stay open.
                if let Some(point) = request.points.first_mut() {
                    point[0] += iteration as f64 * 0.0001;
                }
            }
            let caches: &[_] =
                if options.retained { &["retained_fresh_request"] } else { &["cold_router", "warm_router"] };
            for &cache in caches {
                reads.objects.set(0);
                reads.bytes.set(0);
                let start = Instant::now();
                let deadline = || start.elapsed().as_secs() >= 30;
                let result = router.routes(&request, &Control { cancelled: &deadline, ..Control::default() });
                let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
                let mut sample = Sample {
                    name: case.name.clone(),
                    profile: case.request.profile.clone(),
                    iteration,
                    cache,
                    elapsed_ms,
                    objects_read: reads.objects.get(),
                    bytes_read: reads.bytes.get(),
                    route_count: 0,
                    cost: None,
                    distance_m: None,
                    result_sha256: None,
                    error: None,
                };
                match result {
                    Ok(response) => {
                        sample.route_count = response.routes.len();
                        sample.cost = response.routes.first().map(|r| r.cost);
                        sample.distance_m = response.routes.first().map(|r| r.totals.distance_m);
                        let mut value = route_engine::answer::answer(&response);
                        for route in value["routes"].as_array_mut().ok_or("Missing routes")? {
                            let route = route.as_object_mut().ok_or("Invalid route")?;
                            route.remove("id");
                            route.remove("package");
                        }
                        sample.result_sha256 = Some(digest(&serde_json::to_vec(&value)?));
                    }
                    Err(error) => sample.error = Some(error.to_string()),
                }
                report.samples.push(sample);
            }
            if options.retained {
                retained = Some((router, reads));
            }
        }
        eprintln!("Completed {} / {}", case.request.profile, case.name);
    }
    Ok(report)
}
