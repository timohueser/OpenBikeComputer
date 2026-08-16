//! The two directions of the vector contract: the files match the producer, and the codec matches
//! the files.

use std::collections::BTreeSet;
use std::string::{String, ToString};
use std::vec::Vec;
use std::{format, println};

use super::*;
use crate::control::{ConfigBlock, MountClass};
use crate::error::ErrorBody;
use crate::frame::ControlFrame;
use crate::hello::{Capabilities, SubjectEntry};
use crate::intent::CanonicalIntent;
use crate::metadata::{MetadataEnvelope, MAX_CATALOG_ENVELOPE, MAX_PUT_ENVELOPE};
use crate::query::OperationStatus;
use crate::stream::StreamFrame;
use crate::{Request, Response};

/// Rebuilds `specs/vectors/device-object-v2/` from this producer.
///
/// Deliberately `#[ignore]`d, exactly like the S0 vector suite's own regenerate step: fixtures move
/// only when someone means them to.
#[test]
#[ignore = "writes specs/vectors/device-object-v2/; run deliberately after a spec change"]
fn regenerate() {
    let root = dir();
    if root.exists() {
        std::fs::remove_dir_all(&root).expect("clearing the suite directory");
    }
    let written = write_all().expect("writing the suite");
    println!("wrote {written} files to {}", root.display());
}

#[test]
fn checked_in_fixtures_match_the_producer() {
    let root = dir();
    let all = fixtures();
    for fixture in &all {
        let path = root.join(fixture.path());
        let checked_in = std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!(
                "fixture {} unreadable ({error}) — run `cargo test -p obc-link regenerate -- --ignored`",
                path.display()
            )
        });
        assert_eq!(
            checked_in,
            fixture.json,
            "fixture drift in {} — run `cargo test -p obc-link regenerate -- --ignored` if the change is deliberate",
            fixture.path()
        );
    }

    let manifest_path = root.join("manifest.json");
    let checked_in_manifest = std::fs::read_to_string(&manifest_path).expect("manifest.json");
    assert_eq!(checked_in_manifest, manifest(&all), "manifest drift — the CI guard exists to catch exactly this");

    // Drift in the other direction: a file on disk that the producer no longer emits.
    let expected: BTreeSet<String> = all.iter().map(|fixture| fixture.path()).collect();
    let mut found = BTreeSet::new();
    for category in [Category::Control, Category::Stream, Category::Negative, Category::Transcript] {
        let directory = root.join(category.directory());
        for entry in std::fs::read_dir(&directory).expect("suite subdirectory") {
            let entry = entry.expect("directory entry");
            found.insert(format!("{}/{}", category.directory(), entry.file_name().to_string_lossy()));
        }
    }
    assert_eq!(found, expected, "the suite directory holds a file the producer does not emit, or is missing one");
}

#[test]
fn every_fixture_name_is_unique_and_the_manifest_records_its_digest() {
    let all = fixtures();
    let mut names = BTreeSet::new();
    for fixture in &all {
        assert!(names.insert(fixture.name.clone()), "duplicate fixture name {}", fixture.name);
        assert_eq!(fixture.sha256().len(), 64);
    }
    let rendered = manifest(&all);
    for fixture in &all {
        assert!(rendered.contains(&fixture.sha256()), "{} is missing from the manifest", fixture.name);
        assert!(rendered.contains(&fixture.path()));
    }
    assert!(rendered.contains("\"suite\": \"device-object-v2\""));
    assert!(rendered.contains("\"wire_major\": 3"));
    assert!(rendered.contains("\"storage\": []"));
}

#[test]
fn the_production_codec_decodes_and_re_encodes_every_control_vector() {
    let mut buffer = std::vec![0u8; 1024];
    for vector in controls() {
        let record = vector.frame();
        let frame = ControlFrame::decode(&record)
            .unwrap_or_else(|error| panic!("{} failed frame decode: {error:?}", vector.name));
        assert_eq!(frame.opcode, vector.opcode);
        assert_eq!(frame.payload, &vector.payload[..]);

        if vector.direction == "request" {
            let request = Request::decode(&frame)
                .unwrap_or_else(|error| panic!("{} failed request decode: {error:?}", vector.name));
            let len = request.encode_payload(&mut buffer).expect("re-encode");
            assert_eq!(&buffer[..len], &vector.payload[..], "{} did not re-encode byte-exactly", vector.name);
        } else {
            let response = Response::decode(&frame)
                .unwrap_or_else(|error| panic!("{} failed response decode: {error:?}", vector.name));
            let len = response.encode_payload(&mut buffer).expect("re-encode");
            assert_eq!(&buffer[..len], &vector.payload[..], "{} did not re-encode byte-exactly", vector.name);
        }
    }
}

#[test]
fn the_production_codec_assigns_every_control_vector_the_semantic_body_the_fixture_states() {
    // The defect this test exists for, and the reason §1 asks for a semantic body at all: a codec
    // that reads two adjacent same-width fields the wrong way round round-trips every byte of
    // every fixture and is still wrong. Byte equality cannot see it; field equality can.
    //
    // The comparison is exact in both directions — every field the fixture states is checked, and
    // a field the codec reports that the fixture does not state fails too — so a body cannot be
    // partially checked and a fixture cannot quietly stop covering a field.
    let mut checked = 0usize;
    for vector in controls() {
        let record = vector.frame();
        let frame = ControlFrame::decode(&record).unwrap_or_else(|error| panic!("{}: {error:?}", vector.name));
        let observed = observed::body_of(&frame).unwrap_or_else(|error| panic!("{}: {error:?}", vector.name));
        let stated = vector.body();
        assert_eq!(
            observed.fields.iter().map(|(key, _)| key.clone()).collect::<BTreeSet<_>>(),
            stated.fields.iter().map(|(key, _)| key.clone()).collect::<BTreeSet<_>>(),
            "{}: the decoded field set differs from the fixture's semantic body",
            vector.name
        );
        for (key, value) in &stated.fields {
            let seen = observed.fields.iter().find(|(name, _)| name == key).map(|(_, value)| value);
            assert_eq!(seen, Some(value), "{}: field {key} decoded as {seen:?}, not {value:?}", vector.name);
        }
        checked += 1;
    }
    assert_eq!(checked, controls().len());
}

#[test]
fn every_control_fixture_on_disk_carries_a_semantic_body() {
    // The drift guard for the body itself: a fixture that lost its body, or a producer that
    // stopped emitting one, has to fail rather than silently weaken every suite that reads it.
    let root = dir();
    for vector in controls() {
        let path = root.join(format!("controls/{}.json", vector.name));
        let text = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("{} is missing", path.display()));
        assert!(text.contains("\"body\":"), "{} carries no semantic body", vector.name);
        for (key, _) in &vector.body().fields {
            assert!(text.contains(&format!("\"{key}\":")), "{} does not state field {key}", vector.name);
        }
    }
}

#[test]
fn the_production_codec_decodes_and_re_encodes_every_stream_vector() {
    let mut buffer = std::vec![0u8; 4096];
    for vector in streams() {
        let frame = StreamFrame::decode(&vector.record)
            .unwrap_or_else(|error| panic!("{} failed stream decode: {error:?}", vector.name));
        let len = frame.encode_into(&mut buffer).expect("re-encode");
        assert_eq!(&buffer[..len], &vector.record[..], "{} did not re-encode byte-exactly", vector.name);
    }
}

#[test]
fn the_production_codec_rejects_every_negative_vector_in_the_stated_category() {
    for vector in negatives() {
        let observed = match vector.target {
            NegativeTarget::ControlFrame => ControlFrame::decode(&vector.bytes).err(),
            NegativeTarget::ControlBody(_, response) => match ControlFrame::decode(&vector.bytes) {
                Ok(frame) => {
                    if response {
                        Response::decode(&frame).err()
                    } else {
                        Request::decode(&frame).err()
                    }
                }
                Err(error) => Some(error),
            },
            NegativeTarget::StreamFrame => StreamFrame::decode(&vector.bytes).err(),
            // The ceiling comes from the fixture's own declared class, never from the version byte
            // the envelope carries: §2.2 makes it a call-site fact.
            NegativeTarget::MetadataEnvelope(class) => MetadataEnvelope::decode(&vector.bytes, class.ceiling()).err(),
            NegativeTarget::ErrorBody => ErrorBody::decode(&vector.bytes).err(),
            NegativeTarget::CapabilitiesPayload => Capabilities::decode(&vector.bytes).err(),
            NegativeTarget::SubjectEntry => SubjectEntry::decode(&vector.bytes).err(),
            NegativeTarget::ConfigBlock => ConfigBlock::decode(&vector.bytes).err(),
            // Alone among the negative rows, this one is a policy-predicate test rather than a
            // codec-rejection test: the bytes decode fine and what refuses them is §16's admission
            // rule, so the expected error is synthesized from that predicate's answer.
            NegativeTarget::ResetStoreEcho(class) => {
                // The echo decodes as sixteen opaque bytes; what refuses it is §16's admission
                // rule, which needs the mount class and the StoreId the device currently reports.
                let request = crate::control::ResetStore::decode(&vector.bytes).expect("sixteen bytes");
                let mount_class = MountClass::from_u8(class).expect("a registered mount class");
                if request.echo_is_admissible(mount_class, crate::ids::StoreId::new(STORE)) {
                    None
                } else {
                    Some(crate::DecodeError::invalid_combination())
                }
            }
        };
        let observed = observed.unwrap_or_else(|| panic!("{} was accepted but must be rejected", vector.name));
        assert_eq!(
            (observed.category, observed.detail),
            (vector.category, vector.detail),
            "{} was rejected as {}/{} rather than {}/{}",
            vector.name,
            observed.category.name(),
            crate::error::detail_name(observed.category, 0, observed.detail),
            vector.category.name(),
            crate::error::detail_name(vector.category, 0, vector.detail),
        );
    }
}

#[test]
fn the_production_intent_builder_matches_every_canonical_golden() {
    use crate::draft::{BeginDraft, StartDraftPart};
    use crate::ids::{LogicalObjectId, OperationId, Revision, StoreId};
    use crate::metadata::SchemaClass;
    use crate::mutate::{AcknowledgeRideImported, DeleteObject, InstallUpdate, MutationTarget, SetMetadata};
    use crate::registry::{AbortReason, DraftPartKind, ObjectKind};
    use crate::upload::{AbortOperation, ResumePreference, StartUpload, Target};

    let store = StoreId::new(STORE);
    let payload = inventory::route_payload();
    let payload_crc = raw::crc32(&payload);
    let payload_len = payload.len() as u64;
    let goldens = intents();
    let find = |name: &str| goldens.iter().find(|vector| vector.name == name).expect("golden");

    let route_put_bytes = inventory::route_put(2);
    let create = StartUpload {
        operation_id: OperationId::new(OP_A),
        kind: ObjectKind::Route,
        target: Target::Create,
        resume: ResumePreference::ResumePermitted,
        declared_length: payload_len,
        expected_crc32: payload_crc,
        metadata: MetadataEnvelope::decode(&route_put_bytes, MAX_PUT_ENVELOPE).unwrap(),
    };
    assert_eq!(
        CanonicalIntent::for_start_upload(store, &create).bytes(),
        &find("intent-start-upload-create-route").bytes[..]
    );

    let replace_put_bytes = inventory::route_put(4);
    let replace = StartUpload {
        target: Target::Replace { logical_object_id: LogicalObjectId::new(9), expected_revision: Revision::new(41) },
        metadata: MetadataEnvelope::decode(&replace_put_bytes, MAX_PUT_ENVELOPE).unwrap(),
        ..create
    };
    assert_eq!(
        CanonicalIntent::for_start_upload(store, &replace).bytes(),
        &find("intent-start-upload-replace-route").bytes[..]
    );

    let manifest = inventory::manifest_payload();
    let begin = BeginDraft {
        parent_operation_id: OperationId::new(OP_PARENT),
        kind: ObjectKind::VolumeManifest,
        target: Target::Create,
        declared_manifest_length: manifest.len() as u64,
        declared_manifest_crc32: raw::crc32(&manifest),
        expected_part_count: 3,
    };
    assert_eq!(CanonicalIntent::for_begin_draft(store, &begin).bytes(), &find("intent-begin-draft").bytes[..]);

    let part_payload = inventory::draft_part_payload();
    let part = StartDraftPart {
        child_operation_id: OperationId::new(OP_CHILD),
        parent_operation_id: OperationId::new(OP_PARENT),
        part_kind: DraftPartKind::MapShard,
        part_key: 7,
        declared_length: part_payload.len() as u64,
        expected_crc32: raw::crc32(&part_payload),
        resume: ResumePreference::ResumePermitted,
    };
    assert_eq!(CanonicalIntent::for_start_draft_part(store, &part).bytes(), &find("intent-start-draft-part").bytes[..]);

    let target = MutationTarget {
        operation_id: OperationId::new(OP_B),
        kind: ObjectKind::Route,
        logical_object_id: LogicalObjectId::new(9),
        expected_revision: Revision::new(42),
    };
    assert_eq!(
        CanonicalIntent::for_delete_object(store, &DeleteObject { target }).bytes(),
        &find("intent-delete-object").bytes[..]
    );

    let patch_bytes = inventory::route_patch(Some(3), Some(true), Some("Kaiserstuhl loop"));
    let patch = MetadataEnvelope::decode(&patch_bytes, MAX_PUT_ENVELOPE).unwrap();
    assert_eq!(patch.schema_version, SchemaClass::Patch.version());
    assert_eq!(
        CanonicalIntent::for_set_metadata(store, &SetMetadata { target, patch }).bytes(),
        &find("intent-set-metadata").bytes[..]
    );

    let abort = AbortOperation {
        operation_id: OperationId::new(OP_ABORT),
        target_operation_id: OperationId::new(OP_A),
        reason: AbortReason::ClientCancelled,
    };
    assert_eq!(CanonicalIntent::for_abort_operation(store, &abort).bytes(), &find("intent-abort-operation").bytes[..]);

    let install = InstallUpdate {
        operation_id: OperationId::new(OP_A),
        logical_object_id: LogicalObjectId::new(3),
        expected_revision: Revision::new(70),
    };
    assert_eq!(CanonicalIntent::for_install_update(store, &install).bytes(), &find("intent-install-update").bytes[..]);

    let acknowledge = AcknowledgeRideImported {
        operation_id: OperationId::new(OP_B),
        logical_object_id: LogicalObjectId::new(5),
        expected_revision: Revision::new(61),
    };
    assert_eq!(
        CanonicalIntent::for_acknowledge_ride_imported(store, &acknowledge).bytes(),
        &find("intent-acknowledge-ride-imported").bytes[..]
    );

    // And the digest each golden records is the SHA-256 of exactly those bytes.
    for golden in &goldens {
        assert!(golden.bytes.len() >= 36);
        assert_eq!(&golden.bytes[..16], b"OBC-DOS3-INTENT\0");
        assert_eq!(&golden.bytes[16..32], &STORE[..]);
        assert_eq!(golden.bytes[34], 1);
        assert_eq!(golden.bytes[35], 0);
    }
    assert_eq!(
        CanonicalIntent::for_start_upload(store, &create).digest()[..],
        {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(&find("intent-start-upload-create-route").bytes);
            hasher.finalize()
        }[..]
    );
}

#[test]
fn every_registered_kind_schema_and_projection_appears_in_a_vector() {
    use crate::metadata::{Schema, SchemaClass};
    use crate::registry::ObjectKind;

    // Every catalog projection the registry defines decodes and validates against its schema.
    for (kind, bytes) in [
        (ObjectKind::Route, inventory::route_catalog("Kaiserstuhl loop", 2, Some(true), Some(1_700_000_000))),
        (ObjectKind::Trip, inventory::trip_catalog("Alpine crossing", 4)),
        (ObjectKind::Ride, inventory::ride_catalog(1_700_000_000, 5400, 42_000, true)),
        (ObjectKind::Weather, inventory::weather_catalog(42, 1_700_000_000, 1_700_086_400)),
        (ObjectKind::VolumeManifest, inventory::volume_catalog("Baden-Wurttemberg", false, 3)),
        (ObjectKind::UpdatePackage, inventory::update_catalog("1.4.2", 1, [0x7F; 32])),
    ] {
        let envelope = MetadataEnvelope::decode(&bytes, MAX_CATALOG_ENVELOPE).expect("canonical");
        let schema = Schema::lookup(kind, SchemaClass::Catalog).expect("registered");
        schema
            .validate(&envelope)
            .unwrap_or_else(|error| panic!("{} catalog projection rejected: {error:?}", kind.name()));
        assert!(bytes.len() <= schema.max_encoded_len);
    }

    // And every Put and patch schema.
    for (kind, class, bytes) in [
        (ObjectKind::Route, SchemaClass::Put, inventory::route_put(2)),
        (ObjectKind::Trip, SchemaClass::Put, inventory::trip_put()),
        (
            ObjectKind::Weather,
            SchemaClass::Put,
            inventory::weather_put(42, 480_000_000, 77_000_000, 50_000, 1_700_000_000, 1_700_086_400),
        ),
        (ObjectKind::Route, SchemaClass::Patch, inventory::route_patch(Some(3), Some(true), Some("Kaiserstuhl loop"))),
        (ObjectKind::VolumeManifest, SchemaClass::Patch, inventory::volume_patch(true)),
    ] {
        let envelope = MetadataEnvelope::decode(&bytes, MAX_PUT_ENVELOPE).expect("canonical");
        let schema = Schema::lookup(kind, class).expect("registered");
        schema
            .validate(&envelope)
            .unwrap_or_else(|error| panic!("{} {} rejected: {error:?}", kind.name(), class.name()));
    }
}

#[test]
fn every_error_category_and_registered_detail_appears_in_a_vector() {
    use crate::error::{detail_registry, ErrorCategory};

    let mut categories = BTreeSet::new();
    let mut details = BTreeSet::new();
    let mut guidances = BTreeSet::new();
    let mut owners = BTreeSet::new();

    for vector in controls() {
        if vector.flags & crate::frame::FrameFlags::ERROR == 0 {
            continue;
        }
        let body = ErrorBody::decode(&vector.payload).expect("every error vector decodes");
        categories.insert(body.category);
        details.insert((body.category, body.detail));
        guidances.insert(body.guidance);
        owners.insert(body.owner);
    }
    for vector in negatives() {
        categories.insert(vector.category);
        details.insert((vector.category, vector.detail));
    }

    for category in ErrorCategory::ALL {
        assert!(categories.contains(&category), "no vector carries category {}", category.name());
    }
    for row in detail_registry() {
        if row.code == 0 {
            continue;
        }
        if row.category == ErrorCategory::INVALID_DESCRIPTOR
            && row.code == crate::error::detail::descriptor::ZERO_REQUEST_ID
        {
            // §2 makes this one a close reason rather than a message: "it is never transmitted".
            // A response vector for it would freeze a frame no conforming device can send, so the
            // behaviour is pinned as a negative instead.
            assert!(
                negatives().iter().any(|vector| vector.name == "frame-zero-request-id"),
                "the zero-RequestId close must still be pinned as a negative"
            );
            continue;
        }
        assert!(details.contains(&(row.category, row.code)), "no vector carries {}/{}", row.category.name(), row.name);
    }
    for guidance in crate::error::RetryGuidance::ALL {
        assert!(guidances.contains(&guidance), "no vector carries guidance {}", guidance.name());
    }
    for owner in crate::error::Owner::ALL {
        assert!(owners.contains(&owner), "no vector carries owner {}", owner.name());
    }
}

#[test]
fn every_registered_domain_semantic_detail_appears_in_a_vector() {
    // The other half of the detail space. §6's rows are scoped by ObjectKind, so
    // `semanticValidation/4` is `payloadFactsMismatch` under weather and `downgradeDenied` under
    // update package: a client that reads the code without the namespace reports the wrong reason.
    let mut seen = BTreeSet::new();
    for vector in controls() {
        if vector.flags & crate::frame::FrameFlags::ERROR == 0 {
            continue;
        }
        let body = ErrorBody::decode(&vector.payload).expect("every error vector decodes");
        if body.category == crate::ErrorCategory::SEMANTIC_VALIDATION && body.detail_namespace != 0 {
            seen.insert((body.detail_namespace, body.detail));
        }
    }
    for row in crate::registry::semantic::table() {
        assert!(
            seen.contains(&(row.kind.to_u16(), row.code)),
            "no vector carries the {} semantic detail {}",
            row.kind.name(),
            row.name
        );
        // And the name a fixture would print really is the registry's, rather than a common-table
        // row that happens to share the number.
        assert_eq!(
            crate::error::detail_name(crate::ErrorCategory::SEMANTIC_VALIDATION, row.kind.to_u16(), row.code),
            row.name
        );
    }
}

#[test]
fn every_draft_part_kind_and_capability_policy_bit_appears_in_a_vector() {
    use crate::registry::{policy_flags, DraftPartKind};

    // Both are §2 requirements of the vectors contract — "every ObjectKind, DraftPartKind, metadata
    // version, catalog projection, capability operation/policy bit ... appears in at least one
    // positive or rejection vector" — and both were partly unmet: DraftPartKind 4 and policy bit 2
    // existed only in the registry.
    let mut part_kinds = BTreeSet::new();
    let mut policy_bits = 0u16;
    for vector in controls() {
        let record = vector.frame();
        let frame = ControlFrame::decode(&record).expect("decodes");
        if frame.flags.is_error() {
            continue;
        }
        if vector.direction == "request" {
            if let Ok(Request::StartDraftPart(part)) = Request::decode(&frame) {
                part_kinds.insert(part.part_kind);
            }
            continue;
        }
        match Response::decode(&frame) {
            Ok(Response::DraftPartAccepted(crate::upload::Disposition::Accepted(accepted))) => {
                part_kinds.insert(accepted.part_kind);
            }
            Ok(Response::DraftPage(page)) => {
                for entry in page.entries() {
                    part_kinds.insert(entry.part_kind);
                }
            }
            Ok(Response::Capabilities(page)) => {
                for entry in page.page.entries() {
                    policy_bits |= entry.policy_flags;
                    if let crate::hello::Subject::DraftPart(kind) = entry.subject {
                        part_kinds.insert(kind);
                    }
                }
            }
            _ => {}
        }
    }
    for kind in DraftPartKind::ALL {
        assert!(part_kinds.contains(&kind), "no vector carries DraftPartKind {}", kind.name());
    }
    assert_eq!(policy_bits, policy_flags::ALL, "the subject pages do not between them set every policy bit");
}

#[test]
fn every_device_control_mount_class_and_config_boundary_is_pinned() {
    let vectors = controls();
    for class in MountClass::ALL {
        let name = format!("device-status-mount-class-{}", class.to_u8());
        let vector = vectors.iter().find(|vector| vector.name == name).expect("one vector per mount class");
        let status = crate::control::DeviceStatus::decode(&vector.payload).expect("decodes");
        assert_eq!(status.mount_class, class);
        assert_eq!(status.store_id.is_zero(), !class.reports_store_id());
    }
    for name in
        ["config-block-full-name-response", "config-block-short-name-response", "config-block-empty-name-response"]
    {
        let vector = vectors.iter().find(|vector| vector.name == name).expect("config boundary");
        let block = ConfigBlock::decode(&vector.payload).expect("decodes");
        assert!(block.name().len() <= 32);
    }
    // No device-control vector carries an OperationId.
    for vector in &vectors {
        if vector.opcode.is_device_control() {
            assert!(vector.opcode.command_flag().is_some(), "{} must be gated by a command flag", vector.opcode.name());
        }
    }
}

#[test]
fn the_nine_required_transcripts_are_present_and_every_frame_in_them_decodes() {
    let expected = [
        // The nine flows issue #1358 names.
        "create-upload-publish-and-download",
        "replace-conflict-at-the-commit-lock",
        "lost-result-then-query-operation",
        "disconnect-reboot-and-resume",
        "abort-session-retains-work-abort-operation-abandons-it",
        "wrong-owner-cannot-advance-or-release-a-session",
        "download-pin-survives-replace-and-delete",
        "delete-lost-result-and-pinned-reader-continuity",
        "set-metadata-compare-and-swap-and-lost-result",
        // §5.4's result-window boundary and §5.11's draft machinery, both pure wire traffic.
        "result-window-eviction-boundary",
        "draft-begin-parts-finalize-and-paging",
    ];
    let all = transcripts();
    let names: Vec<String> = all.iter().map(|transcript| transcript.name.clone()).collect();
    for name in expected {
        assert!(names.contains(&name.to_string()), "transcript {name} is missing");
    }
    assert_eq!(all.len(), expected.len());

    let mut buffer = std::vec![0u8; 4096];
    for transcript in &all {
        for (index, event) in transcript.events.iter().enumerate() {
            let Some(record) = &event.record else {
                assert_eq!(event.channel, "injected", "{} event {index} has no record", transcript.name);
                continue;
            };
            match event.channel {
                "control" => {
                    let frame = ControlFrame::decode(record)
                        .unwrap_or_else(|error| panic!("{} event {index}: {error:?}", transcript.name));
                    let len = if frame.flags.is_response() {
                        Response::decode(&frame)
                            .unwrap_or_else(|error| panic!("{} event {index}: {error:?}", transcript.name))
                            .encode_payload(&mut buffer)
                            .expect("re-encode")
                    } else {
                        Request::decode(&frame)
                            .unwrap_or_else(|error| panic!("{} event {index}: {error:?}", transcript.name))
                            .encode_payload(&mut buffer)
                            .expect("re-encode")
                    };
                    assert_eq!(&buffer[..len], frame.payload, "{} event {index} did not re-encode", transcript.name);
                }
                "stream" => {
                    let frame = StreamFrame::decode(record)
                        .unwrap_or_else(|error| panic!("{} event {index}: {error:?}", transcript.name));
                    let len = frame.encode_into(&mut buffer).expect("re-encode");
                    assert_eq!(&buffer[..len], &record[..], "{} event {index} did not re-encode", transcript.name);
                }
                other => panic!("{} event {index} has an unknown channel {other}", transcript.name),
            }
        }
    }
}

/// Finds the payload of a transcript event by transcript name and event index.
fn transcript_payload(name: &str, index: usize) -> Vec<u8> {
    let transcripts = transcripts();
    let transcript = transcripts.iter().find(|transcript| transcript.name == name).expect("transcript");
    transcript.events[index].record.clone().expect("a record")
}

#[test]
fn transcript_lost_result_returns_the_identical_retained_result() {
    // Event 4 is the QueryOperation answer after the reconnect; event 6 is the same-intent replay.
    let queried = transcript_payload("lost-result-then-query-operation", 4);
    let replayed = transcript_payload("lost-result-then-query-operation", 6);

    let query_frame = ControlFrame::decode(&queried).unwrap();
    let OperationStatus::Committed(from_query) = OperationStatus::decode(query_frame.payload).unwrap() else {
        panic!("QueryOperation must answer Committed");
    };

    let replay_frame = ControlFrame::decode(&replayed).unwrap();
    let Response::UploadAccepted(crate::upload::Disposition::AlreadyTerminal(from_replay)) =
        Response::decode(&replay_frame).unwrap()
    else {
        panic!("the same-intent replay must carry disposition 1");
    };

    assert_eq!(from_query, from_replay, "the replay and the query must return the identical result");

    // And the different-intent reissue is a hard conflict with both claim-status bits clear.
    let conflict = transcript_payload("lost-result-then-query-operation", 8);
    let frame = ControlFrame::decode(&conflict).unwrap();
    let Response::Error(body) = Response::decode(&frame).unwrap() else { panic!("expected an error") };
    assert_eq!(body.category, crate::ErrorCategory::OPERATION_ID_CONFLICT);
    assert!(!body.durable_claim_exists());
    assert!(!body.claim_is_terminal());
    assert_eq!(body.guidance, crate::error::RetryGuidance::NEW_ID_FOR_NEW_INTENT);
}

#[test]
fn transcript_resume_reports_the_checkpointed_prefix_and_its_crc() {
    let checkpoint = transcript_payload("disconnect-reboot-and-resume", 1);
    let acceptance = transcript_payload("disconnect-reboot-and-resume", 5);

    let checkpoint_frame = ControlFrame::decode(&checkpoint).unwrap();
    let Response::CheckpointAccepted(checkpoint) = Response::decode(&checkpoint_frame).unwrap() else {
        panic!("expected a checkpoint");
    };
    assert_eq!(checkpoint.checkpoint_sequence, 1, "the first durable checkpoint is sequence 1");

    let acceptance_frame = ControlFrame::decode(&acceptance).unwrap();
    let Response::UploadAccepted(crate::upload::Disposition::Accepted(acceptance)) =
        Response::decode(&acceptance_frame).unwrap()
    else {
        panic!("expected an acceptance");
    };
    assert!(acceptance.flags.resumed_work());
    assert!(!acceptance.flags.restart_at_zero());
    assert_eq!(acceptance.durable_next_offset, checkpoint.durable_next_offset);
    assert_eq!(
        acceptance.finalized_prefix_crc32, checkpoint.finalized_prefix_crc32,
        "the acceptance must satisfy the comparison obligation without a further round trip"
    );
}

#[test]
fn transcript_replace_conflict_reports_the_current_revision_and_a_terminal_claim() {
    let refusal = transcript_payload("replace-conflict-at-the-commit-lock", 5);
    let frame = ControlFrame::decode(&refusal).unwrap();
    let Response::Error(body) = Response::decode(&frame).unwrap() else { panic!("expected an error") };
    assert_eq!(body.category, crate::ErrorCategory::REVISION_CONFLICT);
    assert_eq!(body.current_revision.get(), 43);
    assert!(body.durable_claim_exists() && body.claim_is_terminal());

    let status = transcript_payload("replace-conflict-at-the-commit-lock", 7);
    let frame = ControlFrame::decode(&status).unwrap();
    let OperationStatus::Aborted(retained) = OperationStatus::decode(frame.payload).unwrap() else {
        panic!("expected Aborted");
    };
    assert!(retained.text.is_empty(), "a retained Aborted body carries no diagnostic text");
    assert_eq!(retained.owner, crate::error::Owner::NONE);
    assert_eq!(retained.guidance, crate::error::RetryGuidance::REJECT_PERMANENTLY);
}

#[test]
fn transcript_wrong_owner_never_advances_or_releases_the_session() {
    for (index, expected_detail) in
        [(3usize, crate::error::detail::session::STALE_CONNECTION), (5, crate::error::detail::session::WRONG_LINK)]
    {
        let refusal = transcript_payload("wrong-owner-cannot-advance-or-release-a-session", index);
        let frame = ControlFrame::decode(&refusal).unwrap();
        let Response::Error(body) = Response::decode(&frame).unwrap() else { panic!("expected an error") };
        assert_eq!(body.category, crate::ErrorCategory::INVALID_SESSION);
        assert_eq!(body.detail, expected_detail);
    }
    let wrong_principal = transcript_payload("wrong-owner-cannot-advance-or-release-a-session", 7);
    let frame = ControlFrame::decode(&wrong_principal).unwrap();
    let Response::Error(body) = Response::decode(&frame).unwrap() else { panic!("expected an error") };
    assert_eq!(body.category, crate::ErrorCategory::AUTHORIZATION_FAILED, "authorization precedes status facts");
}

#[test]
fn transcript_download_pin_survives_and_releases_exactly_once() {
    let accepted = transcript_payload("download-pin-survives-replace-and-delete", 1);
    let frame = ControlFrame::decode(&accepted).unwrap();
    let Response::DownloadAccepted(accepted) = Response::decode(&frame).unwrap() else { panic!("expected acceptance") };
    assert_eq!(accepted.pinned_revision.get(), 42);
    assert_eq!(accepted.accepted_start_offset, 0);

    let released = transcript_payload("download-pin-survives-replace-and-delete", 6);
    let frame = ControlFrame::decode(&released).unwrap();
    assert_eq!(Response::decode(&frame).unwrap(), Response::DownloadFinished);

    let page = transcript_payload("download-pin-survives-replace-and-delete", 8);
    let frame = ControlFrame::decode(&page).unwrap();
    let Response::CatalogPage(page) = Response::decode(&frame).unwrap() else { panic!("expected a page") };
    assert_eq!(page.entry_count, 0);
    assert!(page.revision.get() > accepted.pinned_revision.get(), "the delete advanced the repository revision");
}

#[test]
fn transcript_delete_and_metadata_recover_their_results_by_query() {
    let deleted = transcript_payload("delete-lost-result-and-pinned-reader-continuity", 3);
    let frame = ControlFrame::decode(&deleted).unwrap();
    let OperationStatus::Committed(crate::result::ResultEnvelope::Object(result)) =
        OperationStatus::decode(frame.payload).unwrap()
    else {
        panic!("expected a committed ObjectResult");
    };
    assert_eq!(result.outcome, crate::registry::ObjectOutcome::Deleted);

    let replay = transcript_payload("delete-lost-result-and-pinned-reader-continuity", 5);
    let frame = ControlFrame::decode(&replay).unwrap();
    let Response::MutationResult(crate::result::ResultEnvelope::Object(replayed)) = Response::decode(&frame).unwrap()
    else {
        panic!("expected a replayed ObjectResult");
    };
    assert_eq!(replayed, result, "an identical reissue returns the same result and writes nothing");

    let metadata = transcript_payload("set-metadata-compare-and-swap-and-lost-result", 3);
    let frame = ControlFrame::decode(&metadata).unwrap();
    let OperationStatus::Committed(crate::result::ResultEnvelope::Object(result)) =
        OperationStatus::decode(frame.payload).unwrap()
    else {
        panic!("expected a committed ObjectResult");
    };
    assert_eq!(result.outcome, crate::registry::ObjectOutcome::MetadataChanged);

    let stale = transcript_payload("set-metadata-compare-and-swap-and-lost-result", 7);
    let frame = ControlFrame::decode(&stale).unwrap();
    let Response::Error(body) = Response::decode(&frame).unwrap() else { panic!("expected an error") };
    assert_eq!(body.category, crate::ErrorCategory::REVISION_CONFLICT);
    assert_eq!(body.current_revision.get(), 45);
}

#[test]
fn transcript_abort_session_retains_work_and_abort_operation_is_idempotent() {
    let detached = transcript_payload("abort-session-retains-work-abort-operation-abandons-it", 1);
    let frame = ControlFrame::decode(&detached).unwrap();
    assert_eq!(
        Response::decode(&frame).unwrap(),
        Response::SessionAborted(crate::upload::AbortSessionOutcome::Detached)
    );

    let progress = transcript_payload("abort-session-retains-work-abort-operation-abandons-it", 3);
    let frame = ControlFrame::decode(&progress).unwrap();
    let OperationStatus::InProgress(progress) = OperationStatus::decode(frame.payload).unwrap() else {
        panic!("expected InProgress");
    };
    assert_ne!(progress.flags & crate::query::progress_flags::RESUMABLE, 0, "resumable work survives the detach");
    assert_eq!(progress.flags & crate::query::progress_flags::SESSION_ATTACHED, 0, "no session is attached");

    // The two AbortResults differ only in the RequestId that correlates them, which is why the
    // comparison is over the payload rather than the record: RequestId is correlation, never
    // identity.
    let first = transcript_payload("abort-session-retains-work-abort-operation-abandons-it", 5);
    let second = transcript_payload("abort-session-retains-work-abort-operation-abandons-it", 7);
    let first = ControlFrame::decode(&first).unwrap();
    let second = ControlFrame::decode(&second).unwrap();
    assert_ne!(first.request_id, second.request_id);
    assert_eq!(
        Response::decode(&first).unwrap(),
        Response::decode(&second).unwrap(),
        "repeating the abort command is idempotent by its own OperationId"
    );
}

#[test]
fn transcript_create_publishes_one_head_that_the_catalog_and_a_download_agree_on() {
    let result = transcript_payload("create-upload-publish-and-download", 10);
    let frame = ControlFrame::decode(&result).unwrap();
    let Response::UploadResult(crate::result::ResultEnvelope::Object(published)) = Response::decode(&frame).unwrap()
    else {
        panic!("expected an ObjectResult");
    };

    let page = transcript_payload("create-upload-publish-and-download", 12);
    let frame = ControlFrame::decode(&page).unwrap();
    let Response::CatalogPage(page) = Response::decode(&frame).unwrap() else { panic!("expected a page") };
    let entry = page.entries().next().expect("one entry");
    assert_eq!(entry.logical_object_id, published.logical_object_id);
    assert_eq!(entry.revision, published.revision);
    assert_eq!(entry.length, published.length);
    assert_eq!(entry.crc32, published.crc32);

    let download = transcript_payload("create-upload-publish-and-download", 14);
    let frame = ControlFrame::decode(&download).unwrap();
    let Response::DownloadAccepted(accepted) = Response::decode(&frame).unwrap() else { panic!("expected acceptance") };
    assert_eq!(accepted.pinned_revision, published.revision);
    assert_eq!(accepted.total_length, published.length);
    assert_eq!(accepted.whole_source_crc32, published.crc32);
}

#[test]
fn transcript_result_window_holds_at_63_and_evicts_at_64() {
    // Event 3 is the query after 63 newer terminals; event 6 is the query after the 64th.
    let retained = transcript_payload("result-window-eviction-boundary", 3);
    let frame = ControlFrame::decode(&retained).unwrap();
    let OperationStatus::Committed(result) = OperationStatus::decode(frame.payload).unwrap() else {
        panic!("63 newer terminals must not evict");
    };
    let crate::result::ResultEnvelope::Object(result) = result else { panic!("expected an ObjectResult") };
    assert_eq!(result.revision.get(), 42);

    let evicted = transcript_payload("result-window-eviction-boundary", 6);
    let frame = ControlFrame::decode(&evicted).unwrap();
    assert_eq!(OperationStatus::decode(frame.payload).unwrap(), OperationStatus::Unknown);

    // And the reconciliation that follows is a catalog read, not a replay: no frame after the
    // eviction carries the spent OperationId.
    let transcripts = transcripts();
    let transcript = transcripts.iter().find(|t| t.name == "result-window-eviction-boundary").unwrap();
    for event in &transcript.events[7..] {
        let Some(record) = &event.record else { continue };
        assert!(!record.windows(16).any(|window| window == OP_A), "an evicted OperationId must never be replayed");
    }
}

#[test]
fn transcript_draft_publishes_one_release_and_refuses_a_second_parent() {
    // A second BeginDraft while a parent is open is an ownership refusal before any claim.
    let refusal = transcript_payload("draft-begin-parts-finalize-and-paging", 3);
    let frame = ControlFrame::decode(&refusal).unwrap();
    let Response::Error(body) = Response::decode(&frame).unwrap() else { panic!("expected an error") };
    assert_eq!(body.category, crate::ErrorCategory::BUSY);
    assert_eq!(body.detail, crate::error::detail::busy::DRAFT_PARENTS);
    assert_eq!(body.owner, crate::error::Owner::BLE);
    assert!(!body.durable_claim_exists(), "the refusal precedes any claim");

    // Sealing mints the ref; the accepted response before it carried none.
    let accepted = transcript_payload("draft-begin-parts-finalize-and-paging", 5);
    let frame = ControlFrame::decode(&accepted).unwrap();
    let Response::DraftPartAccepted(crate::upload::Disposition::Accepted(accepted)) = Response::decode(&frame).unwrap()
    else {
        panic!("expected a DraftPartAccepted");
    };
    assert_eq!(accepted.durable_next_offset, 0);

    let sealed = transcript_payload("draft-begin-parts-finalize-and-paging", 8);
    let frame = ControlFrame::decode(&sealed).unwrap();
    let Response::UploadResult(crate::result::ResultEnvelope::DraftPart(sealed)) = Response::decode(&frame).unwrap()
    else {
        panic!("expected a DraftPartResult");
    };
    assert!(!sealed.draft_part_ref.is_zero(), "sealing mints the reference");

    // The paged snapshot reports that same ref, and its draft revision moved twice: once for the
    // claim, once for the seal — never for a payload checkpoint.
    let page = transcript_payload("draft-begin-parts-finalize-and-paging", 10);
    let frame = ControlFrame::decode(&page).unwrap();
    let Response::DraftPage(page) = Response::decode(&frame).unwrap() else { panic!("expected a draft page") };
    assert_eq!(page.draft_revision, 3);
    assert_eq!(page.entries().len(), 1);
    assert_eq!(page.entries()[0].draft_part_ref, sealed.draft_part_ref);
    assert_eq!(page.entries()[0].state, crate::query::DraftPartState::Sealed);

    // Finalization publishes one logical head, and selection is a separate compare-and-swap.
    let published = transcript_payload("draft-begin-parts-finalize-and-paging", 15);
    let frame = ControlFrame::decode(&published).unwrap();
    let Response::UploadResult(crate::result::ResultEnvelope::Object(published)) = Response::decode(&frame).unwrap()
    else {
        panic!("expected an ObjectResult");
    };
    assert_eq!(published.kind, crate::registry::ObjectKind::VolumeManifest);
    assert_eq!(published.outcome, crate::registry::ObjectOutcome::Committed);

    let selected = transcript_payload("draft-begin-parts-finalize-and-paging", 17);
    let frame = ControlFrame::decode(&selected).unwrap();
    let Response::MutationResult(crate::result::ResultEnvelope::Object(selected)) = Response::decode(&frame).unwrap()
    else {
        panic!("expected an ObjectResult");
    };
    assert_eq!(selected.outcome, crate::registry::ObjectOutcome::MetadataChanged);
    assert!(selected.revision > published.revision);
}

#[test]
fn the_frame_limit_derivation_cases_match_the_production_derivation() {
    use crate::hello::negotiation::{ble_control_ceiling, control_frame, stream_frame, Limit};

    let vectors = derivations();
    let cases = &vectors[0].cases;
    assert_eq!(cases.len(), 6);
    for case in cases {
        let observed = match case.channel {
            "control" => {
                assert_eq!(ble_control_ceiling(case.link_value), case.ceiling, "ATT MTU {}", case.link_value);
                control_frame(case.client_max, case.device_max, case.ceiling)
            }
            "stream" => stream_frame(case.client_max, case.device_max, case.ceiling),
            other => panic!("unknown channel {other}"),
        };
        let expected = match case.outcome {
            "negotiated" => Limit::Negotiated(case.negotiated),
            "belowProtocolMinimum" => Limit::BelowProtocolMinimum,
            "undeliverable" => Limit::Undeliverable,
            other => panic!("unknown outcome {other}"),
        };
        assert_eq!(observed, expected, "{} case at link value {}", case.channel, case.link_value);
    }
}

#[test]
fn the_progress_matrix_covers_every_claim_family_and_phase_the_matrix_admits() {
    use crate::query::{OperationProgress, PROGRESS_LEN};
    use crate::registry::Phase;

    let rows = progress_matrix();
    // Every row decodes and satisfies the matrix's own shape rules.
    for row in &rows {
        let bytes = progress(row.namespace, row.phase, row.flags, row.kind, row.logical_id, row.offset);
        assert_eq!(bytes.len(), PROGRESS_LEN);
        let decoded = OperationProgress::decode(&bytes)
            .unwrap_or_else(|error| panic!("{} failed to decode: {error:?}", row.name));
        assert_eq!(decoded.phase.to_u8(), row.phase);
        if !decoded.logical_id_present() {
            assert_eq!(decoded.logical_object_id.get(), 0, "{}: ID-present clear means the ID is zero", row.name);
        }
        if decoded.namespace == crate::registry::SubjectNamespace::None {
            assert_eq!(decoded.subject_kind, 0, "{}: namespace none means kind zero", row.name);
        }
    }

    // Every phase the wire enum defines appears somewhere in the matrix.
    let phases: BTreeSet<u8> = rows.iter().map(|row| row.phase).collect();
    for phase in Phase::ALL {
        assert!(phases.contains(&phase.to_u8()), "no matrix row occupies phase {}", phase.name());
    }

    // And each claim family occupies exactly the phases §8.1 gives it.
    let phases_for = |prefix: &str| -> BTreeSet<u8> {
        rows.iter()
            .filter(|row| row.name.starts_with(&format!("query-operation-progress-{prefix}")))
            .map(|row| row.phase)
            .collect()
    };
    assert_eq!(phases_for("start-upload"), BTreeSet::from([0, 1, 2, 3, 4, 7]));
    assert_eq!(phases_for("draft-part"), BTreeSet::from([0, 1, 2, 3, 4, 7]));
    assert_eq!(phases_for("draft-parent"), BTreeSet::from([0, 1, 2, 3, 4, 6, 7]));
    assert_eq!(phases_for("delete"), BTreeSet::from([3, 4, 7]));
    assert_eq!(phases_for("set-metadata"), BTreeSet::from([3, 4, 7]));
    assert_eq!(phases_for("abort-command"), BTreeSet::from([7]));
    // InstallUpdate never enters aborting: §9 makes it non-cancellable from its durable claim.
    assert_eq!(phases_for("install-update"), BTreeSet::from([3, 4, 5]));
    assert_eq!(phases_for("acknowledge-ride"), BTreeSet::from([3, 4, 7]));
}

#[test]
fn every_finalized_prefix_crc_covers_exactly_the_prefix_its_message_reports() {
    // The defect this test exists for: a producer that clamps the CRC span to the whole object
    // emits one identical CRC for several different durable offsets, and a codec that hashes the
    // wrong span sails through. Membership in a set of plausible CRCs is not enough to catch that —
    // swapping two fixtures' CRCs would still satisfy it — so every fixture is recomputed against
    // its own family's bytes at its own reported offset.
    let mut checked = 0usize;
    for vector in controls() {
        let Some((family, offset, crc)) = finalized_prefix(&vector) else { continue };
        let payload = family.payload();
        assert!(
            offset <= payload.len() as u64,
            "{}: a durable offset past the object's own length is not a prefix",
            vector.name
        );
        assert_eq!(
            crc,
            raw::crc32(&payload[..offset as usize]),
            "{} reports offset {offset} with a CRC that is not that prefix's",
            vector.name
        );
        if offset == 0 {
            assert_eq!(crc, 0, "{}: a zero durable offset carries a zero CRC", vector.name);
        }
        checked += 1;
    }
    assert!(checked >= 8, "only {checked} finalized-prefix fixtures were reached");

    // And the three checkpoints really are three different prefixes, so no swap between them could
    // go unnoticed either.
    let route = inventory::route_payload();
    let granule = u64::from(FIXTURE_GRANULE);
    let distinct: BTreeSet<u32> = [granule, granule * 2, route.len() as u64]
        .iter()
        .map(|offset| raw::crc32(&route[..*offset as usize]))
        .collect();
    assert_eq!(distinct.len(), 3, "three different prefixes must hash to three different values");
    assert_ne!(
        raw::crc32(&route[..granule as usize]),
        raw::crc32(&route),
        "a prefix CRC must not equal the whole-object CRC"
    );
}

/// The other side of the semantic-body contract: the body as the *production codec* sees it.
///
/// Everything here reads decoded types — [`Request`], [`Response`], and the structures they carry —
/// and never the payload bytes. That is the whole point: the producer wrote the body from the
/// specification's byte tables, this reads it back out of the codec's own understanding, and the
/// test compares the two. A field the codec places at the wrong offset, or drops, or swaps with
/// its neighbour, cannot agree with the producer here no matter how perfectly it re-encodes.
mod observed {
    use std::format;

    use super::*;
    use crate::control::ConfigBlock;
    use crate::draft::{DraftParentState, DraftPartAccepted, FinalizeDraftAccepted};
    use crate::hello::CapabilityPage;
    use crate::metadata::MetadataEnvelope;
    use crate::query::{CatalogCursor, CatalogPage, DraftPage, OperationStatus, WeatherRequestContext};
    use crate::result::ResultEnvelope;
    use crate::upload::{Disposition, UploadAccepted};
    use crate::vectors::Body;

    /// The body of one whole control record, in whichever direction its flags name.
    pub fn body_of(frame: &ControlFrame<'_>) -> crate::Result<Body> {
        if frame.flags.is_error() {
            let Response::Error(error) = Response::decode(frame)? else { panic!("an error frame is an ErrorBody") };
            return Ok(error_body(&error));
        }
        if frame.flags.is_response() {
            Ok(response_body(&Response::decode(frame)?))
        } else {
            Ok(request_body(&Request::decode(frame)?))
        }
    }

    fn request_body(request: &Request<'_>) -> Body {
        let mut body = Body::new();
        match request {
            Request::Hello(hello) => {
                body.num("minimumMajor", hello.minimum_major);
                body.num("maximumMajor", hello.maximum_major);
                body.num("clientMaxControlFrame", hello.client_max_control_frame);
                body.num("clientMaxStreamFrame", hello.client_max_stream_frame);
                body.num("clientFeatureFlags", hello.client_feature_flags);
                body.num("pageKind", hello.page_kind.to_u8());
                body.num("pageIndex", hello.page_index);
            }
            Request::StartUpload(upload) => {
                body.hex("operationId", upload.operation_id.as_bytes());
                body.num("objectKind", upload.kind.to_u16());
                body.num("targetMode", upload.target.mode().to_u8());
                body.num("resume", upload.resume.to_u8());
                body.u64("logicalObjectId", upload.target.logical_object_id().get());
                body.u64("expectedRevision", upload.target.expected_revision().get());
                body.u64("declaredLength", upload.declared_length);
                body.num("expectedCrc32", upload.expected_crc32);
                nest(&mut body, "metadata.", metadata(&upload.metadata));
            }
            Request::CheckpointUpload(checkpoint) => {
                body.num("sessionId", checkpoint.session_id.get());
                body.u64("receivedNextOffset", checkpoint.received_next_offset);
            }
            Request::FinishUpload(finish) => body.num("sessionId", finish.session_id.get()),
            Request::StartDownload(download) => {
                body.num("objectKind", download.kind.to_u16());
                body.num("flags", if download.start_offset.is_some() { 1 << 1 } else { 0 });
                body.u64("logicalObjectId", download.logical_object_id.get());
                body.u64("startOffset", download.start_offset.unwrap_or(0));
            }
            Request::FinishDownload(finish) => {
                body.num("sessionId", finish.session_id.get());
                body.u64("receivedLength", finish.received_length);
                body.num("wholeSourceCrc32", finish.whole_source_crc32);
            }
            Request::AbortSession(abort) => {
                body.num("sessionId", abort.session_id.get());
                body.num("reason", abort.reason.to_u8());
            }
            Request::AbortOperation(abort) => {
                body.hex("operationId", abort.operation_id.as_bytes());
                body.hex("targetOperationId", abort.target_operation_id.as_bytes());
                body.num("reason", abort.reason.to_u8());
            }
            Request::BeginDraft(begin) => {
                body.hex("parentOperationId", begin.parent_operation_id.as_bytes());
                body.num("objectKind", begin.kind.to_u16());
                body.num("targetMode", begin.target.mode().to_u8());
                body.u64("logicalObjectId", begin.target.logical_object_id().get());
                body.u64("expectedRevision", begin.target.expected_revision().get());
                body.u64("declaredManifestLength", begin.declared_manifest_length);
                body.num("declaredManifestCrc32", begin.declared_manifest_crc32);
                body.num("expectedPartCount", begin.expected_part_count);
            }
            Request::StartDraftPart(part) => {
                body.hex("childOperationId", part.child_operation_id.as_bytes());
                body.hex("parentOperationId", part.parent_operation_id.as_bytes());
                body.num("partKind", part.part_kind.to_u16());
                body.u64("partKey", part.part_key);
                body.u64("declaredLength", part.declared_length);
                body.num("expectedCrc32", part.expected_crc32);
                body.num("resume", part.resume.to_u8());
            }
            Request::FinalizeDraft(finalize) => body.hex("parentOperationId", finalize.parent_operation_id.as_bytes()),
            Request::QueryOperation(query) => body.hex("operationId", query.operation_id.as_bytes()),
            Request::QueryCatalog(query) => {
                body.num("objectKind", query.kind.to_u16());
                body.num("flags", query.page.flags());
                body.u64("expectedRevision", query.page.expected_revision());
                nest(&mut body, "cursor.", cursor(&query.page.cursor()));
            }
            Request::QueryDraft(query) => {
                body.hex("parentOperationId", query.parent_operation_id.as_bytes());
                body.num("flags", query.page.flags());
                body.num("requestedLimit", query.requested_limit);
                body.u64("expectedRevision", query.page.expected_revision());
                nest(&mut body, "cursor.", cursor(&query.page.cursor()));
            }
            Request::QueryWeatherRequest | Request::GetDeviceStatus | Request::GetConfig => {}
            Request::DeleteObject(delete) => nest(&mut body, "", mutation_target(&delete.target)),
            Request::SetMetadata(set) => {
                nest(&mut body, "", mutation_target(&set.target));
                nest(&mut body, "patch.", metadata(&set.patch));
            }
            Request::InstallUpdate(install) => {
                body.hex("operationId", install.operation_id.as_bytes());
                body.u64("logicalObjectId", install.logical_object_id.get());
                body.u64("expectedRevision", install.expected_revision.get());
            }
            Request::AcknowledgeRideImported(acknowledge) => {
                body.hex("operationId", acknowledge.operation_id.as_bytes());
                body.u64("logicalObjectId", acknowledge.logical_object_id.get());
                body.u64("expectedRevision", acknowledge.expected_revision.get());
            }
            Request::SetConfig(config) => nest(&mut body, "", config_block(config)),
            Request::SetClock(clock) => {
                body.i64("epochSeconds", clock.epoch_seconds);
                body.num("source", clock.source.to_u8());
            }
            Request::ForgetBond(forget) => body.num("scope", forget.scope.to_u8()),
            Request::Echo(echo) => body.hex("payload", echo.payload),
            Request::ResetStore(reset) => body.hex("echoStoreId", reset.echoed_store_id.as_bytes()),
        }
        body
    }

    fn response_body(response: &Response<'_>) -> Body {
        let mut body = Body::new();
        match response {
            Response::Capabilities(page) => {
                body.num("selectedMajor", page.selected_major);
                body.num("storageFormatVersion", page.storage_format_version);
                body.num("statusFlags", page.status_flags);
                body.hex("storeId", page.store_id.as_bytes());
                body.num("negotiatedControlFrame", page.negotiated_control_frame);
                body.num("negotiatedStreamFrame", page.negotiated_stream_frame);
                body.num("checkpointGranule", page.checkpoint_granule);
                body.num("retainedResultCapacity", page.retained_result_capacity);
                body.num("metadataEnvelopeLimit", page.metadata_envelope_limit);
                body.num("catalogMetadataLimit", page.catalog_metadata_limit);
                body.num("protocolMinimumControlFrame", page.protocol_min_control_frame);
                body.num("protocolMinimumStreamFrame", page.protocol_min_stream_frame);
                body.num("linkKind", page.link_kind.to_u8());
                body.num("authenticated", u8::from(page.authenticated));
                body.num("capabilityRevision", page.capability_revision);
                body.num("commandFlags", page.command_flags);
                body.num("totalSubjectCount", page.total_subject_count);
                body.num("pageKind", page.page.kind().to_u8());
                body.num("pageIndex", page.page_index);
                body.num("returnedSubjectCount", page.page.entries().len() as i64);
                body.num("totalPages", page.total_pages);
                body.num("deviceWireMinor", page.device_wire_minor);
                match &page.page {
                    CapabilityPage::Resources(limits) => {
                        let mut inner = Body::new();
                        inner.num("codecVersion", crate::hello::ResourceLimits::CODEC_VERSION);
                        inner.num("blockLength", crate::hello::RESOURCE_LIMITS_LEN as i64);
                        inner.num("logicalCatalogHeads", limits.logical_catalog_heads);
                        inner.num("normalClaims", limits.normal_claims);
                        inner.num("uploadWorkSlots", limits.upload_work_slots);
                        inner.num("draftParents", limits.draft_parents);
                        inner.num("draftParts", limits.draft_parts);
                        inner.num("manifestChildren", limits.manifest_children);
                        inner.num("mountedFiles", limits.mounted_files);
                        inner.num("readerLeases", limits.reader_leases);
                        inner.num("retainedGenerations", limits.retained_generations);
                        inner.num("retainedResults", limits.retained_results);
                        inner.num("inactiveWorkHorizon", limits.inactive_work_horizon);
                        inner.u64("maxGenerationLength", limits.max_generation_length);
                        inner.u64("availableReservationBytes", limits.available_reservation_bytes);
                        inner.num("routeHeads", limits.route_heads);
                        inner.num("tripHeads", limits.trip_heads);
                        inner.num("rideHeads", limits.ride_heads);
                        inner.num("weatherHeads", limits.weather_heads);
                        inner.num("volumeManifestHeads", limits.volume_manifest_heads);
                        inner.num("updatePackageHeads", limits.update_package_heads);
                        inner.num("heavyStreamSessions", limits.heavy_stream_sessions);
                        inner.num("maintenanceClaims", limits.maintenance_claims);
                        inner.num("rideSlots", limits.ride_slots);
                        nest(&mut body, "resourceLimits.", inner);
                    }
                    CapabilityPage::Subjects { .. } => {
                        for (index, entry) in page.page.entries().iter().enumerate() {
                            let mut inner = Body::new();
                            inner.num("namespace", entry.subject.namespace());
                            inner.num("kindCode", entry.subject.kind_code());
                            inner.num("operationFlags", entry.operation_flags);
                            inner.num("policyFlags", entry.policy_flags);
                            inner.num("putSchemaVersion", entry.put_schema_version);
                            inner.num("patchSchemaVersion", entry.patch_schema_version);
                            inner.num("catalogSchemaVersion", entry.catalog_schema_version);
                            inner.u64("maxLength", entry.max_length);
                            nest(&mut body, &format!("subjects[{index}]."), inner);
                        }
                    }
                }
            }
            Response::UploadAccepted(accepted) => match accepted {
                UploadAccepted::Accepted(acceptance) => {
                    body.num("disposition", 0);
                    body.num("targetMode", acceptance.target_mode.to_u8());
                    body.num("flags", acceptance.flags.bits());
                    body.hex("operationId", acceptance.operation_id.as_bytes());
                    body.num("sessionId", acceptance.session_id.get());
                    body.u64("logicalObjectId", acceptance.logical_object_id.get());
                    body.u64("admissionRevision", acceptance.admission_revision.get());
                    body.u64("durableNextOffset", acceptance.durable_next_offset);
                    body.num("checkpointGranule", acceptance.checkpoint_granule);
                    body.num("maxStreamPayload", acceptance.max_stream_payload);
                    body.num("finalizedPrefixCrc32", acceptance.finalized_prefix_crc32);
                }
                UploadAccepted::AlreadyTerminal(envelope) => {
                    body.num("disposition", 1);
                    nest(&mut body, "result.", result_envelope(envelope));
                }
            },
            Response::CheckpointAccepted(checkpoint) => {
                body.num("sessionId", checkpoint.session_id.get());
                body.u64("durableNextOffset", checkpoint.durable_next_offset);
                body.num("finalizedPrefixCrc32", checkpoint.finalized_prefix_crc32);
                body.num("checkpointSequence", checkpoint.checkpoint_sequence);
            }
            Response::UploadResult(envelope) | Response::MutationResult(envelope) => {
                nest(&mut body, "", result_envelope(envelope))
            }
            Response::DownloadAccepted(accepted) => {
                body.hex("storeId", accepted.store_id.as_bytes());
                body.num("sessionId", accepted.session_id.get());
                body.u64("logicalObjectId", accepted.logical_object_id.get());
                body.u64("pinnedRevision", accepted.pinned_revision.get());
                body.u64("totalLength", accepted.total_length);
                body.num("wholeSourceCrc32", accepted.whole_source_crc32);
                body.u64("acceptedStartOffset", accepted.accepted_start_offset);
                body.num("maxStreamPayload", accepted.max_stream_payload);
            }
            Response::DownloadFinished | Response::BondForgotten => {}
            Response::SessionAborted(outcome) => body.num("outcome", outcome.encode()[0]),
            Response::BeginDraftAccepted(accepted) => match accepted {
                Disposition::Accepted(acceptance) => {
                    body.num("disposition", 0);
                    body.hex("parentOperationId", acceptance.parent_operation_id.as_bytes());
                    body.u64("draftRevision", acceptance.draft_revision);
                    body.num("expectedPartCount", acceptance.expected_part_count);
                    body.num(
                        "state",
                        match acceptance.state {
                            DraftParentState::Open => 0,
                        },
                    );
                }
                Disposition::AlreadyTerminal(envelope) => {
                    body.num("disposition", 1);
                    nest(&mut body, "result.", result_envelope(envelope));
                }
            },
            Response::DraftPartAccepted(accepted) => match accepted {
                DraftPartAccepted::Accepted(acceptance) => {
                    body.num("disposition", 0);
                    body.num("flags", acceptance.flags.bits());
                    body.hex("childOperationId", acceptance.child_operation_id.as_bytes());
                    body.hex("parentOperationId", acceptance.parent_operation_id.as_bytes());
                    body.num("sessionId", acceptance.session_id.get());
                    body.num("partKind", acceptance.part_kind.to_u16());
                    body.u64("partKey", acceptance.part_key);
                    body.u64("durableNextOffset", acceptance.durable_next_offset);
                    body.num("checkpointGranule", acceptance.checkpoint_granule);
                    body.num("maxStreamPayload", acceptance.max_stream_payload);
                    body.num("finalizedPrefixCrc32", acceptance.finalized_prefix_crc32);
                }
                DraftPartAccepted::AlreadyTerminal(envelope) => {
                    body.num("disposition", 1);
                    nest(&mut body, "result.", result_envelope(envelope));
                }
            },
            Response::FinalizeDraftAccepted(accepted) => match accepted {
                FinalizeDraftAccepted::Accepted(acceptance) => {
                    body.num("disposition", 0);
                    body.num("flags", acceptance.flags.bits());
                    body.hex("parentOperationId", acceptance.parent_operation_id.as_bytes());
                    body.num("sessionId", acceptance.session_id.get());
                    body.u64("logicalObjectId", acceptance.logical_object_id.get());
                    body.u64("admissionRevision", acceptance.admission_revision.get());
                    body.u64("durableManifestOffset", acceptance.durable_manifest_offset);
                    body.num("checkpointGranule", acceptance.checkpoint_granule);
                    body.num("maxStreamPayload", acceptance.max_stream_payload);
                    body.num("finalizedPrefixCrc32", acceptance.finalized_prefix_crc32);
                }
                FinalizeDraftAccepted::AlreadyTerminal(envelope) => {
                    body.num("disposition", 1);
                    nest(&mut body, "result.", result_envelope(envelope));
                }
            },
            Response::OperationStatus(status) => {
                body.num("state", status.state());
                match status {
                    OperationStatus::Unknown => {}
                    OperationStatus::InProgress(progress) => {
                        let mut inner = Body::new();
                        inner.num("namespace", progress.namespace.to_u8());
                        inner.num("phase", progress.phase.to_u8());
                        inner.num("flags", progress.flags);
                        inner.num("subjectKind", progress.subject_kind);
                        inner.u64("logicalObjectId", progress.logical_object_id.get());
                        inner.u64("durableOffset", progress.durable_offset);
                        nest(&mut body, "progress.", inner);
                    }
                    OperationStatus::Committed(envelope) => nest(&mut body, "result.", result_envelope(envelope)),
                    OperationStatus::Aborted(error) => nest(&mut body, "error.", error_body(error)),
                }
            }
            Response::CatalogPage(page) => nest(&mut body, "", catalog_page(page)),
            Response::DraftPage(page) => nest(&mut body, "", draft_page(page)),
            Response::WeatherRequestContext(context) => nest(&mut body, "", weather_context(context)),
            Response::DeviceStatus(status) => {
                body.num("firmwareMajor", status.firmware_major);
                body.num("firmwareMinor", status.firmware_minor);
                body.num("firmwarePatch", status.firmware_patch);
                body.num("hardwareRevision", status.hardware_revision);
                body.hex("deviceSerial", &status.device_serial);
                body.num("bootCount", status.boot_count);
                body.u64("uptimeSeconds", status.uptime_seconds);
                body.num("stackHighWater", status.stack_high_water);
                body.num("statusFlags", status.status_flags);
                body.num("mountClass", status.mount_class.to_u8());
                body.num("firmwareBuild", status.firmware_build);
                body.hex("storeId", status.store_id.as_bytes());
            }
            Response::Config(config) => nest(&mut body, "", config_block(config)),
            Response::ClockStatus(clock) => {
                body.i64("epochSeconds", clock.epoch_seconds);
                body.num("source", clock.source.to_u8());
                body.num("state", clock.state.to_u8());
            }
            Response::Echo(echo) => body.hex("payload", echo.payload),
            Response::ResetStoreResult(result) => body.hex("newStoreId", result.new_store_id.as_bytes()),
            Response::Error(error) => nest(&mut body, "", error_body(error)),
        }
        body
    }

    fn nest(body: &mut Body, prefix: &str, other: Body) {
        for (key, value) in other.fields {
            body.fields.push((format!("{prefix}{key}"), value));
        }
    }

    fn metadata(envelope: &MetadataEnvelope<'_>) -> Body {
        let mut body = Body::new();
        body.num("schemaId", envelope.schema_id);
        body.num("schemaVersion", envelope.schema_version);
        body.num("encodedFieldBytes", (envelope.encoded_len() - 8) as i64);
        body.num("fieldCount", envelope.field_count);
        for (index, field) in envelope.fields().enumerate() {
            body.num(format!("field[{index}].tag"), field.tag());
            body.hex(format!("field[{index}].value"), field.value);
        }
        body
    }

    fn cursor(cursor: &CatalogCursor) -> Body {
        let mut body = Body::new();
        body.u64("revision", cursor.revision);
        body.num("nextEntryIndex", cursor.next_entry_index);
        body.num("kindCode", cursor.kind_code);
        body.num("crc32", cursor.crc32);
        body
    }

    fn error_body(error: &crate::error::ErrorBody<'_>) -> Body {
        let mut body = Body::new();
        body.num("category", error.category.get());
        body.num("detailNamespace", error.detail_namespace);
        body.num("detail", error.detail);
        body.num("guidance", error.guidance.get());
        body.num("owner", error.owner.get());
        body.num("presence", error.presence);
        body.num("retryAfterMs", error.retry_after_ms);
        body.u64("expectedOffset", error.expected_offset);
        body.u64("currentRevision", error.current_revision.get());
        body.u64("requiredBytes", error.required_bytes);
        body.u64("availableBytes", error.available_bytes);
        body.num("textLength", error.text.len() as i64);
        body.hex("text", error.text);
        body
    }

    fn result_envelope(envelope: &ResultEnvelope) -> Body {
        let mut body = Body::new();
        body.num("resultType", envelope.result_type());
        match envelope {
            ResultEnvelope::Object(result) => {
                body.hex("operationId", result.operation_id.as_bytes());
                body.hex("storeId", result.store_id.as_bytes());
                body.num("objectKind", result.kind.to_u16());
                body.num("outcome", result.outcome.to_u16());
                body.u64("logicalObjectId", result.logical_object_id.get());
                body.u64("revision", result.revision.get());
                body.u64("length", result.length);
                body.num("crc32", result.crc32);
            }
            ResultEnvelope::DraftPart(result) => {
                body.hex("childOperationId", result.child_operation_id.as_bytes());
                body.hex("storeId", result.store_id.as_bytes());
                body.hex("parentOperationId", result.parent_operation_id.as_bytes());
                body.hex("draftPartRef", result.draft_part_ref.as_bytes());
                body.num("partKind", result.part_kind.to_u16());
                body.u64("partKey", result.part_key);
                body.u64("length", result.length);
                body.num("crc32", result.crc32);
            }
            ResultEnvelope::Abort(result) => {
                body.hex("operationId", result.operation_id.as_bytes());
                body.hex("storeId", result.store_id.as_bytes());
                body.hex("targetOperationId", result.target_operation_id.as_bytes());
                body.num("disposition", result.disposition.to_u8());
            }
        }
        body
    }

    fn mutation_target(target: &crate::mutate::MutationTarget) -> Body {
        let mut body = Body::new();
        body.hex("operationId", target.operation_id.as_bytes());
        body.num("objectKind", target.kind.to_u16());
        // The expected-revision flag is mandatory on both direct mutations, so the codec models it
        // as always set rather than as a field.
        body.num("flags", 1);
        body.u64("logicalObjectId", target.logical_object_id.get());
        body.u64("expectedRevision", target.expected_revision.get());
        body
    }

    fn catalog_page(page: &CatalogPage<'_>) -> Body {
        let mut body = Body::new();
        body.hex("storeId", page.store_id.as_bytes());
        body.num("objectKind", page.kind.to_u16());
        body.num("entryCount", page.entry_count);
        body.u64("revision", page.revision.get());
        nest(&mut body, "nextCursor.", cursor(&page.next_cursor));
        for (index, entry) in page.entries().enumerate() {
            let mut inner = Body::new();
            inner.u64("logicalObjectId", entry.logical_object_id.get());
            inner.u64("revision", entry.revision.get());
            inner.u64("length", entry.length);
            inner.num("crc32", entry.crc32);
            nest(&mut inner, "metadata.", metadata(&entry.metadata));
            nest(&mut body, &format!("entries[{index}]."), inner);
        }
        body
    }

    fn draft_page(page: &DraftPage) -> Body {
        let mut body = Body::new();
        body.hex("parentOperationId", page.parent_operation_id.as_bytes());
        body.u64("draftRevision", page.draft_revision);
        nest(&mut body, "nextCursor.", cursor(&page.next_cursor));
        body.num("entryCount", page.count);
        body.num("flags", page.flags);
        for (index, entry) in page.entries().iter().enumerate() {
            let mut inner = Body::new();
            inner.hex("childOperationId", entry.child_operation_id.as_bytes());
            inner.hex("draftPartRef", entry.draft_part_ref.as_bytes());
            inner.num("partKind", entry.part_kind.to_u16());
            inner.u64("partKey", entry.part_key);
            inner.num("state", entry.state.to_u8());
            inner.u64("durableOffset", entry.durable_offset);
            inner.u64("declaredLength", entry.declared_length);
            inner.num("crc32", entry.crc32);
            nest(&mut body, &format!("entries[{index}]."), inner);
        }
        body
    }

    fn weather_context(context: &WeatherRequestContext) -> Body {
        let mut body = Body::new();
        body.hex("storeId", context.store_id.as_bytes());
        body.u64("currentWeatherRequestId", context.current_request_id.get());
        body.u64("contextRevision", context.context_revision);
        body.num("flags", i64::from(u32::from(context.head_request_id.is_some())));
        body.u64("weatherLogicalObjectId", context.weather_logical_object_id.get());
        body.u64("repositoryRevision", context.repository_revision.get());
        body.u64("headWeatherRequestId", context.head_request_id.map_or(0, |id| id.get()));
        body.num("centreLatitudeE7", context.centre_latitude_e7);
        body.num("centreLongitudeE7", context.centre_longitude_e7);
        body.num("radiusMetres", context.radius_metres);
        body.i64("earliestIssuedUtc", context.earliest_issued_utc);
        body.i64("requiredValidUntilUtc", context.required_valid_until_utc);
        body.num("state", context.state.to_u8());
        body
    }

    fn config_block(config: &ConfigBlock) -> Body {
        let mut body = Body::new();
        body.num("codecVersion", ConfigBlock::CODEC_VERSION);
        body.num("blockLength", crate::control::CONFIG_BLOCK_LEN as i64);
        body.num("nameLength", config.name_len);
        body.num("unitFlags", config.unit_flags);
        body.num("weatherRefresh", config.weather_refresh.to_u8());
        body.hex("name", config.name());
        body
    }
}

/// Which of the suite's three payloads a message's durable offset is measured into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrefixFamily {
    /// The 3,000-byte route object.
    Route,
    /// The 65,536-byte draft part.
    DraftPart,
    /// The 264-byte volume manifest.
    Manifest,
}

impl PrefixFamily {
    fn payload(self) -> Vec<u8> {
        match self {
            PrefixFamily::Route => inventory::route_payload(),
            PrefixFamily::DraftPart => inventory::draft_part_payload(),
            PrefixFamily::Manifest => inventory::manifest_payload(),
        }
    }
}

/// The `(family, durable offset, finalized prefix CRC)` a message reports, when it reports one.
///
/// The opcode fixes the family: a checkpoint and an UploadAccepted measure into the route object,
/// a DraftPartAccepted into the part, and the FinalizeDraft acceptance into the manifest.
fn finalized_prefix(vector: &ControlVector) -> Option<(PrefixFamily, u64, u32)> {
    if vector.direction != "response" || vector.flags & crate::frame::FrameFlags::ERROR != 0 {
        // An error response on one of these opcodes carries an ErrorBody, whose expected-offset
        // field is a resume hint rather than a durable prefix of anything this suite holds.
        return None;
    }
    let payload = &vector.payload;
    let at = |offset: usize, crc: usize| -> Option<(u64, u32)> {
        Some((
            u64::from_le_bytes(payload.get(offset..offset + 8)?.try_into().ok()?),
            u32::from_le_bytes(payload.get(crc..crc + 4)?.try_into().ok()?),
        ))
    };
    let accepted = payload.first() == Some(&0);
    let (family, offset, crc) = match vector.opcode {
        crate::frame::Opcode::CheckpointUpload => (PrefixFamily::Route, 4, 12),
        crate::frame::Opcode::StartUpload if accepted => (PrefixFamily::Route, 40, 56),
        crate::frame::Opcode::StartDraftPart if accepted => (PrefixFamily::DraftPart, 52, 68),
        crate::frame::Opcode::FinalizeDraft if accepted => (PrefixFamily::Manifest, 40, 56),
        _ => return None,
    };
    let (offset, crc) = at(offset, crc)?;
    Some((family, offset, crc))
}
