//! `planner-service INSTALL_DIRECTORY --missing|--install`: the installer of the routing cells
//! that `obc data serve` runs on the objects of its store.

use planner_router::cells::{Cell, Header, Part};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// One zstd frame of one raw block, with the content size.
fn frame(bytes: &[u8]) -> Vec<u8> {
    let mut frame = vec![0x28, 0xB5, 0x2F, 0xFD, 0x20, bytes.len() as u8];
    frame.extend(&(1 | (bytes.len() as u32) << 3).to_le_bytes()[..3]);
    frame.extend(bytes);
    frame
}

fn hex(hash: &[u8]) -> String {
    hash.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn run(args: &[&Path], action: &str, extra: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_planner-service"));
    command.arg(args[0]).arg(action).args(&args[1..]).args(extra);
    command.output().unwrap()
}

#[test]
fn an_install_from_the_store_goes_on_after_a_stop() {
    let scratch = std::env::temp_dir().join(format!("planner-service-install-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let (target, objects) = (scratch.join("route-cells"), scratch.join("objects"));
    // Two download cells of one level, each with a topology and a geometry part.
    let mut published: Vec<(PathBuf, Vec<u8>)> = Vec::new();
    let mut cells = Vec::new();
    for id in 0..2u32 {
        let parts = (0..2)
            .map(|part| {
                let bytes = format!("cell {id} part {part}").into_bytes();
                let object = frame(&bytes);
                let sha256: [u8; 32] = Sha256::digest(&object).into();
                published.push((objects.join(&hex(&sha256)[..2]).join(hex(&sha256)), object.clone()));
                Part {
                    object: sha256,
                    object_bytes: object.len() as u32,
                    sha256: Sha256::digest(&bytes).into(),
                    bytes: bytes.len() as u32,
                }
            })
            .collect();
        cells.push(Cell { level: 1, id, first_road: id, roads: 1, parts, ..Cell::default() });
    }
    let info = br#"{"profiles": []}"#;
    let mut directory = Vec::new();
    let header = Header {
        levels: 1,
        download_level: 1,
        groups: 0,
        cells: 2,
        roads: 2,
        info: Sha256::digest(info).into(),
        ..Header::default()
    };
    Header { level_bits: [1, 0, 0, 0, 0, 0, 0, 0], ..header }.encode(&mut directory);
    cells.iter().for_each(|cell| cell.encode(&mut directory));
    let (directory_path, info_path) = (scratch.join("directory.bin"), scratch.join("package.json"));
    std::fs::create_dir_all(&scratch).unwrap();
    std::fs::write(&directory_path, &directory).unwrap();
    std::fs::write(&info_path, info).unwrap();
    let publish = |objects: &[(PathBuf, Vec<u8>)]| {
        for (path, bytes) in objects {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
    };
    let args = [target.as_path(), &directory_path, &info_path];
    let missing = || String::from_utf8(run(&args, "--missing", &[]).stdout).unwrap().trim().parse::<u64>().unwrap();
    let install = || run(&[target.as_path(), &directory_path, &info_path, &objects], "--install", &["2"]);
    assert_eq!(missing(), 4 * 13);

    // The store lacks the objects of the second cell: the install stops with the first.
    publish(&published[..2]);
    let stopped = install();
    let said = String::from_utf8_lossy(&stopped.stderr);
    assert!(!stopped.status.success() && said.starts_with("object "), "{said}");
    let files: Vec<String> = std::fs::read_dir(target.join("cells"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(files.len(), 2, "{files:?}");
    assert!(files.iter().all(|name| name.starts_with("1-00000000-")), "{files:?}");
    assert!(!target.join("install.bin").exists());
    assert_eq!(missing(), 2 * 13);

    publish(&published[2..]);
    let done = install();
    let said = String::from_utf8_lossy(&done.stderr);
    assert!(done.status.success() && said.starts_with("Installed 2 files of 26 bytes"), "{said}");
    assert!(target.join("install.bin").is_file());
    assert_eq!(missing(), 0);
    std::fs::remove_dir_all(&scratch).unwrap();
}
