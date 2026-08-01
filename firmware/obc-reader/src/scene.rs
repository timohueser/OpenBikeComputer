//! Thin `Reader` adapter for the allocation-free semantic map-scene contract.

use heapless::Vec;
use obc_map_scene::{
    BBox, Candidate, CandidateReport, CapacityError as SceneCapacityError, DecodeReport, Diagnostics, Feature,
    FeatureError as SceneFeatureError, FeatureToken, MapScene, ReadError as SceneReadError, SelectedFeatures,
};

use crate::{CacheError, CapacityError, FeatureDecodeError, FeatureReadError, MapReadError, Reader};

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
    fn marker_color(&self) -> u16 {
        self.marker_color
    }

    #[inline]
    fn backdrop_style(&self) -> Option<&obc_map_scene::Style> {
        Reader::backdrop_style(self)
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
            let mut refetched = false;
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
                match self.decode_feature_at(lod, cid, offset, &node, points, ring_lens) {
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
                    Err(error) => {
                        let _ = selected.failed(i, feature_error(error));
                    }
                }
            }
            if refetched {
                report.chunks_refetched += 1;
            }
        });
        if let Err(error) = walk {
            report.read_failures.record(read_error(error));
        }
        report
    }
}
