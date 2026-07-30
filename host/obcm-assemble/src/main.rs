//! `obcm-assemble` — the native driver for the assembly engine.
//!
//! Everything here is I/O and argument parsing. The crate's rule is that the **engine** never
//! touches a filesystem (it has to run in a browser tab, #1024/P4), so this file owns every
//! `std::fs` call: it opens cell artifacts as [`ByteSource`]s, implements the [`ShardStore`] over
//! real files, and prints what the engine reports.
//!
//! ```text
//! obcm-assemble --cells <cells.json> --skin <skin.json> --out <dir> [options]
//! ```
//!
//! `cells.json` is the cutter's provenance sidecar (`obc-pack cut`), which already states every
//! cell's band, path and `partial` flag plus the schema they were baked at — so the common case
//! needs no second document. `--schema` overrides it (an OBCC v2 root or a bare `SchemaEntry`),
//! which is what a hosted catalog hands in.

use std::cell::RefCell;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use obc_formats::io::{ByteSource, Error as IoError};
use obcm_assemble::grid::CellId;
use obcm_assemble::schema::{Schema, Skin};
use obcm_assemble::{assemble, CellInput, Clock, Error, Options, Result, ShardPlan, ShardStore};

/// A cell artifact read on demand. Cell regions are copied in 256 KB blocks, so the whole tree never
/// has to be resident — which is what keeps a country assembly's memory about the nav graph rather
/// than about the geometry.
struct FileSource {
    file: RefCell<File>,
    len: u32,
}

impl FileSource {
    fn open(path: &Path) -> std::io::Result<FileSource> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        Ok(FileSource { file: RefCell::new(file), len: len as u32 })
    }
}

impl ByteSource for FileSource {
    fn read_at(&self, offset: u32, buf: &mut [u8]) -> std::result::Result<(), IoError> {
        let mut f = self.file.borrow_mut();
        f.seek(SeekFrom::Start(offset as u64)).map_err(|_| IoError::Io)?;
        f.read_exact(buf).map_err(|_| IoError::Io)
    }
    fn len(&self) -> u32 {
        self.len
    }
}

/// Shards as files under an output directory, with the §5.2 derived names. The manifest is written
/// **last** (§5.4), so an interrupted run leaves files no reader will mount.
struct FileStore {
    dir: PathBuf,
    card_id: u16,
    open: Option<std::io::BufWriter<File>>,
    sealed: Vec<FileSource>,
    manifest_path: PathBuf,
}

impl FileStore {
    fn new(dir: &Path, card_id: u16) -> Result<FileStore> {
        std::fs::create_dir_all(dir).map_err(|_| Error::Io(IoError::Io))?;
        Ok(FileStore {
            dir: dir.to_path_buf(),
            card_id,
            open: None,
            sealed: Vec::new(),
            manifest_path: dir.join(obcm_assemble::shard::manifest_filename(card_id)),
        })
    }
}

impl ShardStore for FileStore {
    fn begin(&mut self, plan: &ShardPlan) -> Result<()> {
        // A stale manifest must never point at shards being overwritten (§5.4).
        let _ = std::fs::remove_file(&self.manifest_path);
        let path = self.dir.join(obcm_assemble::shard::shard_filename(self.card_id, plan.index));
        let file = File::create(&path).map_err(|_| Error::Io(IoError::Io))?;
        self.open = Some(std::io::BufWriter::new(file));
        Ok(())
    }
    fn write(&mut self, buf: &[u8]) -> Result<()> {
        self.open.as_mut().expect("a shard is open").write_all(buf).map_err(|_| Error::Io(IoError::Io))
    }
    fn seal(&mut self) -> Result<()> {
        let mut w = self.open.take().expect("a shard is open");
        w.flush().map_err(|_| Error::Io(IoError::Io))?;
        drop(w);
        let path = self.dir.join(obcm_assemble::shard::shard_filename(self.card_id, self.sealed.len()));
        self.sealed.push(FileSource::open(&path).map_err(|_| Error::Io(IoError::Io))?);
        Ok(())
    }
    fn source(&self, index: usize) -> Result<&dyn ByteSource> {
        self.sealed.get(index).map(|s| s as &dyn ByteSource).ok_or(Error::Io(IoError::BadOffset))
    }
    fn manifest(&mut self, bytes: &[u8]) -> Result<()> {
        std::fs::write(&self.manifest_path, bytes).map_err(|_| Error::Io(IoError::Io))
    }
}

/// Wall-clock microseconds, for the phase split the engine reports.
struct StdClock(Instant);

impl Clock for StdClock {
    fn now_us(&self) -> u64 {
        self.0.elapsed().as_micros() as u64
    }
}

/// One cell as the cutter's sidecar states it.
struct SidecarCell {
    id: CellId,
    band: String,
    path: String,
    partial: bool,
}

fn parse_sidecar(text: &str) -> std::result::Result<(Schema, Vec<SidecarCell>), String> {
    let doc: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("cells.json: {e}"))?;
    let schema: Schema = serde_json::from_value(doc.get("schema").cloned().unwrap_or_default())
        .map_err(|e| format!("cells.json schema: {e}"))?;
    let cells = doc.get("cells").and_then(|c| c.as_array()).ok_or("cells.json has no `cells` array")?;
    let mut out = Vec::with_capacity(cells.len());
    for c in cells {
        let id = c.get("id").and_then(|v| v.as_str()).ok_or("a cell entry has no `id`")?;
        out.push(SidecarCell {
            id: CellId::parse(id)?,
            band: c.get("band").and_then(|v| v.as_str()).ok_or("a cell entry has no `band`")?.to_string(),
            path: c.get("path").and_then(|v| v.as_str()).ok_or("a cell entry has no `path`")?.to_string(),
            partial: c.get("partial").and_then(|v| v.as_bool()).unwrap_or(false),
        });
    }
    Ok((schema, out))
}

const USAGE: &str = "\
obcm-assemble — assemble baked OBCA cells into one .obcm or a volume set

USAGE:
    obcm-assemble --cells <cells.json> --skin <skin.json> --out <dir> [OPTIONS]

REQUIRED:
    --cells <path>          the cutter's provenance sidecar (obc-pack cut)
    --skin <path>           the skin to stamp (OBCC §11.4, or an id-keyed style table)
    --out <dir>             where the shards and the manifest are written

OPTIONS:
    --schema <path>         schema document (OBCC v2 root or SchemaEntry); default: the sidecar's
    --band <id>             only assemble these bands (repeatable)
    --cell <id>             only assemble these cells (repeatable, `<log2>/<i>/<j>`)
    --name <text>           the set's display name (24 bytes on the card)
    --card-id <n>           the id the derived filenames use (default 1)
    --target-shard-bytes <n>  split a geometry shard above this (default 1 GiB)
    --accept-holes          proceed although the selection has missing cells
    --accept-partial        proceed although a cell is `partial` (OBCA §3.7)
    --skip-verify           skip the §4.8 verify pass — BENCHMARKS ONLY, never for a device
    --json                  print the summary as JSON
";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> std::result::Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{USAGE}");
        return Ok(());
    }
    let mut cells_path: Option<PathBuf> = None;
    let mut schema_path: Option<PathBuf> = None;
    let mut skin_path: Option<PathBuf> = None;
    let mut out_dir: Option<PathBuf> = None;
    let mut only_bands: Vec<String> = Vec::new();
    let mut only_cells: Vec<CellId> = Vec::new();
    let mut json = false;
    let mut opts = Options::default();

    let mut i = 0;
    while i < args.len() {
        let value = |i: &mut usize| -> std::result::Result<String, String> {
            *i += 1;
            args.get(*i).cloned().ok_or_else(|| format!("{} needs a value", args[*i - 1]))
        };
        match args[i].as_str() {
            "--cells" => cells_path = Some(PathBuf::from(value(&mut i)?)),
            "--schema" => schema_path = Some(PathBuf::from(value(&mut i)?)),
            "--skin" => skin_path = Some(PathBuf::from(value(&mut i)?)),
            "--out" => out_dir = Some(PathBuf::from(value(&mut i)?)),
            "--band" => only_bands.push(value(&mut i)?),
            "--cell" => only_cells.push(CellId::parse(&value(&mut i)?)?),
            "--name" => opts.name = value(&mut i)?,
            "--card-id" => opts.card_id = value(&mut i)?.parse().map_err(|_| "--card-id takes a number")?,
            "--target-shard-bytes" => {
                opts.target_shard_bytes = value(&mut i)?.parse().map_err(|_| "--target-shard-bytes takes a number")?
            }
            "--accept-holes" => opts.accept_holes = true,
            "--accept-partial" => opts.accept_partial = true,
            "--skip-verify" => opts.skip_verify = true,
            "--json" => json = true,
            other => return Err(format!("unknown argument {other:?}\n\n{USAGE}")),
        }
        i += 1;
    }

    let cells_path = cells_path.ok_or("--cells is required")?;
    let skin_path = skin_path.ok_or("--skin is required")?;
    let out_dir = out_dir.ok_or("--out is required")?;
    let root = cells_path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let sidecar = std::fs::read_to_string(&cells_path).map_err(|e| format!("read {}: {e}", cells_path.display()))?;
    let (sidecar_schema, listed) = parse_sidecar(&sidecar)?;
    let schema = match &schema_path {
        None => sidecar_schema,
        Some(p) => Schema::parse(&std::fs::read_to_string(p).map_err(|e| format!("read {}: {e}", p.display()))?)?,
    };
    let skin =
        Skin::parse(&std::fs::read_to_string(&skin_path).map_err(|e| format!("read {}: {e}", skin_path.display()))?)?;

    // Open every selected cell. The sources outlive the assembly, so they are held here.
    let selected: Vec<&SidecarCell> = listed
        .iter()
        .filter(|c| only_bands.is_empty() || only_bands.contains(&c.band))
        .filter(|c| only_cells.is_empty() || only_cells.contains(&c.id))
        .collect();
    if selected.is_empty() {
        return Err("the selection is empty".into());
    }
    let mut sources: Vec<FileSource> = Vec::with_capacity(selected.len());
    for c in &selected {
        let path = root.join(&c.path);
        sources.push(FileSource::open(&path).map_err(|e| format!("open {}: {e}", path.display()))?);
    }
    let inputs: Vec<CellInput<'_>> = selected
        .iter()
        .zip(&sources)
        .map(|(c, src)| CellInput { id: c.id, band: c.band.clone(), src, partial: c.partial })
        .collect();

    let mut store = FileStore::new(&out_dir, opts.card_id).map_err(|e| e.to_string())?;
    let clock = StdClock(Instant::now());
    let summary = assemble(inputs, &schema, &skin, &opts, &mut store, &clock).map_err(|e| e.to_string())?;

    if json {
        println!("{}", summary_json(&summary, &out_dir));
    } else {
        print_summary(&summary, &out_dir);
    }
    Ok(())
}

fn print_summary(s: &obcm_assemble::Summary, out_dir: &Path) {
    let (min_lon, min_lat, max_lon, max_lat) = s.assembly_box.ubox();
    println!(
        "assembly bbox: lat [{min_lat}, {max_lat}) × lon [{min_lon}, {max_lon})  (2^{} µdeg square)",
        s.assembly_box.span_log2
    );
    println!("{} cell(s) → {} shard(s), {} bytes total", s.stats.cells, s.shards.len(), s.bytes);
    for sh in &s.shards {
        let v = sh
            .verify
            .as_ref()
            .map(|r| {
                format!(
                    "  verified: {} chunk(s), {} feature(s), {} nav node(s), largest component {:.1}%",
                    r.chunks,
                    r.features,
                    r.nav_nodes,
                    r.largest_component_permille as f64 / 10.0
                )
            })
            .unwrap_or_else(|| "  NOT VERIFIED (--skip-verify)".into());
        println!("  [{}] {:8} {:>12} B  {}\n{v}", sh.index, sh.role.as_str(), sh.bytes, sh.filename);
    }
    println!("manifest: {}", out_dir.join(&s.manifest_filename).display());
    let st = &s.stats;
    println!(
        "phases (ms): open {:.1} · poi {:.1} · nav {:.1} · plan {:.1} · write(+graft) {:.1} · verify {:.1} · total \
         {:.1}",
        st.open_us as f64 / 1000.0,
        st.poi_us as f64 / 1000.0,
        st.nav_us as f64 / 1000.0,
        st.plan_us as f64 / 1000.0,
        st.write_us as f64 / 1000.0,
        st.verify_us as f64 / 1000.0,
        st.total_us as f64 / 1000.0
    );
    println!(
        "geometry copied: {} B · nav section: {} B ({} node(s), {} edge(s), {} unified at seams, {} island node(s) \
         pruned) · POIs: {} ({} duplicate(s))",
        st.geometry_bytes,
        st.nav_section_bytes,
        st.nav.nodes,
        st.nav.edges,
        st.nav.unified,
        st.nav.pruned_nodes,
        st.poi_records,
        st.poi_duplicates
    );
}

fn summary_json(s: &obcm_assemble::Summary, out_dir: &Path) -> String {
    let shards: Vec<serde_json::Value> = s
        .shards
        .iter()
        .map(|sh| {
            serde_json::json!({
                "index": sh.index,
                "role": sh.role.as_str(),
                "file": sh.filename,
                "bytes": sh.bytes,
                "sha256": sh.sha256.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            })
        })
        .collect();
    let st = &s.stats;
    serde_json::to_string_pretty(&serde_json::json!({
        "assembly_bbox_udeg": {
            "min_lat": s.assembly_box.min_lat,
            "min_lon": s.assembly_box.min_lon,
            "span_log2": s.assembly_box.span_log2,
        },
        "cells": st.cells,
        "bytes": s.bytes,
        "shards": shards,
        "manifest": out_dir.join(&s.manifest_filename).display().to_string(),
        "phases_us": {
            "open": st.open_us, "poi": st.poi_us, "nav": st.nav_us,
            "plan": st.plan_us, "write": st.write_us, "verify": st.verify_us, "total": st.total_us,
        },
        "geometry_bytes": st.geometry_bytes,
        "nav": {
            "section_bytes": st.nav_section_bytes,
            "cell_nodes": st.nav.cell_nodes,
            "nodes": st.nav.nodes,
            "edges": st.nav.edges,
            "unified": st.nav.unified,
            "duplicate_edges": st.nav.duplicate_edges,
            "components_found": st.nav.components_found,
            "components_kept": st.nav.components_kept,
            "pruned_nodes": st.nav.pruned_nodes,
            "pruned_edges": st.nav.pruned_edges,
            "largest_component_permille": st.nav.largest_component_permille,
        },
        "poi": { "records": st.poi_records, "duplicates": st.poi_duplicates, "section_bytes": st.poi_section_bytes },
    }))
    .unwrap_or_else(|_| "{}".into())
}
