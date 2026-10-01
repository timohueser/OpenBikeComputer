//! Offline query: read one JSON request on stdin and write its response to stdout.
use route_engine::{directory::Directory, Control, Request, Router};
use std::io::Read;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("Usage: query PACKAGE_DIRECTORY < request.json")?;
    let package = Directory::open(std::path::Path::new(&path))?;
    let mut bytes = Vec::new();
    std::io::stdin().take(65_537).read_to_end(&mut bytes)?;
    if bytes.len() > 65_536 {
        return Err("Request exceeds 64 KiB".into());
    }
    let request: Request = serde_json::from_slice(&bytes)?;
    let response = Router::new(package, 768 * 1024 * 1024).routes(&request, &Control::default())?;
    serde_json::to_writer(std::io::stdout(), &response)?;
    Ok(())
}
