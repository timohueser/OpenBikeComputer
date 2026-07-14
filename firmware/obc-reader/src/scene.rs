//! Thin `Reader` adapter for the allocation-free semantic map-scene contract.

use heapless::Vec;
use obc_map_scene::{
    Candidate, CandidateReport, CapacityError as SceneCapacityError, DecodeReport, Diagnostics, Feature,
    FeatureError as SceneFeatureError, FeatureToken, MapScene, ReadError as SceneReadError, SelectedFeatures,
};

use crate::{BBox, CacheError, CapacityError, FeatureDecodeError, FeatureReadError, MapReadError, Reader};

#[inline]
fn read_error(error: MapReadError) -> SceneReadError {
    match error {
        MapReadError::Source(_) => SceneReadError::Source,
        MapReadError::Cache(CacheError::Busy) => SceneReadError::CacheBusy,
        MapReadError::Malformed => SceneReadError::Malformed,
    }
}

#[inline]
fn feature_error(error: FeatureReadError) -> SceneFeatureError {
    match error {
        FeatureReadError::Decode(FeatureDecodeError::Capacity(CapacityError::Points)) => {
            SceneFeatureError::Capacity(SceneCapacityError::Points)
        }
        FeatureReadError::Decode(FeatureDecodeError::Capacity(CapacityError::Rings)) => {
            SceneFeatureError::Capacity(SceneCapacityError::Rings)
        }
        FeatureReadError::Decode(FeatureDecodeError::Malformed) => SceneFeatureError::Malformed,
        FeatureReadError::Read(error) => SceneFeatureError::Read(read_error(error)),
    }
}

#[inline]
fn token(cid: u32, offset: usize) -> FeatureToken {
    debug_assert!(offset <= u16::MAX as usize);
    FeatureToken::from_source_words([cid as u16, (cid >> 16) as u16, offset as u16])
}

#[inline]
fn token_parts(token: FeatureToken) -> (u32, usize) {
    let [lo, hi, offset] = token.source_words();
    (((hi as u32) << 16) | lo as u32, offset as usize)
}

impl MapScene for Reader<'_> {
    #[inline]
    fn lod_count(&self) -> usize {
        self.lods().len()
    }

    #[inline]
    fn select_lod_for_mpp(&self, mpp: f32) -> usize {
        Reader::select_lod_for_mpp(self, mpp)
    }

    #[inline]
    fn style(&self, id: u8) -> Option<&obc_map_scene::Style> {
        Reader::style(self, id)
    }

    #[inline]
    fn diagnostics(&self) -> Result<Option<Diagnostics>, SceneReadError> {
        self.try_chunk_cache_stats()
            .map(|s| {
                Some(Diagnostics {
                    chunk_hits: s.chunk_hits,
                    chunk_misses: s.chunk_misses,
                    source_reads: s.sd_reads,
                    bytes_read: s.bytes_read,
                })
            })
            .map_err(|CacheError::Busy| SceneReadError::CacheBusy)
    }

    fn visit_candidates<const P: usize, const R: usize>(
        &self,
        lod: usize,
        view: &BBox,
        points: &mut Vec<(i32, i32), P>,
        ring_lens: &mut Vec<usize, R>,
        should_decode: impl Fn(u8) -> bool,
        mut visit: impl FnMut(Candidate<'_>),
    ) -> CandidateReport {
        let mut report = CandidateReport::default();
        let walk = self.for_each_chunk(lod, view, |cid, node| {
            report.chunks_visited += 1;
            match self.for_each_feature_filtered(lod, cid, &node, points, ring_lens, &should_decode, |feature| {
                visit(Candidate {
                    token: token(cid, feature.offset()),
                    feature: Feature::new(
                        feature.style_id,
                        feature.kind,
                        feature.points(),
                        feature.ring_lens(),
                        feature.bbox(),
                    ),
                });
            }) {
                Ok(status) => {
                    report.capacity_dropped = report.capacity_dropped.saturating_add(status.capacity_dropped);
                    report.malformed_features = report.malformed_features.saturating_add(status.malformed);
                }
                Err(error) => report.read_failures.record(read_error(error)),
            }
        });
        if let Err(error) = walk {
            report.read_failures.record(read_error(error));
        }
        report
    }

    fn decode_selected<const P: usize, const R: usize>(
        &self,
        lod: usize,
        view: &BBox,
        points: &mut Vec<(i32, i32), P>,
        ring_lens: &mut Vec<usize, R>,
        selected: &mut impl SelectedFeatures,
    ) -> DecodeReport {
        let mut report = DecodeReport::default();
        if selected.is_empty() {
            return report;
        }

        let walk = self.for_each_chunk(lod, view, |cid, node| {
            // Keep the existing `for visible chunk -> scan selected tokens` matching loop (making it
            // linear in chunks + winners is the dependent #849): first find whether this visited
            // chunk owns *any* pending winner, so a chunk with no winner is never loaded in pass B.
            let owns_winner = (0..selected.len())
                .any(|i| selected.is_pending(i) && selected.token(i).is_some_and(|t| token_parts(t).0 == cid));
            if !owns_winner {
                return;
            }

            // Load/borrow this winning chunk **once** and decode every selected offset that belongs
            // to it while the bytes are resident — one cache lookup / borrow / LRU update for the
            // whole batch instead of one per winner (#848). The batch borrow is held across the
            // callback exactly like `for_each_feature_filtered`.
            let mut refetched = false;
            let outcome = self.with_feature_chunk(lod, cid, &node, |chunk| {
                for i in 0..selected.len() {
                    if !selected.is_pending(i) {
                        continue;
                    }
                    let Some(token) = selected.token(i) else {
                        continue;
                    };
                    let (wanted_cid, offset) = token_parts(token);
                    if wanted_cid != cid {
                        continue;
                    }
                    match chunk.decode(offset, points, ring_lens) {
                        Ok(feature) => {
                            refetched |= selected.decoded(
                                i,
                                Feature::new(
                                    feature.style_id,
                                    feature.kind,
                                    feature.points(),
                                    feature.ring_lens(),
                                    feature.bbox(),
                                ),
                            );
                        }
                        // The bytes are already resident, so `decode` only ever returns a per-feature
                        // decode error (malformed/capacity) here — never a `Read`. Same typed outcome
                        // the per-winner `decode_feature_at` produced, scratch left empty on failure.
                        Err(error) => {
                            let _ = selected.failed(i, feature_error(error));
                        }
                    }
                }
            });
            match outcome {
                // `chunks_refetched` stays the count of chunks that successfully publish ≥1 feature.
                Ok(()) => {
                    if refetched {
                        report.chunks_refetched += 1;
                    }
                }
                // A single load failure records exactly one read failure for the whole chunk — no
                // per-winner retry of the same failed load — mirroring the pass-A chunk-read-failure
                // path in `visit_candidates`. Every winner of this chunk is failed the same way: none
                // publishes, so all stay unresolved (skipped by the pass-B compaction) rather than
                // one being marked and the rest lingering. Recording per token instead would re-count
                // the single I/O failure once per winner (the inflation this task removes).
                Err(error) => {
                    report.read_failures.record(read_error(error));
                }
            }
        });
        if let Err(error) = walk {
            report.read_failures.record(read_error(error));
        }
        report
    }
}
