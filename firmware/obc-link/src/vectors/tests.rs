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
            NegativeTarget::MetadataEnvelope(ceiling) => MetadataEnvelope::decode(&vector.bytes, ceiling).err(),
            NegativeTarget::ErrorBody => ErrorBody::decode(&vector.bytes).err(),
            NegativeTarget::CapabilitiesPayload => Capabilities::decode(&vector.bytes).err(),
            NegativeTarget::SubjectEntry => SubjectEntry::decode(&vector.bytes).err(),
            NegativeTarget::ConfigBlock => ConfigBlock::decode(&vector.bytes).err(),
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

    let begin = BeginDraft {
        parent_operation_id: OperationId::new(OP_PARENT),
        kind: ObjectKind::VolumeManifest,
        target: Target::Create,
        declared_manifest_length: 96 + 56 * 3,
        declared_manifest_crc32: 0x0F0F_0F0F,
        expected_part_count: 3,
    };
    assert_eq!(CanonicalIntent::for_begin_draft(store, &begin).bytes(), &find("intent-begin-draft").bytes[..]);

    let part = StartDraftPart {
        child_operation_id: OperationId::new(OP_CHILD),
        parent_operation_id: OperationId::new(OP_PARENT),
        part_kind: DraftPartKind::MapShard,
        part_key: 7,
        declared_length: 65_536,
        expected_crc32: 0x99AA_BBCC,
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
        "create-upload-publish-and-download",
        "replace-conflict-at-the-commit-lock",
        "lost-result-then-query-operation",
        "disconnect-reboot-and-resume",
        "abort-session-retains-work-abort-operation-abandons-it",
        "wrong-owner-cannot-advance-or-release-a-session",
        "download-pin-survives-replace-and-delete",
        "delete-lost-result-and-pinned-reader-continuity",
        "set-metadata-compare-and-swap-and-lost-result",
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
