//! The break matrix: every opcode flow, cut at every protocol step boundary.
//!
//! It discharges one obligation of `FLAT_Store_Protocol.md`: any break before the commit leaves the
//! card as if nothing happened. Each flow is run once to the end to establish the card after it,
//! then run again for every prefix of its records with the link torn down at that point. Three
//! things must then hold:
//!
//! 1. The catalog's byte image is the one before the flow or the one after it, and nothing between.
//! 2. No row leaked: free extents match that state, both reservation rows are free, and the hold
//!    table let go of what a download was reading. A dropped `Allocation` or `Handle` releases
//!    nothing, and the only symptom until the next mount is an extent that never comes back.
//! 3. A following `STATUS` answers the catalog's own truth in all four of its fields.
//!
//! A break is between two link records, so nothing here cuts inside a commit; that is the crash
//! matrix in `obc-storage`. The byte-image comparison also sees the batch a commit applied, not the
//! batch it was built from, so `expected` below is the independent oracle.

mod flat_harness;

use flat_harness::{boot, boot_on, catalog_image, client, formatted_card, payload, Answer, Plain};
use obc_link::flat::store::Policy;
use obc_link::flat::{Ceilings, Link, ObjectId, ObjectKind, Revision};
use obc_storage::flat::sim::SparseDisk;

const ROUTE: u16 = 1;

/// One thing a client does, or one pump of a transfer the engine is driving.
#[derive(Clone, Debug)]
enum Step {
    /// A control record. The engine is pumped once, so a `GET` emits its first payload record here.
    Control(Vec<u8>),
    /// A stream record of an upload.
    Stream(Vec<u8>),
    /// One more record out of a live transfer.
    Pump,
    /// An `ARM` on a device that *can* arm — the one flow whose commit is not a client's.
    Arm(Vec<u8>),
}

/// A device with an update path, for the arming flow: both policy hooks satisfied.
#[derive(Default)]
struct Armer;

impl Policy for Armer {
    fn validate_package(&mut self, _package: ObjectId, _revision: Revision) -> Result<u64, u16> {
        Ok(900_000)
    }

    fn hand_off(&mut self, _package: (ObjectId, Revision), _reserve: (ObjectId, Revision)) -> Result<(), u16> {
        Ok(())
    }
}

/// How a scenario sets its card up and what it then does.
type Build = fn(&mut Plain<'_>) -> Plan;

/// A flow, and the object a client would reconcile against after a break.
struct Plan {
    steps: Vec<Step>,
    /// What the client asks `STATUS` about: the `ObjectId` and the `Revision` it expected.
    subject: (u64, u64),
    /// Whether the catalog names the subject in **every** state this flow can be broken into. False
    /// only for a flow that either creates it (absent before the commit) or removes it.
    subject_survives: bool,
    /// The entries the catalog must hold once the flow has run, in catalog order: `(ObjectId,
    /// Revision, RETAINED)`.
    ///
    /// The matrix's independent oracle: every other assertion compares the card against the same
    /// code's own "after" run, so a commit built from the wrong batch would agree with itself.
    settled: Vec<(u64, u64, bool)>,
    /// Objects besides the subject whose extents must come back when they are removed — the hold
    /// probe, for a flow that commits an entry of its own. Probed only in the states that hold them.
    probe: Vec<u64>,
}

impl Plan {
    /// The common shape: no second object to probe.
    fn new(steps: Vec<Step>, subject: (u64, u64), subject_survives: bool, settled: Vec<(u64, u64, bool)>) -> Self {
        Plan { steps, subject, subject_survives, settled, probe: Vec::new() }
    }
}

/// Feeds `plan`'s records from `from` up to `steps`.
fn feed(device: &mut Plain<'_>, plan: &Plan, steps: usize, from: usize) {
    for step in plan.steps.iter().take(steps).skip(from) {
        match step {
            Step::Control(record) => {
                device.control_upto(record, 1);
            }
            Step::Stream(record) => {
                device.stream(record);
            }
            Step::Pump => {
                device.pump_once();
            }
            Step::Arm(record) => {
                device.control_with_upto(record, &mut Armer, 1);
            }
        }
    }
}

/// The catalog as the matrix compares it: `(ObjectId, Revision, RETAINED)` per entry, in order.
fn settled(device: &Plain<'_>) -> Vec<(u64, u64, bool)> {
    device
        .entries()
        .iter()
        .map(|meta| (meta.id.0, meta.revision.0, meta.flags.has(obc_storage::flat::EntryFlags::RETAINED)))
        .collect()
}

/// The whole `STATUS` answer for `subject`, read straight off the catalog: state, then the head's
/// revision, payload length and CRC, all zero when the object is absent.
fn truth(device: &Plain<'_>, subject: (u64, u64)) -> (u8, u64, u64, u32) {
    match device.entry(subject.0) {
        None => (0, 0, 0, 0),
        Some(entry) => {
            (if entry.revision.0 == subject.1 { 1 } else { 2 }, entry.revision.0, entry.payload_len, entry.payload_crc)
        }
    }
}

/// Runs one scenario at every break point and holds each result to the three rules above.
fn matrix(name: &str, seed: u64, build: Build) -> usize {
    // One reference run, recording the card after every record: that sequence is what a break at
    // each point must land on. A flow may commit more than once, so the rule is "the state the
    // records that landed produce", not "the state before or the state after".
    let disk = formatted_card(seed);
    let mut device = boot(&disk);
    let plan = build(&mut device);
    let mut images = vec![catalog_image(&disk)];
    let mut catalogs = vec![settled(&device)];
    for step in 0..plan.steps.len() {
        feed(&mut device, &plan, step + 1, step);
        images.push(catalog_image(&disk));
        catalogs.push(settled(&device));
    }
    assert!(device.is_quiet(), "{name}: the engine is still busy after the whole flow");
    assert_eq!(settled(&device), plan.settled, "{name}: the flow did not leave the catalog it says it does");
    drop(device);

    let breaks = plan.steps.len() + 1;
    for cut in 0..breaks {
        let disk = formatted_card(seed);
        let mut device = boot(&disk);
        let plan = build(&mut device);
        assert_eq!(catalog_image(&disk), images[0], "{name}: the scenario is not deterministic");
        feed(&mut device, &plan, cut, 0);
        device.link_lost();

        let where_ = format!("{name}: broken after {cut} of {} records", plan.steps.len());
        assert!(device.is_quiet(), "{where_}: a transfer survived the link");
        // 1. The catalog is exactly what the records that landed produced: a commit is atomically
        //    visible or it is not there.
        assert_eq!(catalog_image(&disk), images[cut], "{where_}: the catalog is not the one those records produce");
        assert_eq!(settled(&device), catalogs[cut], "{where_}: the entries are not the ones those records produce");

        // 2. Nothing leaked. A fresh mount rebuilds the free map from the catalog and sees no
        //    reservation and no hold, so it is the answer the broken store must already agree with.
        let expected_free = boot(&disk).free_extents();
        assert_eq!(device.free_extents(), expected_free, "{where_}: a reservation or a hold outlived the link");
        // Both reservation rows are free: a leaked row is invisible in the extent count once its
        // extents came back.
        let first = device.hog(1_024);
        let second = device.hog(1_024);
        device.release(first);
        device.release(second);
        assert_eq!(device.free_extents(), expected_free, "{where_}: the reservation probe changed the card");

        // 3. The reconcile path answers the catalog's own truth in all four fields: a state that
        //    agreed while the length or CRC did not would send a client to re-download, or to trust
        //    bytes it should not.
        let answer = Answer::of(device.control(&client::status(0x5EED, plan.subject.0, plan.subject.1)).answer());
        assert!(!answer.is_error(), "{where_}: STATUS refused");
        let answered = (answer.body[0], answer.u64_at(4), answer.u64_at(12), answer.u32_at(20));
        assert_eq!(answered, truth(&device, plan.subject), "{where_}: STATUS does not reconcile");

        // The hold table let go: a handle the engine failed to close keeps the entry's extents out
        // of the allocator when it is removed.
        if plan.subject_survives {
            assert!(
                device.entry(plan.subject.0).is_some(),
                "{where_}: the subject is not in the catalog and this flow never removes it"
            );
        }
        for id in core::iter::once(plan.subject.0).chain(plan.probe.iter().copied()) {
            if device.entry(id).is_some() {
                let freed = device.remove_and_measure(id);
                assert!(freed > 0, "{where_}: a hold kept object {id}'s extents after it was removed");
            }
        }
    }
    breaks
}

fn stream_steps(request: u32, bytes: &[u8]) -> Vec<Step> {
    client::stream_all(request, bytes, 1_008).into_iter().map(Step::Stream).collect()
}

fn create(_device: &mut Plain<'_>) -> Plan {
    let bytes = payload(2_600);
    let mut steps = vec![Step::Control(client::put(1, 0, 0, &bytes, ROUTE, "created"))];
    steps.extend(stream_steps(1, &bytes));
    Plan::new(steps, (1, 1), false, vec![(1, 1, false)])
}

/// A create long enough that most of its break points are inside the stream, which is where a
/// staging buffer either releases what it holds or does not.
fn long_create(_device: &mut Plain<'_>) -> Plan {
    let bytes = payload(8_000);
    let mut steps = vec![Step::Control(client::put(1, 0, 0, &bytes, ROUTE, "long"))];
    steps.extend(stream_steps(1, &bytes));
    Plan::new(steps, (1, 1), false, vec![(1, 1, false)])
}

fn replace(device: &mut Plain<'_>) -> Plan {
    let bytes = payload(1_200);
    let (id, revision) = device.seed(ObjectKind::Route, &payload(600), "first");
    let mut steps = vec![Step::Control(client::put(1, id, revision, &bytes, ROUTE, "second"))];
    steps.extend(stream_steps(1, &bytes));
    // An ordinary replace leaves the object with a head and nothing else.
    Plan::new(steps, (id, revision + 1), true, vec![(id, revision + 1, false)])
}

fn remove(device: &mut Plain<'_>) -> Plan {
    let (id, revision) = device.seed(ObjectKind::Route, &payload(600), "doomed");
    Plan::new(vec![Step::Control(client::remove(1, id, revision))], (id, revision), false, vec![])
}

fn download(device: &mut Plain<'_>) -> Plan {
    let (id, revision) = device.seed(ObjectKind::Route, &payload(2_600), "served");
    // The control record emits the first payload record; two more and the answer follow.
    let steps = vec![Step::Control(client::get(1, id, 0)), Step::Pump, Step::Pump, Step::Pump];
    Plan::new(steps, (id, revision), true, vec![(id, revision, false)])
}

fn cancelled_upload(device: &mut Plain<'_>) -> Plan {
    let bytes = payload(2_600);
    let (id, revision) = device.seed(ObjectKind::Route, &payload(600), "untouched");
    let records = client::stream_all(1, &bytes, 1_008);
    let steps = vec![
        Step::Control(client::put(1, 0, 0, &bytes, ROUTE, "abandoned")),
        Step::Stream(records[0].clone()),
        Step::Control(client::cancel(2, 1)),
        Step::Pump,
    ];
    Plan::new(steps, (id, revision), true, vec![(id, revision, false)])
}

fn cancelled_download(device: &mut Plain<'_>) -> Plan {
    let (id, revision) = device.seed(ObjectKind::Route, &payload(2_600), "half served");
    let steps = vec![Step::Control(client::get(1, id, 0)), Step::Pump, Step::Control(client::cancel(2, 1)), Step::Pump];
    Plan::new(steps, (id, revision), true, vec![(id, revision, false)])
}

fn paged_listing(device: &mut Plain<'_>) -> Plan {
    for index in 0..5 {
        device.seed(ObjectKind::Route, &payload(64), &format!("object {index}"));
    }
    let steps = vec![Step::Control(client::list(1, None)), Step::Control(client::list_from(2, None, (2, 1), 6))];
    Plan::new(steps, (1, 1), true, (1..=5).map(|id| (id, 1, false)).collect())
}

/// The one flow whose commit is the device's rather than a client's: `ARM` commits a rollback
/// reserve. A break can land before the `ARM` or after its answer, but not between the commit and
/// the boot handoff, which are one engine call; `flat_engine.rs` drives that refusal directly.
fn arm_succeeds(device: &mut Plain<'_>) -> Plan {
    let (id, revision) = device.seed(ObjectKind::UpdatePackage, &payload(4_096), "v2");
    // One rollback-reserve entry with `RESERVED`, beside the package it will roll back to.
    let mut plan = Plan::new(
        vec![Step::Arm(client::arm(1, id, revision))],
        (id, revision),
        true,
        vec![(id, revision, false), (id + 1, 1, false)],
    );
    // The reserve owns extents the store never writes, so its own release is worth measuring.
    plan.probe = vec![id + 1];
    plan
}

fn arm_refused(device: &mut Plain<'_>) -> Plan {
    let (id, revision) = device.seed(ObjectKind::UpdatePackage, &payload(4_096), "v2");
    // The harness device has no update path, so validation refuses and nothing is committed.
    Plan::new(vec![Step::Control(client::arm(1, id, revision))], (id, revision), true, vec![(id, revision, false)])
}

fn status_only(device: &mut Plain<'_>) -> Plan {
    let (id, revision) = device.seed(ObjectKind::Route, &payload(600), "asked about");
    Plan::new(vec![Step::Control(client::status(1, id, revision))], (id, revision), true, vec![(id, revision, false)])
}

#[test]
fn every_flow_survives_a_break_at_every_step() {
    let scenarios: [(&str, Build); 11] = [
        ("create", create),
        ("long create", long_create),
        ("replace", replace),
        ("remove", remove),
        ("download", download),
        ("cancelled upload", cancelled_upload),
        ("cancelled download", cancelled_download),
        ("paged listing", paged_listing),
        ("arm succeeds", arm_succeeds),
        ("arm refused", arm_refused),
        ("status", status_only),
    ];
    let mut points = 0;
    for (seed, (name, build)) in scenarios.into_iter().enumerate() {
        points += matrix(name, 100 + seed as u64, build);
    }
    assert_eq!(points, 45, "the matrix's own size, so a flow that stopped being covered is visible");
}

/// After a break the client restarts from zero, and for every break before the commit the restart
/// lands on a card that never heard of the first attempt.
#[test]
fn a_client_restarts_from_zero_after_every_break() {
    let bytes = payload(2_600);
    let records = client::stream_all(1, &bytes, 1_008);
    for cut in 0..records.len() {
        let disk = formatted_card(7);
        let mut device = boot(&disk);
        device.control(&client::put(1, 0, 0, &bytes, ROUTE, "attempt one"));
        for record in records.iter().take(cut) {
            device.stream(record);
        }
        device.link_lost();

        let answered = restart(&mut device, &bytes);
        assert_eq!(answered.u64_at(0), 1, "the restart still gets ObjectId 1: nothing claimed it");
        assert_eq!(answered.u64_at(8), 1, "and Revision 1: the broken attempt published nothing");
        assert_eq!(device.entries().len(), 1);
    }
}

/// The one hole in reconciling with `STATUS`: a create whose response was lost cannot be, because
/// the client never learned the assigned id. It restarts, and the cost is one duplicate object.
#[test]
fn a_create_whose_answer_was_lost_costs_one_duplicate_and_no_more() {
    let disk = formatted_card(8);
    let mut device = boot(&disk);
    let bytes = payload(2_600);
    device.control(&client::put(1, 0, 0, &bytes, ROUTE, "attempt one"));
    for record in client::stream_all(1, &bytes, 1_008) {
        device.stream(&record);
    }
    // The commit landed and the answer never arrived.
    device.link_lost();

    let answered = restart(&mut device, &bytes);
    assert_eq!(answered.u64_at(0), 2, "the restart is a second object, not a second revision");
    let entries = device.entries();
    assert_eq!(entries.len(), 2, "one duplicate, priced by §3.4");
    assert_eq!(entries[0].payload_crc, entries[1].payload_crc, "which a client matches on and removes");
}

fn restart(device: &mut Plain<'_>, bytes: &[u8]) -> Answer {
    device.control(&client::put(2, 0, 0, bytes, ROUTE, "attempt two"));
    let mut answer = None;
    for record in client::stream_all(2, bytes, 1_008) {
        let wire = device.stream(&record);
        if !wire.control.is_empty() {
            answer = Some(Answer::of(wire.answer()));
        }
    }
    answer.expect("the restart is answered")
}

/// A break with the card gone as well: the catalog a fresh mount finds is the one the break left,
/// and nothing the abandoned transfer wrote is reachable from it.
#[test]
fn a_remount_after_a_break_finds_the_card_the_break_left() {
    let disk: SparseDisk = formatted_card(9);
    let free = {
        let mut device = boot(&disk);
        let bytes = payload(2_600);
        device.control(&client::put(1, 0, 0, &bytes, ROUTE, "lost"));
        device.stream(&client::stream(1, 0, &bytes[..1_008]));
        device.link_lost();
        device.free_extents()
    };
    let mut remounted = boot(&disk);
    assert!(remounted.entries().is_empty(), "the abandoned bytes are anonymous");
    assert_eq!(remounted.free_extents(), free, "and the mount computes the same free map");
    let answer = Answer::of(remounted.control(&client::status(1, 1, 1)).answer());
    assert_eq!(answer.body[0], 0, "STATUS says absent, which is the truth the client restarts from");
}

#[test]
fn usb_map_replacement_preserves_the_old_map_and_other_objects_until_commit() {
    let usb = Ceilings::for_usb(4_112).expect("USB stream ceiling");
    let old_map = payload(1_700);
    let new_map = payload(12_000);
    let streams = client::stream_all(42, &new_map, 4_096);

    for cut in 0..=streams.len() + 1 {
        let disk = formatted_card(70 + cut as u64);
        let mut device = boot_on(&disk, usb);
        device.link_up(Link::Usb, usb);
        let (map_id, map_rev) = device.seed(ObjectKind::MapShard, &old_map, "installed");
        let route_a = device.seed(ObjectKind::Route, &payload(620), "route A").0;
        let route_b = device.seed(ObjectKind::Route, &payload(1_040), "route B").0;
        let ride = device.seed(ObjectKind::Ride, &payload(920), "finished ride").0;
        let recording = device.seed_recording(4_096).0;
        let preserved: Vec<_> = [route_a, route_b, ride, recording]
            .into_iter()
            .map(|id| (id, device.entry(id).unwrap(), device.read_object(id, 0)))
            .collect();

        if cut > 0 {
            let answer = device.control_on(
                Link::Usb,
                &client::put(42, map_id, map_rev, &new_map, ObjectKind::MapShard.value(), "replacement"),
            );
            assert!(answer.control.is_empty());
            for record in streams.iter().take(cut - 1) {
                device.stream_on(Link::Usb, record);
            }
        }
        device.link_lost_on(Link::Usb);
        drop(device);

        let remounted = boot(&disk);
        let committed = cut == streams.len() + 1;
        let expected = if committed { &new_map } else { &old_map };
        assert_eq!(remounted.read_object(map_id, 0).as_ref(), Some(expected), "cut {cut}: map bytes");
        assert_eq!(
            remounted.entry(map_id).unwrap().revision.0,
            map_rev + u64::from(committed),
            "cut {cut}: map revision"
        );
        for (id, metadata, bytes) in &preserved {
            assert_eq!(remounted.entry(*id), Some(*metadata), "cut {cut}: object {id} metadata");
            assert_eq!(remounted.read_object(*id, 0).as_ref(), bytes.as_ref(), "cut {cut}: object {id} bytes");
        }
    }
}
