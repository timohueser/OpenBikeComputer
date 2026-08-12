//! The second event pack holds the same guarantees the first one does (#1248).
//!
//! `event_pack.rs` is written against the derecho and states the rules a pack must satisfy in
//! full; this file does not restate them, it applies them to `us-airmass-2023-06-24` through the
//! same public helpers. A pack nobody re-bakes is a pile of bytes with a story attached, and this
//! one is the evidence behind a shipped constant, so it earns the same CI as the first.
//!
//! Two things differ from the derecho pack, and both are the point of capturing it:
//!
//! * it carries the **motion-history observation** (19:50 Z, ten minutes before its anchor), so its
//!   re-bake exercises WXR9's *live* branch — a nowcast layer at f+15 … f+60 — where the derecho
//!   pack exercises the fallback. Between them the two branches are both covered by a real pack.
//! * it is the **hard case**: scattered airmass convection instead of an organised derecho. What
//!   that is worth is `nowcast_skill_events.rs`; its 8.4 MB lives in the external package, not Git.

#![cfg(feature = "external-fixtures")]

use std::path::PathBuf;

use obc_wx_bake::pack::window::sub_lattice;
use obc_wx_bake::pack::{self, rebake, Event, Role};

const EVENT_ID: &str = "us-airmass-2023-06-24";

fn pack_root() -> PathBuf {
    obc_fixtures::root().join("weather-event-airmass")
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("obc-wx-airmass-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn event() -> Event {
    Event::read(&pack_root()).expect("the registry pack parses")
}

#[test]
fn the_pack_rebakes_byte_identically() {
    let event = event();
    let report = rebake::verify_rebake(&pack_root(), &event, &scratch("rebake")).expect("the pack re-bakes");
    eprintln!("{EVENT_ID} re-bake:\n{}", report.cycle.summary());
    // Every request the replay made must be answered by a member the pack carries — the property
    // that makes "it re-baked" mean "it re-baked from these bytes" rather than "from the network".
    let members: Vec<&str> = event.members.iter().map(|member| member.url.as_str()).collect();
    for request in &report.requests {
        let url = request.trim_start_matches("HEAD ").split('#').next().unwrap_or(request);
        assert!(members.contains(&url), "the replay asked for {url}, which no member records");
    }
}

/// The live branch, which the derecho pack cannot exercise: this capture found the observation ten
/// minutes before its anchor, so the cycle really does publish an advected radar layer.
#[test]
fn the_pack_carries_its_motion_history_and_publishes_a_nowcast_layer() {
    let event = event();
    let anchor = obc_wx_bake::timefmt::parse_rfc3339(&event.window_start).expect("window_start");
    let history = obc_wx_bake::source::mrms::object_url(anchor - obc_wx_bake::source::mrms::MOTION_LAG_SECONDS);
    // Each object appears twice — the adapter's HEAD probe and the body it then fetched. The body
    // is the one that has to be stored in the registry package.
    let member = event
        .members
        .iter()
        .find(|member| member.url == history && member.is_body_like())
        .unwrap_or_else(|| panic!("no body member for the motion-history observation {history}"));
    assert!(member.stored, "{history} is recorded but not stored in the registry package");
    assert_eq!(member.role, Role::Service, "the motion history is part of what the service had");

    let report = rebake::bake_into(&pack_root(), &event, &scratch("nowcast")).expect("a re-bake");
    let summary = report.cycle.summary();
    assert!(summary.contains("advected forward frames"), "the cycle published no nowcast layer:\n{summary}");
}

/// Re-derive `service/`, `truth/` and `event.json` from the registry-packaged `upstream/`, offline.
///
/// Ignored, like the derecho pack's twin: it **rewrites the fixture**, so it runs when the baker
/// deliberately changes and never in CI, where `the_pack_rebakes_byte_identically` is the check.
///
/// This pack needs it more often than the derecho does, and the reason is the interesting half:
/// the derecho publishes no nowcast layer (it has no motion baseline), so a change inside
/// `crate::flow` cannot move its bytes. This pack's f+15 … f+60 **are** the advected layer, so it
/// is the only fixture in the repository that notices. Raising `flow::MAX_FILL_NODES` from 6 to 9
/// is exactly such a change, and this is where it showed up.
#[test]
#[ignore = "rewrites the registry-packaged pack; run deliberately after a baker change"]
fn regenerate() {
    let root = pack_root();
    let mut event = event();
    rebake::regenerate(&root, &mut event).expect("the pack re-derives from its own upstream");
    eprintln!(
        "{EVENT_ID}: {} service objects and {} truth frames rewritten",
        event.service.len(),
        event.truth_frames.len()
    );
}

#[test]
fn every_stored_byte_matches_its_recorded_digest() {
    let event = event();
    let report = pack::verify_digests(&pack_root(), &event).expect("digests");
    eprintln!("{EVENT_ID}: {} digests verified", report.verified);
    assert!(report.verified >= event.service.len() + event.truth_frames.len());
    assert!(report.unmaterialized.is_empty());
    assert!(rebake::unmaterialized(&event).is_empty(), "the pack must have nothing left to fetch");
}

#[test]
fn the_truth_ladder_rebakes_byte_identically() {
    let event = event();
    let compared = rebake::verify_truth_rebake(&pack_root(), &event).expect("the truth ladder re-bakes");
    assert_eq!(compared, event.truth_frames.len());
    assert_eq!(compared, 8, "a two-hour ladder at 15-minute steps");
}

/// The one-basemap convention, held the cheap way: this pack requested the **same** window the
/// derecho did, so the two events are scored over identical ground and the Iowa map serves both.
/// A future pack that wants different ground answers `event_pack.rs`'s budget arithmetic instead.
#[test]
fn the_pack_stays_on_the_basemap_and_on_the_derechos_window() {
    let event = event();
    assert_eq!(event.basemap_region, pack::US_BASEMAP_REGION);
    let derecho = Event::read(&obc_fixtures::root().join("weather-event-derecho")).expect("the first pack");
    assert_eq!(
        event.bake.bbox_udeg, derecho.bake.bbox_udeg,
        "the second event is a controlled comparison: same window, same lattice, different weather"
    );
    assert_eq!(event.coverage_udeg, derecho.coverage_udeg);
    sub_lattice(&event.bake.bbox_udeg).expect("the pack's lattice");
}
