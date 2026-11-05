//! The native times of `model_times` over a `requests` folder, the best of three runs per cell:
//! `cell graph-ms near-ms far-ms near-words far-words`. Prototype timing of the one road model.

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("usage: model_times DIR"));
    let index: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("index.json")).unwrap()).unwrap();
    for entry in index.as_array().unwrap() {
        let bytes = std::fs::read(dir.join(entry["file"].as_str().unwrap())).unwrap();
        let best = |far: bool| {
            (0..3)
                .map(|_| obc_ground_plan_bridge::model_times(&bytes, far).unwrap())
                .fold([f64::MAX; 3], |a, t| [a[0].min(t[0]), a[1].min(t[1]), t[2]])
        };
        let (n, f) = (best(false), best(true));
        let cell = &entry["cell"];
        println!("{}-{} {:.3} {:.3} {:.3} {} {}", cell[0], cell[1], n[0].min(f[0]), n[1], f[1], n[2], f[2]);
    }
}
