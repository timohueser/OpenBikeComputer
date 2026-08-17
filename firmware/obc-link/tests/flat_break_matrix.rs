//! The break matrix: every opcode flow, cut at every protocol step boundary.
//!
//! `FLAT_Store_Protocol.md` §3.6 is the obligation this discharges: "**Any break before the commit
//! leaves the card as if nothing happened**: the allocation is released, the written bytes are
//! anonymous, the catalog is untouched, and the client restarts from zero. That holds for a cable
//! pull, a cancel, a CRC failure, a validator refusal, and a power cut alike."
//!
//! So each flow is run once to the end to establish the card **after** it, then run again for every
//! prefix of its records with the link torn down at that point. Three things must then hold, and
//! nothing weaker is admissible:
//!
//! 1. **The catalog's byte image is the one before the flow or the one after it.** Nothing in
//!    between, which is what "one commit, atomically visible" means when it is a claim about bytes.
//! 2. **No row leaked.** Free extents match whichever of those two states the card is in, both
//!    reservation rows are free again, and the hold table let go of what a download was reading —
//!    each measured, because a dropped `Allocation` or a dropped `Handle` releases nothing and the
//!    only symptom until the next mount is an extent that never comes back.
//! 3. **A following `STATUS` reconciles the client.** §3.4 is the whole reconcile path after a break,
//!    and its answer must be the catalog's own truth in every one of these states.

mod flat_harness;

use flat_harness::{boot, catalog_image, client, formatted_card, payload, Answer, Device};
use obc_link::flat::ObjectKind;
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
}

/// How a scenario sets its card up and what it then does.
type Build = fn(&mut Device<'_>) -> Plan;

/// A flow, and the object a client would reconcile against after a break.
struct Plan {
    steps: Vec<Step>,
    /// What the client asks `STATUS` about: the `ObjectId` and the `Revision` it expected.
    subject: (u64, u64),
}

/// Feeds `steps` of `plan` and reports what the engine sent.
fn feed(device: &mut Device<'_>, plan: &Plan, steps: usize) {
    for step in plan.steps.iter().take(steps) {
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
        }
    }
}

/// What a `STATUS` for `subject` must answer, read straight off the catalog.
fn truth(device: &Device<'_>, subject: (u64, u64)) -> u8 {
    match device.entry(subject.0) {
        None => 0,
        Some(entry) if entry.revision.0 == subject.1 => 1,
        Some(_) => 2,
    }
}

/// Runs one scenario at every break point and holds each result to the three rules above.
fn matrix(name: &str, seed: u64, build: Build) -> usize {
    // The card before the flow, and the card after it.
    let disk = formatted_card(seed);
    let mut device = boot(&disk);
    let plan = build(&mut device);
    let before = (catalog_image(&disk), device.free_extents());
    feed(&mut device, &plan, plan.steps.len());
    assert!(device.is_quiet(), "{name}: the engine is still busy after the whole flow");
    let after = (catalog_image(&disk), device.free_extents());
    drop(device);

    let breaks = plan.steps.len() + 1;
    for cut in 0..breaks {
        let disk = formatted_card(seed);
        let mut device = boot(&disk);
        let plan = build(&mut device);
        assert_eq!(catalog_image(&disk), before.0, "{name}: the scenario is not deterministic");
        feed(&mut device, &plan, cut);
        device.link_lost();

        let where_ = format!("{name}: broken after {cut} of {} records", plan.steps.len());
        assert!(device.is_quiet(), "{where_}: a transfer survived the link");
        let image = catalog_image(&disk);
        assert!(
            image == before.0 || image == after.0,
            "{where_}: the catalog is neither the state before the flow nor the state after it"
        );
        let expected_free = if image == before.0 { before.1 } else { after.1 };
        assert_eq!(device.free_extents(), expected_free, "{where_}: extents leaked or went missing");

        // Both reservation rows are free: a leaked row is invisible in the extent count once the
        // extents themselves came back, and it is one of only two.
        let first = device.hog(1_024);
        let second = device.hog(1_024);
        device.release(first);
        device.release(second);
        assert_eq!(device.free_extents(), expected_free, "{where_}: the reservation probe changed the card");

        // §3.4: the reconcile path answers the catalog's own truth.
        let answer = Answer::of(device.control(&client::status(0x5EED, plan.subject.0, plan.subject.1)).answer());
        assert!(!answer.is_error(), "{where_}: STATUS refused");
        assert_eq!(answer.body[0], truth(&device, plan.subject), "{where_}: STATUS does not reconcile");

        // The hold table let go: a handle the engine failed to close keeps the entry's extents out
        // of the allocator when it is removed.
        if device.entry(plan.subject.0).is_some() {
            let freed = device.remove_and_measure(plan.subject.0);
            assert!(freed > 0, "{where_}: a hold kept the entry's extents after it was removed");
        }
    }
    breaks
}

fn stream_steps(request: u32, bytes: &[u8]) -> Vec<Step> {
    client::stream_all(request, bytes, 1_008).into_iter().map(Step::Stream).collect()
}

fn create(_device: &mut Device<'_>) -> Plan {
    let bytes = payload(2_600);
    let mut steps = vec![Step::Control(client::put(1, 0, 0, &bytes, ROUTE, false, "created"))];
    steps.extend(stream_steps(1, &bytes));
    Plan { steps, subject: (1, 1) }
}

fn replace(device: &mut Device<'_>) -> Plan {
    let bytes = payload(1_200);
    let (id, revision) = device.seed(ObjectKind::Route, &payload(600), "first");
    let mut steps = vec![Step::Control(client::put(1, id, revision, &bytes, ROUTE, false, "second"))];
    steps.extend(stream_steps(1, &bytes));
    Plan { steps, subject: (id, revision + 1) }
}

fn retaining_replace(device: &mut Device<'_>) -> Plan {
    let bytes = payload(1_200);
    let (id, revision) = device.seed(ObjectKind::WeatherBundle, &payload(600), "yesterday");
    let mut steps = vec![Step::Control(client::put(1, id, revision, &bytes, WEATHER, true, "today"))];
    steps.extend(stream_steps(1, &bytes));
    Plan { steps, subject: (id, revision + 1) }
}

fn remove(device: &mut Device<'_>) -> Plan {
    let (id, revision) = device.seed(ObjectKind::Route, &payload(600), "doomed");
    Plan { steps: vec![Step::Control(client::remove(1, id, revision))], subject: (id, revision) }
}

/// A remove of an object that also has a retained revision: one commit takes both.
fn remove_with_retention(device: &mut Device<'_>) -> Plan {
    let bytes = payload(1_200);
    let (id, revision) = device.seed(ObjectKind::WeatherBundle, &payload(600), "yesterday");
    device.control(&client::put(9, id, revision, &bytes, WEATHER, true, "today"));
    for record in client::stream_all(9, &bytes, 1_008) {
        device.stream(&record);
    }
    assert_eq!(device.entries().len(), 2, "the setup left a retained revision");
    Plan { steps: vec![Step::Control(client::remove(1, id, revision + 1))], subject: (id, revision + 1) }
}

/// A create long enough that most of its break points are inside the stream, which is where a
/// staging buffer either releases what it holds or does not.
fn long_create(_device: &mut Device<'_>) -> Plan {
    let bytes = payload(8_000);
    let mut steps = vec![Step::Control(client::put(1, 0, 0, &bytes, ROUTE, false, "long"))];
    steps.extend(stream_steps(1, &bytes));
    Plan { steps, subject: (1, 1) }
}

fn download(device: &mut Device<'_>) -> Plan {
    let (id, revision) = device.seed(ObjectKind::Route, &payload(2_600), "served");
    // The control record emits the first payload record; two more and the answer follow.
    let steps = vec![Step::Control(client::get(1, id, 0)), Step::Pump, Step::Pump, Step::Pump];
    Plan { steps, subject: (id, revision) }
}

fn cancelled_upload(device: &mut Device<'_>) -> Plan {
    let bytes = payload(2_600);
    let (id, revision) = device.seed(ObjectKind::Route, &payload(600), "untouched");
    let records = client::stream_all(1, &bytes, 1_008);
    let steps = vec![
        Step::Control(client::put(1, 0, 0, &bytes, ROUTE, false, "abandoned")),
        Step::Stream(records[0].clone()),
        Step::Control(client::cancel(2, 1)),
        Step::Pump,
    ];
    Plan { steps, subject: (id, revision) }
}

fn cancelled_download(device: &mut Device<'_>) -> Plan {
    let (id, revision) = device.seed(ObjectKind::Route, &payload(2_600), "half served");
    let steps = vec![Step::Control(client::get(1, id, 0)), Step::Pump, Step::Control(client::cancel(2, 1)), Step::Pump];
    Plan { steps, subject: (id, revision) }
}

fn paged_listing(device: &mut Device<'_>) -> Plan {
    for index in 0..5 {
        device.seed(ObjectKind::Route, &payload(64), &format!("object {index}"));
    }
    let steps = vec![Step::Control(client::list(1, None)), Step::Control(client::list_from(2, None, (2, 1), 6))];
    Plan { steps, subject: (1, 1) }
}

fn arm_refused(device: &mut Device<'_>) -> Plan {
    let (id, revision) = device.seed(ObjectKind::UpdatePackage, &payload(4_096), "v2");
    // The harness device has no update path, so §4 step 1 refuses and nothing is committed.
    Plan { steps: vec![Step::Control(client::arm(1, id, revision))], subject: (id, revision) }
}

fn status_only(device: &mut Device<'_>) -> Plan {
    let (id, revision) = device.seed(ObjectKind::Route, &payload(600), "asked about");
    Plan { steps: vec![Step::Control(client::status(1, id, revision))], subject: (id, revision) }
}

#[test]
fn every_flow_survives_a_break_at_every_step() {
    let scenarios: [(&str, Build); 12] = [
        ("create", create),
        ("long create", long_create),
        ("replace", replace),
        ("retaining replace", retaining_replace),
        ("remove", remove),
        ("remove with retention", remove_with_retention),
        ("download", download),
        ("cancelled upload", cancelled_upload),
        ("cancelled download", cancelled_download),
        ("paged listing", paged_listing),
        ("arm refused", arm_refused),
        ("status", status_only),
    ];
    let mut points = 0;
    for (seed, (name, build)) in scenarios.into_iter().enumerate() {
        points += matrix(name, 100 + seed as u64, build);
    }
    assert_eq!(points, 49, "the matrix's own size, so a flow that stopped being covered is visible");
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

fn restart(device: &mut Device<'_>, bytes: &[u8]) -> Answer {
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
