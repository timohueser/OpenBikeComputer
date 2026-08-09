//! The cycle orchestrator: one idempotent, stateless pass suitable for a systemd timer.
//!
//! Order is the whole safety story: read the previously published manifest (the only state that
//! exists), bake **every** selected product completely in memory, then upload frames, verify
//! them at the destination, and swap the manifest in last. Any failure before that final swap —
//! a corrupt upstream, a short body, a decode surprise, an upload error — publishes nothing and
//! leaves the previous manifest and its frames fully consistent. Unchanged upstream runs
//! short-circuit: their immutable frames are already published, so their previous manifest
//! entries are carried forward verbatim and no bytes move.
//!
//! A cycle may bake a **subset** of the products — that is exactly what the per-adapter systemd
//! timers (WX18) do, so one broken upstream costs only its own product's freshness. The manifest
//! is the whole service's state, so products this invocation did not select are carried forward
//! from the previous manifest verbatim, exactly like an unchanged one.
//!
//! **Expiry is handled uniformly and it is never a deletion.** A carried product past its own
//! `staleness_deadline` stays in the manifest — clients already refuse to use it, so it is
//! visibly, honestly expired rather than quietly absent — but its frames are *exempt from the
//! pre-swap fetchability proof*, because nothing may fetch them any more and the bucket's 48 h
//! lifecycle rule is entitled to have deleted them. Both halves matter:
//!
//! * dropping an expired entry instead would make the product's own next tick find no previous
//!   entry, lose its short-circuit (ETag / run identity), and re-fetch + re-publish the same
//!   stalled run — a re-download loop against an upstream that is already in trouble, and a
//!   manifest that flickers the product present/absent (and an external alarm that flaps with it);
//! * verifying an expired entry's frames instead would let one long-dead product's expired
//!   objects block every healthy product's publication once the lifecycle rule collects them.
//!
//! A product therefore leaves the manifest only when a human retires it (RUNBOOK.md § retiring an
//! adapter), never as a side effect of an outage.

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
    /// Not selected by this invocation; the previous entry was carried forward untouched.
    NotSelected,
}

#[derive(Debug)]
pub struct CycleReport {
    pub products: Vec<(String, ProductStatus, usize)>,
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
                ProductStatus::NotSelected => format!("{id}: not selected ({frames} published frames carried forward)"),
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
    let mut baked_ids: Vec<String> = Vec::new();
    let mut statuses = Vec::new();
    for adapter in adapters {
        let id = adapter.id();
        match adapter.bake(upstream, previous_product(id), now, &mut warnings)? {
            AdapterOutcome::Unchanged => {
                let carried = previous_product(id)
                    .ok_or_else(|| format!("{id}: adapter reported unchanged with no published entry"))?
                    .clone();
                statuses.push((adapter.id().to_string(), ProductStatus::Unchanged, carried.frames.len()));
                products.push(carried);
            }
            AdapterOutcome::Baked(baked) => {
                let emitted = emit::emit_product(&baked)?;
                let entries: Vec<_> = emitted.iter().map(|frame| frame.entry.clone()).collect();
                statuses.push((baked.id.to_string(), ProductStatus::Baked, entries.len()));
                for frame in emitted {
                    frame_objects.push(PlannedObject {
                        key: frame.key,
                        bytes: frame.bytes,
                        cache_control: publish::FRAME_CACHE_CONTROL,
                        content_type: "application/octet-stream",
                    });
                }
                baked_ids.push(baked.id.to_string());
                products.push(emit::product_entry(&baked, entries, now));
            }
        }
    }

    // Products no adapter in this invocation owns: carry the previous entry forward untouched.
    // Never drop one — see the module comment: dropping costs the product's own next tick its
    // short-circuit and turns a stalled upstream into a re-download loop plus a flapping manifest.
    if let Some(previous) = previous.as_ref() {
        for product in &previous.products {
            if products.iter().any(|selected| selected.id == product.id) {
                continue;
            }
            statuses.push((product.id.clone(), ProductStatus::NotSelected, product.frames.len()));
            products.push(product.clone());
        }
    }
    // One manifest, one order, whoever baked it: sort so a per-adapter cycle and a full cycle
    // produce the same document for the same inputs.
    products.sort_by(|left, right| left.id.cmp(&right.id));

    // What the publisher must prove fetchable before it swears the manifest is true: every frame
    // of every carried product that a client is still allowed to read. An expired product's
    // frames are deliberately exempt — no client may fetch them, and the 48 h lifecycle rule is
    // entitled to have collected them, so demanding they still exist would let one dead product
    // block every live one. Freshly baked frames are proven by `publish` itself.
    let mut carried_frames: Vec<(String, u64)> = Vec::new();
    for product in &products {
        if baked_ids.iter().any(|id| id == &product.id) {
            continue;
        }
        match manifest::parse_rfc3339(&product.staleness_deadline) {
            Some(deadline) if deadline > now => {
                carried_frames.extend(product.frames.iter().map(|frame| (frame.key.clone(), frame.bytes)));
            }
            Some(_) => warnings.push(format!(
                "{}: carried past its staleness deadline {} — expired for every client; its frames are no longer verified",
                product.id, product.staleness_deadline
            )),
            None => warnings.push(format!(
                "{}: staleness deadline {} is unreadable — carried, and treated as expired",
                product.id, product.staleness_deadline
            )),
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
