//! The break matrix: every opcode flow, cut at every protocol step boundary.
//!
//! `FLAT_Store_Protocol.md` §3.6 is the obligation this discharges: "**Any break before the commit
//! leaves the card as if nothing happened**: the allocation is released, the written bytes are
//! anonymous, the catalog is untouched, and the client restarts from zero. That holds for a cable
//! pull, a cancel, a CRC failure, a validator refusal, and a power cut alike."
//!
//! So each flow is run once to the end to establish the card **after** it, then run again for every
//! prefix of its records with the link torn down at that point. Three things must then hold:
//!
//! 1. **The catalog's byte image is the one before the flow or the one after it.** Nothing in
//!    between, which is what "one commit, atomically visible" means when it is a claim about bytes.
//! 2. **No row leaked.** Free extents match whichever of those two states the card is in, both
//!    reservation rows are free again, and the hold table let go of what a download was reading —
//!    each measured, because a dropped `Allocation` or a dropped `Handle` releases nothing and the
//!    only symptom until the next mount is an extent that never comes back.
//! 3. **A following `STATUS` reconciles the client.** §3.4 is the whole reconcile path after a break,
//!    and all four fields of its answer must be the catalog's own truth in every one of these states.
//!
//! What that does **not** cover, stated so the guarantee is not read wider than it is: a break is
//! between two link records, so nothing here cuts *inside* a commit — that is the crash matrix's
//! (`obc-storage`'s `flat::crash`), which cuts every media operation of every durable path. And the
//! byte-image comparison sees the batch a commit applied, not the batch it was built from: a flow
//! whose commit composition is wrong in a way that produces the same bytes would pass, which is why
//! the retaining replace runs as a **stepped** flow with its `RETAINED` state produced inside the
//! matrix rather than seeded ahead of it.

mod flat_harness;

use flat_harness::{boot, catalog_image, client, formatted_card, payload, Answer, Plain};
use obc_link::flat::store::Policy;
use obc_link::flat::{ObjectId, ObjectKind, Revision};
use obc_storage::flat::sim::SparseDisk;

const ROUTE: u16 = 1;
const WEATHER: u16 = 4;

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

/// A device with an update path, for the arming flow. §4's two hooks, both satisfied.
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
    /// This is the matrix's independent oracle and the reason it is here rather than derived: every
    /// other assertion compares the card against *the same code's* own "after" run, so a commit
    /// built from the wrong batch would agree with itself. This says what the batch was supposed to
    /// produce.
    settled: Vec<(u64, u64, bool)>,
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

/// §3.4's whole answer for `subject`, read straight off the catalog: state, then the head's
/// revision, payload length and CRC — zero on all three when the object is absent.
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
    // each point must land on exactly. A flow may commit more than once — a stepped double retention
    // does — so "the state before or the state after" is not the shape of this; "the state the
    // records that landed produce" is.
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
    // The one hand-written expectation in here, and the reason it is hand-written: everything else
    // compares the card against another run of the same code, so a commit built from the wrong batch
    // would agree with itself. This says what the flow was supposed to leave behind.
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
        // 1. The catalog is exactly what the records that landed produced. A commit is atomically
        //    visible or it is not there, and there is no third image.
        assert_eq!(catalog_image(&disk), images[cut], "{where_}: the catalog is not the one those records produce");
        assert_eq!(settled(&device), catalogs[cut], "{where_}: the entries are not the ones those records produce");

        // 2. Nothing leaked. A fresh mount rebuilds the free map from the catalog and can see no
        //    reservation and no hold, so it is the answer the broken store must already agree with —
        //    a released allocation, a closed handle, and nothing held over.
        let expected_free = boot(&disk).free_extents();
        assert_eq!(device.free_extents(), expected_free, "{where_}: a reservation or a hold outlived the link");
        // Both reservation rows are free: a leaked row is invisible in the extent count once its
        // extents came back, and there are only two.
        let first = device.hog(1_024);
        let second = device.hog(1_024);
        device.release(first);
        device.release(second);
        assert_eq!(device.free_extents(), expected_free, "{where_}: the reservation probe changed the card");

        // 3. §3.4: the reconcile path answers the catalog's own truth, in all four of its fields — a
        //    state that agreed while the length or the CRC did not would send a client to re-download
        //    bytes it already has, or to trust bytes it does not.
        let answer = Answer::of(device.control(&client::status(0x5EED, plan.subject.0, plan.subject.1)).answer());
        assert!(!answer.is_error(), "{where_}: STATUS refused");
        let answered = (answer.body[0], answer.u64_at(4), answer.u64_at(12), answer.u32_at(20));
        assert_eq!(answered, truth(&device, plan.subject), "{where_}: STATUS does not reconcile");

        // The hold table let go: a handle the engine failed to close keeps the entry's extents out
        // of the allocator when it is removed. The subject is in the catalog in every state a flow
        // that never removes it can reach, so for those the probe is unconditional.
        let present = device.entry(plan.subject.0).is_some();
        if plan.subject_survives {
            assert!(present, "{where_}: the subject is not in the catalog and this flow never removes it");
        }
        if present {
            let freed = device.remove_and_measure(plan.subject.0);
            assert!(freed > 0, "{where_}: a hold kept the entry's extents after it was removed");
        }
    }
    breaks
}

fn stream_steps(request: u32, bytes: &[u8]) -> Vec<Step> {
    client::stream_all(request, bytes, 1_008).into_iter().map(Step::Stream).collect()
}

fn create(_device: &mut Plain<'_>) -> Plan {
    let bytes = payload(2_600);
    let mut steps = vec![Step::Control(client::put(1, 0, 0, &bytes, ROUTE, false, "created"))];
    steps.extend(stream_steps(1, &bytes));
    Plan { steps, subject: (1, 1), subject_survives: false, settled: vec![(1, 1, false)] }
}

/// A create long enough that most of its break points are inside the stream, which is where a
/// staging buffer either releases what it holds or does not.
fn long_create(_device: &mut Plain<'_>) -> Plan {
    let bytes = payload(8_000);
    let mut steps = vec![Step::Control(client::put(1, 0, 0, &bytes, ROUTE, false, "long"))];
    steps.extend(stream_steps(1, &bytes));
    Plan { steps, subject: (1, 1), subject_survives: false, settled: vec![(1, 1, false)] }
}

fn replace(device: &mut Plain<'_>) -> Plan {
    let bytes = payload(1_200);
    let (id, revision) = device.seed(ObjectKind::Route, &payload(600), "first");
    let mut steps = vec![Step::Control(client::put(1, id, revision, &bytes, ROUTE, false, "second"))];
    steps.extend(stream_steps(1, &bytes));
    Plan { steps, subject: (id, revision + 1), subject_survives: true, settled: vec![(id, revision + 1, false)] }
}

fn retaining_replace(device: &mut Plain<'_>) -> Plan {
    let bytes = payload(1_200);
    let (id, revision) = device.seed(ObjectKind::WeatherBundle, &payload(600), "yesterday");
    let mut steps = vec![Step::Control(client::put(1, id, revision, &bytes, WEATHER, true, "today"))];
    steps.extend(stream_steps(1, &bytes));
    Plan {
        steps,
        subject: (id, revision + 1),
        subject_survives: true,
        settled: vec![(id, revision, true), (id, revision + 1, false)],
    }
}

/// Two retaining replaces in one flow, so the second one's **three**-mutation commit — publish the
/// head, retain what it displaced, free what was retained before — is what the break points run
/// through. The `RETAINED` state is produced inside the flow rather than seeded ahead of it: a batch
/// missing its third mutation only shows up where the matrix can see the entry that should have gone.
fn stepped_double_retention(device: &mut Plain<'_>) -> Plan {
    let first = payload(1_200);
    let second = payload(900);
    let (id, revision) = device.seed(ObjectKind::WeatherBundle, &payload(600), "monday");
    let mut steps = vec![Step::Control(client::put(1, id, revision, &first, WEATHER, true, "tuesday"))];
    steps.extend(stream_steps(1, &first));
    steps.push(Step::Control(client::put(2, id, revision + 1, &second, WEATHER, true, "wednesday")));
    steps.extend(stream_steps(2, &second));
    Plan {
        steps,
        subject: (id, revision + 2),
        subject_survives: true,
        // Two entries, not three: a second retaining replace frees the first retained revision.
        settled: vec![(id, revision + 1, true), (id, revision + 2, false)],
    }
}

fn remove(device: &mut Plain<'_>) -> Plan {
    let (id, revision) = device.seed(ObjectKind::Route, &payload(600), "doomed");
    Plan {
        steps: vec![Step::Control(client::remove(1, id, revision))],
        subject: (id, revision),
        subject_survives: false,
        settled: vec![],
    }
}

/// A remove of an object that also has a retained revision: one commit takes both.
fn remove_with_retention(device: &mut Plain<'_>) -> Plan {
    let bytes = payload(1_200);
    let (id, revision) = device.seed(ObjectKind::WeatherBundle, &payload(600), "yesterday");
    device.control(&client::put(9, id, revision, &bytes, WEATHER, true, "today"));
    for record in client::stream_all(9, &bytes, 1_008) {
        device.stream(&record);
    }
    assert_eq!(device.entries().len(), 2, "the setup left a retained revision");
    Plan {
        steps: vec![Step::Control(client::remove(1, id, revision + 1))],
        subject: (id, revision + 1),
        subject_survives: false,
        settled: vec![],
    }
}

fn download(device: &mut Plain<'_>) -> Plan {
    let (id, revision) = device.seed(ObjectKind::Route, &payload(2_600), "served");
    // The control record emits the first payload record; two more and the answer follow.
    let steps = vec![Step::Control(client::get(1, id, 0)), Step::Pump, Step::Pump, Step::Pump];
    Plan { steps, subject: (id, revision), subject_survives: true, settled: vec![(id, revision, false)] }
}

fn cancelled_upload(device: &mut Plain<'_>) -> Plan {
    let bytes = payload(2_600);
    let (id, revision) = device.seed(ObjectKind::Route, &payload(600), "untouched");
    let records = client::stream_all(1, &bytes, 1_008);
    let steps = vec![
        Step::Control(client::put(1, 0, 0, &bytes, ROUTE, false, "abandoned")),
        Step::Stream(records[0].clone()),
        Step::Control(client::cancel(2, 1)),
        Step::Pump,
    ];
    Plan { steps, subject: (id, revision), subject_survives: true, settled: vec![(id, revision, false)] }
}

fn cancelled_download(device: &mut Plain<'_>) -> Plan {
    let (id, revision) = device.seed(ObjectKind::Route, &payload(2_600), "half served");
    let steps = vec![Step::Control(client::get(1, id, 0)), Step::Pump, Step::Control(client::cancel(2, 1)), Step::Pump];
    Plan { steps, subject: (id, revision), subject_survives: true, settled: vec![(id, revision, false)] }
}

fn paged_listing(device: &mut Plain<'_>) -> Plan {
    for index in 0..5 {
        device.seed(ObjectKind::Route, &payload(64), &format!("object {index}"));
    }
    let steps = vec![Step::Control(client::list(1, None)), Step::Control(client::list_from(2, None, (2, 1), 6))];
    Plan { steps, subject: (1, 1), subject_survives: true, settled: (1..=5).map(|id| (id, 1, false)).collect() }
}

/// §4's own hazard, which nothing else covers: `ARM` commits a rollback reserve and *then* writes the
/// boot handoff, so the break points run either side of the one commit it makes.
fn arm_succeeds(device: &mut Plain<'_>) -> Plan {
    let (id, revision) = device.seed(ObjectKind::UpdatePackage, &payload(4_096), "v2");
    Plan {
        steps: vec![Step::Arm(client::arm(1, id, revision))],
        subject: (id, revision),
        subject_survives: true,
        settled: vec![(id, revision, false), (id + 1, 1, false)],
    }
}

fn arm_refused(device: &mut Plain<'_>) -> Plan {
    let (id, revision) = device.seed(ObjectKind::UpdatePackage, &payload(4_096), "v2");
    // The harness device has no update path, so §4 step 1 refuses and nothing is committed.
    Plan {
        steps: vec![Step::Control(client::arm(1, id, revision))],
        subject: (id, revision),
        subject_survives: true,
        settled: vec![(id, revision, false)],
    }
}

fn status_only(device: &mut Plain<'_>) -> Plan {
    let (id, revision) = device.seed(ObjectKind::Route, &payload(600), "asked about");
    Plan {
        steps: vec![Step::Control(client::status(1, id, revision))],
        subject: (id, revision),
        subject_survives: true,
        settled: vec![(id, revision, false)],
    }
}

#[test]
fn every_flow_survives_a_break_at_every_step() {
    let scenarios: [(&str, Build); 14] = [
        ("create", create),
        ("long create", long_create),
        ("replace", replace),
        ("retaining replace", retaining_replace),
        ("stepped double retention", stepped_double_retention),
        ("remove", remove),
        ("remove with retention", remove_with_retention),
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
    assert_eq!(points, 57, "the matrix's own size, so a flow that stopped being covered is visible");
}

/// The other half of §3.6's sentence: after a break the client restarts from zero, and the restart
/// lands on a card that never heard of the first attempt — for every break **before** the commit.
#[test]
fn a_client_restarts_from_zero_after_every_break() {
    let bytes = payload(2_600);
    let records = client::stream_all(1, &bytes, 1_008);
    for cut in 0..records.len() {
        let disk = formatted_card(7);
        let mut device = boot(&disk);
        device.control(&client::put(1, 0, 0, &bytes, ROUTE, false, "attempt one"));
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

/// §3.4's one hole, stated as a test: a create whose response was lost cannot be reconciled with
/// `STATUS`, because the client never learned the assigned id. It restarts, the card takes the
/// upload again, and the cost is exactly one duplicate object — which the client then removes.
#[test]
fn a_create_whose_answer_was_lost_costs_one_duplicate_and_no_more() {
    let disk = formatted_card(8);
    let mut device = boot(&disk);
    let bytes = payload(2_600);
    device.control(&client::put(1, 0, 0, &bytes, ROUTE, false, "attempt one"));
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
    device.control(&client::put(2, 0, 0, bytes, ROUTE, false, "attempt two"));
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
        device.control(&client::put(1, 0, 0, &bytes, ROUTE, false, "lost"));
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
