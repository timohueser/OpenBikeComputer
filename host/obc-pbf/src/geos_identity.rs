//! The two GEOS implementations loaded by this process, bound before producer work.

#[cfg(any(target_os = "macos", target_os = "linux"))]
use sha2::{Digest, Sha256};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::collections::BTreeSet;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::fs::File;
use std::fs::{self, Metadata};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Clone, Debug)]
pub struct Library {
    pub name: &'static str,
    pub path: PathBuf,
    pub sha256: String,
    stamp: String,
}

static BINDING: OnceLock<Result<[Library; 2], String>> = OnceLock::new();

/// Attempt this before a long-lived interface opens. A retained error blocks only GEOS work.
pub fn startup() {
    let _ = BINDING.get_or_init(discover);
}

pub fn libraries() -> Result<&'static [Library; 2], String> {
    let libraries = BINDING.get_or_init(discover).as_ref().map_err(Clone::clone)?;
    for library in libraries {
        if metadata(&library.path).map(|value| stamp(&value))? != library.stamp {
            return Err(format!("GEOS library {} changed; start a fresh worker", library.name));
        }
    }
    Ok(libraries)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn bind(name: &'static str, path: PathBuf) -> Result<Library, String> {
    let path = path.canonicalize().map_err(|error| format!("GEOS {}: {error}", path.display()))?;
    let before = stamp(&metadata(&path)?);
    let mut file = File::open(&path).map_err(|error| format!("GEOS {}: {error}", path.display()))?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let size = file.read(&mut buffer).map_err(|error| format!("GEOS {}: {error}", path.display()))?;
        if size == 0 {
            break;
        }
        hash.update(&buffer[..size]);
    }
    if stamp(&metadata(&path)?) != before {
        return Err("GEOS libraries changed at startup; start a fresh worker".into());
    }
    Ok(Library {
        name,
        path,
        sha256: hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect(),
        stamp: before,
    })
}

fn metadata(path: &Path) -> Result<Metadata, String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("GEOS {}: {error}; start a fresh worker", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("GEOS {} is not a library file", path.display()));
    }
    Ok(metadata)
}

fn stamp(metadata: &Metadata) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        format!(
            "{}:{}:{}:{}:{}:{}:{}",
            metadata.dev(),
            metadata.ino(),
            metadata.len(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec()
        )
    }
    #[cfg(not(unix))]
    format!("{}:{:?}", metadata.len(), metadata.modified())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn discover() -> Result<[Library; 2], String> {
    use std::ffi::CStr;
    use std::os::unix::ffi::OsStrExt;
    let _ = geos::version().map_err(|error| format!("GEOS: {error}"))?;
    let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
    // GEOSversion is supplied by the loaded C library; dladdr borrows its image name.
    if unsafe { libc::dladdr(geos::sys::GEOSversion as *const () as *const _, &mut info) } == 0
        || info.dli_fname.is_null()
    {
        return Err("Cannot identify loaded GEOS; use shared GEOS libraries and start a fresh worker".into());
    }
    let c = PathBuf::from(std::ffi::OsStr::from_bytes(unsafe { CStr::from_ptr(info.dli_fname) }.to_bytes()));
    let mut images = [BTreeSet::new(), BTreeSet::new()];
    for path in loaded_images() {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else { continue };
        let role = if name.starts_with("libgeos_c.") || name.starts_with("libgeos_c-") {
            0
        } else if name.starts_with("libgeos.") || name.starts_with("libgeos-") {
            1
        } else {
            continue;
        };
        images[role].insert(path.canonicalize().map_err(|error| format!("GEOS {}: {error}", path.display()))?);
    }
    let c = c.canonicalize().map_err(|error| format!("GEOS C library: {error}"))?;
    if images[0].len() != 1 || !images[0].contains(&c) || images[1].len() != 1 {
        return Err("Need one loaded shared GEOS C/C++ pair; correct the installation and start a fresh worker".into());
    }
    Ok([bind("geos-c", c)?, bind("geos-cpp", images[1].pop_first().expect("one C++ library"))?])
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn discover() -> Result<[Library; 2], String> {
    Err("GEOS producer binding requires shared libraries on macOS or Linux".into())
}

#[cfg(target_os = "macos")]
fn loaded_images() -> Vec<PathBuf> {
    use std::ffi::CStr;
    use std::os::unix::ffi::OsStrExt;
    unsafe extern "C" {
        fn _dyld_image_count() -> u32;
        fn _dyld_get_image_name(index: u32) -> *const libc::c_char;
    }
    (0..unsafe { _dyld_image_count() })
        .filter_map(|index| {
            let name = unsafe { _dyld_get_image_name(index) };
            (!name.is_null())
                .then(|| PathBuf::from(std::ffi::OsStr::from_bytes(unsafe { CStr::from_ptr(name) }.to_bytes())))
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn loaded_images() -> Vec<PathBuf> {
    unsafe extern "C" fn collect(
        info: *mut libc::dl_phdr_info,
        _: libc::size_t,
        paths: *mut libc::c_void,
    ) -> libc::c_int {
        use std::ffi::CStr;
        use std::os::unix::ffi::OsStrExt;
        // The loader owns info/name during this callback; paths points to our live Vec.
        let name = unsafe { (*info).dlpi_name };
        if !name.is_null() {
            unsafe { &mut *paths.cast::<Vec<PathBuf>>() }
                .push(PathBuf::from(std::ffi::OsStr::from_bytes(unsafe { CStr::from_ptr(name) }.to_bytes())));
        }
        0
    }
    let mut paths = Vec::new();
    unsafe { libc::dl_iterate_phdr(Some(collect), (&mut paths as *mut Vec<PathBuf>).cast()) };
    paths
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;

    #[test]
    fn startup_binds_the_loaded_c_and_cpp_images_by_complete_file_content() {
        startup();
        let pair = libraries().unwrap();
        assert_eq!([pair[0].name, pair[1].name], ["geos-c", "geos-cpp"]);
        assert_ne!(pair[0].path, pair[1].path);
        for library in pair {
            assert_eq!(library.path.canonicalize().unwrap(), library.path);
            assert_eq!(bind(library.name, library.path.clone()).unwrap().sha256, library.sha256);
        }
    }
}
