//! Offline query: read one JSON request on stdin and write its route answer to stdout.
use planner_router::{data::RoutingData, Control, Request, Router};
use std::io::Read;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("Usage: query PACKAGE_DIRECTORY < request.json")?;
    let package = planner_router::open(std::path::Path::new(&path))?;
    let mut bytes = Vec::new();
    std::io::stdin().take(65_537).read_to_end(&mut bytes)?;
    if bytes.len() > 65_536 {
        return Err("Request exceeds 64 KiB".into());
    }
    let request: Request = serde_json::from_slice(&bytes)?;
    let budget = package.default_budget();
    let response = Router::new(package, budget).routes(&request, &Control::default())?;
    serde_json::to_writer(std::io::stdout(), &planner_router::answer::answer(&response))?;
    Ok(())
}
