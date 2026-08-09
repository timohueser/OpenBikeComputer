//! The cycle orchestrator: one idempotent, stateless pass suitable for a systemd timer.
//!
//! Order is the whole safety story: read the previously published manifest (the only state that
//! exists), bake **every** selected product completely in memory, then upload frames, verify
//! them at the destination, and swap the manifest in last. Any failure before that final swap —
//! a corrupt upstream, a short body, a decode surprise, an upload error — publishes nothing and
//! leaves the previous manifest and its frames fully consistent. Unchanged upstream runs
//! short-circuit: their immutable frames are already published, so their previous manifest
//! entries are carried forward verbatim and no bytes move.

use std::time::Instant;

use crate::emit;
use crate::fetch::Upstream;
use crate::manifest::{self, Manifest};
use crate::publish::{self, ObjectStore, PlannedObject};
use crate::source::{Adapter, AdapterOutcome};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductStatus {
    /// Freshly baked and published this cycle.
    Baked,
    /// Upstream unchanged; the previous entry was carried forward.
    Unchanged,
}

#[derive(Debug)]
pub struct CycleReport {
    pub products: Vec<(&'static str, ProductStatus, usize)>,
    pub fetched_bytes: u64,
    pub published_objects: usize,
    pub published_bytes: u64,
    pub elapsed_ms: u128,
    pub warnings: Vec<String>,
}

impl CycleReport {
    pub fn summary(&self) -> String {
        let mut lines = Vec::new();
        for (id, status, frames) in &self.products {
            lines.push(match status {
                ProductStatus::Baked => format!("{id}: baked {frames} frames"),
                ProductStatus::Unchanged => format!("{id}: upstream unchanged ({frames} published frames stand)"),
            });
        }
        lines.push(format!(
            "fetched {} upstream bytes; published {} objects / {} bytes; {} ms",
            self.fetched_bytes, self.published_objects, self.published_bytes, self.elapsed_ms
        ));
        for warning in &self.warnings {
            lines.push(format!("warning: {warning}"));
        }
        lines.join("\n")
    }
}

pub fn run_cycle(
    adapters: &[&dyn Adapter],
    upstream: &mut dyn Upstream,
    store: &mut dyn ObjectStore,
    now: i64,
    dry_run: bool,
) -> Result<CycleReport, String> {
    let started = Instant::now();
    let mut warnings = Vec::new();

    // The previously published manifest is the cycle's only state. A corrupt one must not wedge
    // the service forever: warn and rebuild from scratch (every bake is idempotent anyway).
    let previous = match store.get(manifest::MANIFEST_KEY)? {
        None => None,
        Some(bytes) => match manifest::from_json(&bytes) {
            Ok(previous) => Some(previous),
            Err(error) => {
                warnings.push(format!("published manifest is unreadable ({error}); rebaking everything"));
                None
            }
        },
    };
    let previous_product =
        |id: &str| previous.as_ref().and_then(|manifest| manifest.products.iter().find(|product| product.id == id));

    // Bake everything before publishing anything.
    let mut products = Vec::new();
    let mut frame_objects: Vec<PlannedObject> = Vec::new();
    let mut carried_frames: Vec<(String, u64)> = Vec::new();
    let mut statuses = Vec::new();
    for adapter in adapters {
        let id = adapter.id();
        match adapter.bake(upstream, previous_product(id), now, &mut warnings)? {
            AdapterOutcome::Unchanged => {
                let carried = previous_product(id)
                    .ok_or_else(|| format!("{id}: adapter reported unchanged with no published entry"))?
                    .clone();
                statuses.push((adapter.id(), ProductStatus::Unchanged, carried.frames.len()));
                carried_frames.extend(carried.frames.iter().map(|frame| (frame.key.clone(), frame.bytes)));
                products.push(carried);
            }
            AdapterOutcome::Baked(baked) => {
                let emitted = emit::emit_product(&baked)?;
                let entries: Vec<_> = emitted.iter().map(|frame| frame.entry.clone()).collect();
                statuses.push((baked.id, ProductStatus::Baked, entries.len()));
                for frame in emitted {
                    frame_objects.push(PlannedObject {
                        key: frame.key,
                        bytes: frame.bytes,
                        cache_control: publish::FRAME_CACHE_CONTROL,
                        content_type: "application/octet-stream",
                    });
                }
                products.push(emit::product_entry(&baked, entries, now));
            }
        }
    }

    let document = Manifest { version: manifest::MANIFEST_VERSION, generated_at: manifest::rfc3339(now), products };
    let manifest_object = PlannedObject {
        key: manifest::MANIFEST_KEY.to_string(),
        bytes: manifest::to_json(&document).into_bytes(),
        cache_control: publish::MANIFEST_CACHE_CONTROL,
        content_type: "application/json",
    };

    let (published_objects, published_bytes) =
        if dry_run { (0, 0) } else { publish::publish(store, &frame_objects, &carried_frames, &manifest_object)? };

    Ok(CycleReport {
        products: statuses,
        fetched_bytes: upstream.fetched_bytes(),
        published_objects,
        published_bytes,
        elapsed_ms: started.elapsed().as_millis(),
        warnings,
    })
}
