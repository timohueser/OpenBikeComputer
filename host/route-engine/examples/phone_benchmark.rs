//! The phone harness calls the same runner as the desktop benchmark.
#[path = "support/benchmark.rs"]
mod benchmark;
use std::ffi::{c_char, CStr};

/// # Safety
/// Each argument must point to a valid NUL-terminated UTF-8 string for this call.
#[no_mangle]
pub unsafe extern "C" fn planner_benchmark(root: *const c_char, requests: *const c_char, output: *const c_char) -> i32 {
    if root.is_null() || requests.is_null() || output.is_null() {
        return 1;
    }
    let run = || -> Result<(), Box<dyn std::error::Error>> {
        // SAFETY: The host retains these three C strings until this synchronous call returns.
        let (root, requests, output) = unsafe {
            (CStr::from_ptr(root).to_str()?, CStr::from_ptr(requests).to_str()?, CStr::from_ptr(output).to_str()?)
        };
        let cases: Vec<benchmark::Case> = serde_json::from_slice(&std::fs::read(requests)?)?;
        let report = benchmark::run(std::path::Path::new(root), &cases, 3, 768 * 1024 * 1024)?;
        std::fs::write(output, serde_json::to_vec(&report)?)?;
        Ok(())
    };
    match std::panic::catch_unwind(run) {
        Ok(Ok(())) => 0,
        Ok(Err(error)) => {
            eprintln!("{error}");
            1
        }
        Err(_) => 2,
    }
}
