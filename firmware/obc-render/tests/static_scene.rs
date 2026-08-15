//! The base renderer is testable without OBCM bytes or a concrete `obc_reader::Reader`.

mod common;

use core::cell::Cell;

use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::prelude::RgbColor;
use heapless::Vec;
use obc_map_scene::{
    BBox, Candidate, CandidateReport, DecodeReport, Feature, FeatureError, FeatureToken, Kind, MapScene,
    SelectedFeatures, Style, StyleFlags,
};
use obc_render::{RenderConfig, RenderScratch, Viewport, MAX_SPANS};

use common::Buf;

const TOKEN: FeatureToken = FeatureToken::from_source_words([1, 0, 0]);
const POINTS: [(i32, i32); 4] = [(-8, -8), (8, -8), (8, 8), (-8, 8)];
const RINGS: [usize; 1] = [4];
const BOUNDS: BBox = BBox { min_lon: -8, min_lat: -8, max_lon: 8, max_lat: 8 };
const STYLE: Style =
    Style { id: 7, z_index: 3, color: 0xF800, weight: 1, priority: 1, flags: StyleFlags::NONE, color2: None };

struct StaticScene;

impl MapScene for StaticScene {
    fn lod_count(&self) -> usize {
        1
    }

    fn select_lod_for_mpp(&self, _mpp: f32) -> usize {
        0
    }

    fn style(&self, id: u8) -> Option<&Style> {
        (id == STYLE.id).then_some(&STYLE)
    }

    fn visit_candidates<const P: usize, const R: usize>(
        &self,
        _lod: usize,
        view: &BBox,
        points: &mut Vec<(i32, i32), P>,
        ring_lens: &mut Vec<usize, R>,
        should_decode: impl Fn(u8) -> bool,
        mut visit: impl FnMut(Candidate<'_>),
    ) -> CandidateReport {
        if should_decode(STYLE.id) && BOUNDS.intersects(view) {
            points.clear();
            ring_lens.clear();
            points.extend_from_slice(&POINTS).unwrap();
            ring_lens.extend_from_slice(&RINGS).unwrap();
            visit(Candidate {
                token: TOKEN,
                feature: Feature::new(STYLE.id, Kind::Polygon, points, ring_lens, BOUNDS),
            });
        }
        CandidateReport { chunks_visited: 1, ..Default::default() }
    }

    fn decode_selected<const P: usize, const R: usize>(
        &self,
        _lod: usize,
        _view: &BBox,
        points: &mut Vec<(i32, i32), P>,
        ring_lens: &mut Vec<usize, R>,
        selected: &mut impl SelectedFeatures,
    ) -> DecodeReport {
        for i in 0..selected.len() {
            if selected.is_pending(i) && selected.token(i) == Some(TOKEN) {
                points.clear();
                ring_lens.clear();
                points.extend_from_slice(&POINTS).unwrap();
                ring_lens.extend_from_slice(&RINGS).unwrap();
                let _ = selected.decoded(i, Feature::new(STYLE.id, Kind::Polygon, points, ring_lens, BOUNDS));
            }
        }
        DecodeReport { chunks_refetched: 1, ..Default::default() }
    }
}

const TOKEN2: FeatureToken = FeatureToken::from_source_words([2, 0, 0]);
const LINE_POINTS: [(i32, i32); 2] = [(-12, 0), (12, 0)];
const LINE_RING: [usize; 1] = [2];
const BIG_POINTS: [(i32, i32); 5] = [(-8, -8), (8, -8), (8, 8), (-8, 8), (0, 0)];
const BIG_RING: [usize; 1] = [5];
const SHORT_RING: [usize; 1] = [3];
const MULTI_RINGS: [usize; 2] = [2, 2];
const ZERO_MULTI_RINGS: [usize; 3] = [2, 0, 2];
const OVERFLOW_MULTI_RINGS: [usize; 2] = [usize::MAX, 2];
const GOOD2: Style =
    Style { id: 8, z_index: 4, color: 0x001F, weight: 2, priority: 2, flags: StyleFlags::NONE, color2: None };
const PRIORITY_ZERO: Style = Style { id: 9, priority: 0, ..STYLE };
const PRIORITY_FIVE: Style = Style { id: 10, priority: 5, ..STYLE };

#[derive(Clone, Copy)]
enum Hostility {
    PriorityZero,
    PriorityFive,
    PassAMissingStyle,
    PassAMalformedRings,
    PassAZeroMultiRing,
    PassAOverflowMultiRing,
    BoundsAndDuplicateSuccess,
    DuplicateFailure,
    PassBSizeMismatch,
    PassBMalformedRings,
    PassBRingCountMismatch,
    PassBMissingStyle,
    PassBIdentityMismatch,
}

struct HostileScene(Hostility);

impl HostileScene {
    fn visit_one<'a>(
        token: FeatureToken,
        style_id: u8,
        kind: Kind,
        points: &'a [(i32, i32)],
        rings: &'a [usize],
        visit: &mut impl FnMut(Candidate<'a>),
    ) {
        visit(Candidate { token, feature: Feature::new(style_id, kind, points, rings, BOUNDS) });
    }
}

impl MapScene for HostileScene {
    fn lod_count(&self) -> usize {
        1
    }

    fn select_lod_for_mpp(&self, _mpp: f32) -> usize {
        0
    }

    fn style(&self, id: u8) -> Option<&Style> {
        match id {
            7 => Some(&STYLE),
            8 => Some(&GOOD2),
            9 => Some(&PRIORITY_ZERO),
            10 => Some(&PRIORITY_FIVE),
            _ => None,
        }
    }

    fn visit_candidates<const P: usize, const R: usize>(
        &self,
        _lod: usize,
        _view: &BBox,
        points: &mut Vec<(i32, i32), P>,
        ring_lens: &mut Vec<usize, R>,
        _should_decode: impl Fn(u8) -> bool,
        mut visit: impl FnMut(Candidate<'_>),
    ) -> CandidateReport {
        points.clear();
        ring_lens.clear();
        match self.0 {
            Hostility::PriorityZero => {
                points.extend_from_slice(&POINTS).unwrap();
                ring_lens.extend_from_slice(&RINGS).unwrap();
                Self::visit_one(TOKEN, PRIORITY_ZERO.id, Kind::Polygon, points, ring_lens, &mut visit);
            }
            Hostility::PriorityFive => {
                points.extend_from_slice(&POINTS).unwrap();
                ring_lens.extend_from_slice(&RINGS).unwrap();
                Self::visit_one(TOKEN, PRIORITY_FIVE.id, Kind::Polygon, points, ring_lens, &mut visit);
            }
            Hostility::PassAMalformedRings => {
                points.extend_from_slice(&POINTS).unwrap();
                ring_lens.extend_from_slice(&SHORT_RING).unwrap();
                Self::visit_one(TOKEN, STYLE.id, Kind::Polygon, points, ring_lens, &mut visit);
            }
            Hostility::PassAMissingStyle => {
                points.extend_from_slice(&POINTS).unwrap();
                ring_lens.extend_from_slice(&RINGS).unwrap();
                Self::visit_one(TOKEN, 99, Kind::Polygon, points, ring_lens, &mut visit);
            }
            Hostility::PassAZeroMultiRing => {
                points.extend_from_slice(&POINTS).unwrap();
                ring_lens.extend_from_slice(&ZERO_MULTI_RINGS).unwrap();
                Self::visit_one(TOKEN, STYLE.id, Kind::Polygon, points, ring_lens, &mut visit);
            }
            Hostility::PassAOverflowMultiRing => {
                points.extend_from_slice(&POINTS).unwrap();
                ring_lens.extend_from_slice(&OVERFLOW_MULTI_RINGS).unwrap();
                Self::visit_one(TOKEN, STYLE.id, Kind::Polygon, points, ring_lens, &mut visit);
            }
            Hostility::BoundsAndDuplicateSuccess | Hostility::DuplicateFailure => {
                points.extend_from_slice(&POINTS).unwrap();
                ring_lens.extend_from_slice(&RINGS).unwrap();
                Self::visit_one(TOKEN, STYLE.id, Kind::Polygon, points, ring_lens, &mut visit);
            }
            Hostility::PassBSizeMismatch
            | Hostility::PassBMalformedRings
            | Hostility::PassBRingCountMismatch
            | Hostility::PassBMissingStyle
            | Hostility::PassBIdentityMismatch => {
                points.extend_from_slice(&POINTS).unwrap();
                let pass_a_rings: &[usize] =
                    if matches!(self.0, Hostility::PassBRingCountMismatch) { &MULTI_RINGS } else { &RINGS };
                ring_lens.extend_from_slice(pass_a_rings).unwrap();
                Self::visit_one(TOKEN, STYLE.id, Kind::Polygon, points, ring_lens, &mut visit);
                points.clear();
                ring_lens.clear();
                points.extend_from_slice(&LINE_POINTS).unwrap();
                ring_lens.extend_from_slice(&LINE_RING).unwrap();
                Self::visit_one(TOKEN2, GOOD2.id, Kind::Line, points, ring_lens, &mut visit);
                // Force the optimistic collector to overflow its span reservoir. The retry must
                // enter stub-select/pass B, where the hostile second decode below remains covered.
                for _ in 0..MAX_SPANS - 1 {
                    Self::visit_one(TOKEN2, GOOD2.id, Kind::Line, points, ring_lens, &mut visit);
                }
            }
        }
        CandidateReport { chunks_visited: 1, ..Default::default() }
    }

    fn decode_selected<const P: usize, const R: usize>(
        &self,
        _lod: usize,
        _view: &BBox,
        points: &mut Vec<(i32, i32), P>,
        ring_lens: &mut Vec<usize, R>,
        selected: &mut impl SelectedFeatures,
    ) -> DecodeReport {
        match self.0 {
            Hostility::PriorityZero
            | Hostility::PriorityFive
            | Hostility::PassAMissingStyle
            | Hostility::PassAMalformedRings
            | Hostility::PassAZeroMultiRing
            | Hostility::PassAOverflowMultiRing => {
                assert_eq!(selected.len(), 0);
            }
            Hostility::BoundsAndDuplicateSuccess => {
                assert_eq!(selected.len(), 1);
                assert!(!selected.is_pending(1));
                assert_eq!(selected.token(1), None);
                assert!(!selected.decoded(1, Feature::new(STYLE.id, Kind::Polygon, &POINTS, &RINGS, BOUNDS)));
                assert!(!selected.failed(1, FeatureError::Malformed));
                assert_eq!(selected.token(0), Some(TOKEN));
                assert!(selected.decoded(0, Feature::new(STYLE.id, Kind::Polygon, &POINTS, &RINGS, BOUNDS)));
                assert!(!selected.is_pending(0));
                assert_eq!(selected.token(0), None);
                assert!(!selected.decoded(0, Feature::new(STYLE.id, Kind::Polygon, &POINTS, &RINGS, BOUNDS)));
                assert!(!selected.failed(0, FeatureError::Malformed));
            }
            Hostility::DuplicateFailure => {
                assert!(selected.failed(0, FeatureError::Malformed));
                assert!(!selected.failed(0, FeatureError::Malformed));
                assert!(!selected.decoded(0, Feature::new(STYLE.id, Kind::Polygon, &POINTS, &RINGS, BOUNDS)));
                assert_eq!(selected.token(0), None);
            }
            mode @ (Hostility::PassBSizeMismatch
            | Hostility::PassBMalformedRings
            | Hostility::PassBRingCountMismatch
            | Hostility::PassBMissingStyle
            | Hostility::PassBIdentityMismatch) => {
                assert_eq!(selected.len(), MAX_SPANS);
                points.clear();
                ring_lens.clear();
                match mode {
                    Hostility::PassBSizeMismatch => {
                        points.extend_from_slice(&BIG_POINTS).unwrap();
                        ring_lens.extend_from_slice(&BIG_RING).unwrap();
                        assert!(!selected.decoded(0, Feature::new(STYLE.id, Kind::Polygon, points, ring_lens, BOUNDS)));
                    }
                    Hostility::PassBMalformedRings => {
                        points.extend_from_slice(&POINTS).unwrap();
                        ring_lens.extend_from_slice(&SHORT_RING).unwrap();
                        assert!(!selected.decoded(0, Feature::new(STYLE.id, Kind::Polygon, points, ring_lens, BOUNDS)));
                    }
                    Hostility::PassBRingCountMismatch => {
                        points.extend_from_slice(&POINTS).unwrap();
                        ring_lens.extend_from_slice(&RINGS).unwrap();
                        assert!(!selected.decoded(0, Feature::new(STYLE.id, Kind::Polygon, points, ring_lens, BOUNDS)));
                    }
                    Hostility::PassBMissingStyle => {
                        points.extend_from_slice(&POINTS).unwrap();
                        ring_lens.extend_from_slice(&RINGS).unwrap();
                        assert!(!selected.decoded(0, Feature::new(99, Kind::Polygon, points, ring_lens, BOUNDS)));
                    }
                    Hostility::PassBIdentityMismatch => {
                        points.extend_from_slice(&POINTS).unwrap();
                        ring_lens.extend_from_slice(&RINGS).unwrap();
                        assert!(!selected.decoded(0, Feature::new(STYLE.id, Kind::Line, points, ring_lens, BOUNDS)));
                    }
                    _ => unreachable!(),
                }
                points.clear();
                ring_lens.clear();
                points.extend_from_slice(&LINE_POINTS).unwrap();
                ring_lens.extend_from_slice(&LINE_RING).unwrap();
                for index in 1..selected.len() {
                    assert!(selected.decoded(index, Feature::new(GOOD2.id, Kind::Line, points, ring_lens, BOUNDS)));
                }
            }
        }
        DecodeReport { chunks_refetched: 1, ..Default::default() }
    }
}

struct ZeroLodScene;

impl MapScene for ZeroLodScene {
    fn lod_count(&self) -> usize {
        0
    }

    fn select_lod_for_mpp(&self, _mpp: f32) -> usize {
        panic!("zero-LOD scene must not be asked to select an LOD")
    }

    fn style(&self, _id: u8) -> Option<&Style> {
        panic!("zero-LOD scene must not be queried for styles")
    }

    fn visit_candidates<const P: usize, const R: usize>(
        &self,
        _lod: usize,
        _view: &BBox,
        _points: &mut Vec<(i32, i32), P>,
        _ring_lens: &mut Vec<usize, R>,
        _should_decode: impl Fn(u8) -> bool,
        _visit: impl FnMut(Candidate<'_>),
    ) -> CandidateReport {
        panic!("zero-LOD scene must not be visited")
    }

    fn decode_selected<const P: usize, const R: usize>(
        &self,
        _lod: usize,
        _view: &BBox,
        _points: &mut Vec<(i32, i32), P>,
        _ring_lens: &mut Vec<usize, R>,
        _selected: &mut impl SelectedFeatures,
    ) -> DecodeReport {
        panic!("zero-LOD scene must not decode")
    }
}

#[derive(Clone, Copy)]
enum MetadataHostility {
    FluctuatingLodCount,
    OutOfRangeSelectedLod,
}

struct MetadataScene {
    mode: MetadataHostility,
    lod_count_calls: Cell<usize>,
}

impl MetadataScene {
    fn new(mode: MetadataHostility) -> Self {
        Self { mode, lod_count_calls: Cell::new(0) }
    }
}

impl MapScene for MetadataScene {
    fn lod_count(&self) -> usize {
        let calls = self.lod_count_calls.get();
        self.lod_count_calls.set(calls.saturating_add(1));
        match self.mode {
            MetadataHostility::FluctuatingLodCount if calls > 0 => 0,
            _ => 1,
        }
    }

    fn select_lod_for_mpp(&self, _mpp: f32) -> usize {
        match self.mode {
            MetadataHostility::OutOfRangeSelectedLod => usize::MAX,
            MetadataHostility::FluctuatingLodCount => 0,
        }
    }

    fn style(&self, id: u8) -> Option<&Style> {
        StaticScene.style(id)
    }

    fn visit_candidates<const P: usize, const R: usize>(
        &self,
        lod: usize,
        view: &BBox,
        points: &mut Vec<(i32, i32), P>,
        ring_lens: &mut Vec<usize, R>,
        should_decode: impl Fn(u8) -> bool,
        visit: impl FnMut(Candidate<'_>),
    ) -> CandidateReport {
        StaticScene.visit_candidates(lod, view, points, ring_lens, should_decode, visit)
    }

    fn decode_selected<const P: usize, const R: usize>(
        &self,
        lod: usize,
        view: &BBox,
        points: &mut Vec<(i32, i32), P>,
        ring_lens: &mut Vec<usize, R>,
        selected: &mut impl SelectedFeatures,
    ) -> DecodeReport {
        StaticScene.decode_selected(lod, view, points, ring_lens, selected)
    }
}

fn hostile_render(mode: Hostility) -> (obc_render::RenderStats, Buf) {
    let mut renderer = RenderScratch::new();
    let mut target = Buf::new(64, 64);
    let viewport = Viewport::new(64.0, 64.0, 0, 0, 1.0);
    let stats =
        renderer.render(&mut target, &HostileScene(mode), &viewport, Rgb888::BLACK, RenderConfig::default(), |color| {
            match color {
                0xF800 => Rgb888::RED,
                0x001F => Rgb888::BLUE,
                _ => Rgb888::WHITE,
            }
        });
    (stats, target)
}

#[test]
fn selected_publication_is_bounds_checked_and_duplicate_success_is_a_noop() {
    let (stats, target) = hostile_render(Hostility::BoundsAndDuplicateSuccess);
    assert_eq!(stats.features_drawn, 1);
    assert_eq!(stats.points_drawn, 4);
    assert_eq!(stats.malformed_features, 0);
    assert!(target.count(Rgb888::RED) > 200);
}

#[test]
fn direct_collect_does_not_invoke_a_duplicate_second_pass_failure() {
    let (stats, target) = hostile_render(Hostility::DuplicateFailure);
    assert_eq!(stats.features_drawn, 1);
    assert_eq!(stats.malformed_features, 0);
    assert!(target.count(Rgb888::RED) > 200);
}

#[test]
fn invalid_priorities_are_rejected_without_indexing_the_level_table() {
    for mode in [Hostility::PriorityZero, Hostility::PriorityFive] {
        let (stats, target) = hostile_render(mode);
        assert_eq!(stats.features_tried, 1);
        assert_eq!(stats.features_drawn, 0);
        assert_eq!(stats.malformed_features, 1);
        assert_eq!(target.count(Rgb888::RED), 0);
    }
}

#[test]
fn invalid_pass_a_features_never_reach_the_painter() {
    for mode in [
        Hostility::PassAMissingStyle,
        Hostility::PassAMalformedRings,
        Hostility::PassAZeroMultiRing,
        Hostility::PassAOverflowMultiRing,
    ] {
        let (stats, target) = hostile_render(mode);
        assert_eq!(stats.features_tried, 1);
        assert_eq!(stats.features_drawn, 0);
        assert_eq!(stats.malformed_features, 1);
        assert_eq!(target.count(Rgb888::RED), 0);
    }
}

#[test]
fn zero_lod_scene_renders_only_the_background() {
    let mut renderer = RenderScratch::new();
    let mut target = Buf::new(64, 64);
    let viewport = Viewport::new(64.0, 64.0, 0, 0, 1.0);
    let stats =
        renderer.render(&mut target, &ZeroLodScene, &viewport, Rgb888::BLUE, RenderConfig::default(), |_| Rgb888::RED);
    assert_eq!(stats.features_tried, 0);
    assert_eq!(stats.features_drawn, 0);
    assert_eq!(target.count(Rgb888::BLUE), 64 * 64);
}

#[test]
fn lod_count_is_snapshotted_before_drawing() {
    let scene = MetadataScene::new(MetadataHostility::FluctuatingLodCount);
    let mut renderer = RenderScratch::new();
    let mut target = Buf::new(64, 64);
    let viewport = Viewport::new(64.0, 64.0, 0, 0, 1.0);
    let stats =
        renderer.render(&mut target, &scene, &viewport, Rgb888::BLACK, RenderConfig::default(), |_| Rgb888::RED);

    assert_eq!(scene.lod_count_calls.get(), 1);
    assert_eq!(stats.features_drawn, 1);
    assert!(target.count(Rgb888::RED) > 200);
}

#[test]
fn out_of_range_selected_lod_is_clamped_and_reported() {
    let scene = MetadataScene::new(MetadataHostility::OutOfRangeSelectedLod);
    let mut renderer = RenderScratch::new();
    let mut target = Buf::new(64, 64);
    let viewport = Viewport::new(64.0, 64.0, 0, 0, 1.0);
    let stats =
        renderer.render(&mut target, &scene, &viewport, Rgb888::BLACK, RenderConfig::default(), |_| Rgb888::RED);

    assert_eq!(scene.lod_count_calls.get(), 1);
    assert_eq!(stats.lod, 0);
    assert_eq!(stats.map_structure_failures, 1);
    assert_eq!(stats.features_drawn, 1);
    assert!(target.count(Rgb888::RED) > 200);
}

#[test]
fn saturated_fallback_omits_a_hostile_pass_b_feature_without_stealing_later_capacity() {
    for mode in [
        Hostility::PassBSizeMismatch,
        Hostility::PassBMalformedRings,
        Hostility::PassBRingCountMismatch,
        Hostility::PassBMissingStyle,
        Hostility::PassBIdentityMismatch,
    ] {
        let (stats, target) = hostile_render(mode);
        assert_eq!(stats.features_tried, MAX_SPANS + 1);
        assert_eq!(stats.features_drawn, MAX_SPANS - 1);
        assert_eq!(stats.points_drawn, (MAX_SPANS - 1) * 2);
        assert_eq!(stats.features_dropped, 1);
        assert_eq!(stats.malformed_features, 1);
        assert_eq!(stats.feature_decode_capacity_drops, 0);
        assert_eq!(target.count(Rgb888::RED), 0);
        assert!(target.count(Rgb888::BLUE) > 0);
    }
}

#[test]
fn renders_static_scene_without_reader_or_map_bytes() {
    let mut renderer = RenderScratch::new();
    let mut target = Buf::new(64, 64);
    let viewport = Viewport::new(64.0, 64.0, 0, 0, 1.0);

    let stats =
        renderer.render(&mut target, &StaticScene, &viewport, Rgb888::BLACK, RenderConfig::default(), |_| Rgb888::RED);

    assert_eq!(stats.lod, 0);
    assert_eq!(stats.features_tried, 1);
    assert_eq!(stats.features_drawn, 1);
    assert_eq!(stats.points_drawn, 4);
    assert!(target.count(Rgb888::RED) > 200);
}
