//! Measure the shared SQLite overlay query and serialized renderer response.
use route_engine::package::digest;
use route_server::Overlays;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    ffi::{c_char, CStr},
    path::Path,
    time::Instant,
};

fn run(root: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    let reference: Value = serde_json::from_slice(&std::fs::read(root.join("reference.json"))?)?;
    let manifest = std::fs::read(root.join("manifest.json"))?;
    if digest(&manifest) != reference["package"].as_str().ok_or("Missing package identity")? {
        return Err("Overlay package identity differs".into());
    }
    let mut samples = Vec::new();
    let mut mismatches = Vec::new();
    let start = Instant::now();
    let overlays = Overlays::open(root, &digest(&manifest))?;
    let initialization_ms = start.elapsed().as_secs_f64() * 1000.0;
    for iteration in 0..3 {
        for case in reference["cases"].as_array().ok_or("Missing cases")? {
            let params: HashMap<String, String> = serde_json::from_value(case["params"].clone())?;
            let started = Instant::now();
            let encoded = overlays.query(&params, &|| started.elapsed().as_secs() >= 15)?;
            let query_ms = started.elapsed().as_secs_f64() * 1000.0;
            let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
            let fingerprint = digest(&encoded);
            if fingerprint != case["sha256"].as_str().ok_or("Missing response identity")? {
                mismatches.push(json!({"case": case["name"], "iteration": iteration, "actual": fingerprint}));
            }
            samples.push(json!({"case": case["name"], "iteration": iteration,
                "query_ms": query_ms, "elapsed_ms": elapsed_ms, "bytes": encoded.len(),
                "sha256": fingerprint}));
        }
    }
    Ok(json!({"initialization_ms": initialization_ms, "samples": samples, "mismatches": mismatches,
        "scope": "SQLite query and complete JSON serialization; excludes renderer and network"}))
}

/// # Safety
/// The host retains both NUL-terminated UTF-8 paths until the call returns.
#[no_mangle]
pub unsafe extern "C" fn planner_overlay_benchmark(root: *const c_char, output: *const c_char) -> i32 {
    if root.is_null() || output.is_null() {
        return 1;
    }
    let execute = || -> Result<(), Box<dyn std::error::Error>> {
        // SAFETY: The caller retains these C strings for this synchronous call.
        let (root, output) = unsafe { (CStr::from_ptr(root).to_str()?, CStr::from_ptr(output).to_str()?) };
        let report = run(Path::new(root))?;
        std::fs::write(output, serde_json::to_vec(&report)?)?;
        if report["mismatches"].as_array().is_none_or(|values| !values.is_empty()) {
            return Err("Overlay response mismatch".into());
        }
        Ok(())
    };
    match std::panic::catch_unwind(execute) {
        Ok(Ok(())) => 0,
        Ok(Err(error)) => {
            eprintln!("{error}");
            1
        }
        Err(_) => 2,
    }
}
