#![cfg(feature = "client")]

use obc_flat_device::{crc32, formatted_card, Device, Reaction, STORE, TOTAL_BLOCKS};
use obc_link::flat::{
    client::{Action, Client, Error, Event, Options, Outcome},
    wire::*,
    ArchiveSource, Ceilings, Channel, DisplayName, Link, ObjectId, ObjectKind, Revision, StoreId,
};
use obc_storage::flat::sim::SparseDisk;

struct Session<'a> {
    client: Client,
    device: Device<&'a SparseDisk>,
    source: Vec<u8>,
    sink: Vec<u8>,
    progress: Vec<(u64, u64)>,
    answer_first: bool,
    lose_commit: bool,
    cancel_after: Option<u64>,
    mutate_listing: bool,
    allow_arm: bool,
    now: u64,
}

impl<'a> Session<'a> {
    fn new(disk: &'a SparseDisk) -> Self {
        let ceilings = Ceilings::new(256, 128).unwrap();
        Self {
            client: Client::new(ceilings, Options::default()),
            device: Device::boot_on(disk, Link::Ble, ceilings),
            source: Vec::new(),
            sink: Vec::new(),
            progress: Vec::new(),
            answer_first: true,
            lose_commit: false,
            cancel_after: None,
            mutate_listing: false,
            allow_arm: false,
            now: 0,
        }
    }

    fn deliver(&mut self, reaction: Reaction) {
        match reaction {
            Reaction::Send { channel: Channel::Control, bytes } | Reaction::SendAndReboot { bytes } => {
                if self.lose_commit
                    && matches!(
                        decode_response(&bytes).unwrap().1,
                        Response::Put(_) | Response::Remove(_) | Response::ArchiveRide(_) | Response::Arm { .. }
                    )
                {
                    self.lose_commit = false;
                    self.device.link_down(Link::Ble);
                    self.client.event(Event::LinkLost, self.now);
                } else {
                    if self.mutate_listing
                        && matches!(decode_response(&bytes).unwrap().1, Response::List(page) if page.more)
                    {
                        self.mutate_listing = false;
                        self.device.seed(ObjectKind::Route as u16, &[1, 2], "new route");
                    }
                    self.client.event(Event::Control(&bytes), self.now);
                }
            }
            Reaction::Send { channel: Channel::Stream, bytes } => self.client.event(Event::Stream(&bytes), self.now),
            Reaction::Idle => {}
            Reaction::Close(_) => panic!("client emitted an unanswerable frame"),
        }
    }

    fn run(&mut self, request: Request) -> Result<Outcome, Error> {
        self.progress.clear();
        self.client.start(request, self.now).unwrap();
        self.pump()
    }

    fn pump(&mut self) -> Result<Outcome, Error> {
        for _ in 0..10_000 {
            self.now += 1;
            if let Some(action) = self.client.next_action() {
                match action {
                    Action::Send { token, channel, record } => {
                        let reaction = match channel {
                            Channel::Control if self.allow_arm => self.device.on_control_with(
                                Link::Ble,
                                &mut obc_flat_device::AllowArm { reserve: 4096 },
                                &record,
                            ),
                            Channel::Control => self.device.on_control(&record),
                            Channel::Stream => self.device.on_stream(&record),
                        };
                        if self.answer_first {
                            self.deliver(reaction);
                            self.client.event(Event::Written(token), self.now);
                        } else {
                            self.client.event(Event::Written(token), self.now);
                            self.deliver(reaction);
                        }
                    }
                    Action::ReadSource { token, offset, max_len } => {
                        let start = offset as usize;
                        let bytes = &self.source[start..(start + max_len).min(self.source.len())];
                        self.client.event(Event::Source { token, offset, bytes }, self.now);
                    }
                    Action::WriteSink { token, offset, bytes } => {
                        assert_eq!(offset, self.sink.len() as u64);
                        self.sink.extend_from_slice(&bytes);
                        self.client.event(Event::SinkWritten { token, offset, len: bytes.len() }, self.now);
                    }
                    Action::ResetSink => self.sink.clear(),
                    Action::Progress { done, total } => {
                        self.progress.push((done, total));
                        if self.cancel_after.is_some_and(|limit| done >= limit) {
                            self.cancel_after = None;
                            self.client.event(Event::Cancel, self.now);
                        }
                    }
                    Action::ResetChannels => {}
                    Action::Restore => {
                        let ceilings = self.device.ceilings();
                        self.device.link_up(Link::Ble, ceilings);
                        self.client.event(Event::Restored(ceilings), self.now);
                    }
                    Action::Complete(result) => return result,
                }
            } else {
                let reaction = self.device.poll();
                self.deliver(reaction);
                self.client.event(Event::Tick, self.now);
            }
        }
        panic!("client did not complete");
    }
}

fn put(id: ObjectId, expected: Revision, bytes: &[u8]) -> Request {
    Request::Put(PutRequest {
        id,
        expected,
        payload_len: bytes.len() as u64,
        payload_crc: crc32(bytes),
        kind: ObjectKind::Route,
        name: DisplayName::new("A route").unwrap(),
    })
}

fn first_put(session: &mut Session<'_>) -> TransferResponse {
    session.source = (0..900).map(|n| n as u8).collect();
    let request = put(ObjectId::NONE, Revision::HEAD, &session.source);
    let Outcome::Put(result) = session.run(request).unwrap() else {
        panic!("PUT result");
    };
    result
}

#[test]
fn real_card_upload_download_status_and_remove_preserve_bytes() {
    let disk = formatted_card(TOTAL_BLOCKS, 1);
    for answer_first in [false, true] {
        let mut s = Session::new(&disk);
        s.answer_first = answer_first;
        let result = first_put(&mut s);
        assert_eq!(s.progress.last(), Some(&(900, 900)));
        assert!(s.progress.windows(2).all(|p| p[0].0 < p[1].0));
        assert_eq!(s.device.read_object(result.id.0, result.revision.0), Some(s.source.clone()));
        let get = s.run(Request::Get(GetRequest { id: result.id, revision: result.revision })).unwrap();
        assert_eq!(get, Outcome::Get(result));
        assert_eq!(s.sink, s.source);
        assert!(matches!(
            s.run(Request::Status(StatusRequest { id: result.id, revision: result.revision })),
            Ok(Outcome::Status(StatusResponse { state: ObjectState::Committed, .. }))
        ));
        assert!(matches!(
            s.run(Request::Remove(RemoveRequest { id: result.id, expected: result.revision })),
            Ok(Outcome::Remove { sequence: Some(_) })
        ));
        assert!(s.device.read_object(result.id.0, 0).is_none());
    }
}

#[test]
fn listing_pages_a_real_catalog_in_order_and_keeps_one_snapshot() {
    let disk = formatted_card(TOTAL_BLOCKS, 2);
    let mut s = Session::new(&disk);
    for _ in 0..5 {
        first_put(&mut s);
    }
    let Outcome::Catalog { store, entries, .. } =
        s.run(Request::List(ListRequest { kind: Some(ObjectKind::Route), cursor: None })).unwrap()
    else {
        panic!("LIST result");
    };
    assert_eq!(store, StoreId(STORE.0));
    assert_eq!(entries.len(), 5);
    assert!(entries.windows(2).all(|pair| pair[0].id < pair[1].id));
}

#[test]
fn lost_create_replace_and_remove_answers_reconcile_on_the_card() {
    let disk = formatted_card(TOTAL_BLOCKS, 3);
    let mut s = Session::new(&disk);
    s.lose_commit = true;
    let initial = first_put(&mut s);
    s.source = vec![73; 445];
    s.lose_commit = true;
    let request = put(initial.id, initial.revision, &s.source);
    let Outcome::Put(replacement) = s.run(request).unwrap() else {
        panic!("recovered PUT");
    };
    assert_eq!(replacement.revision, Revision(2));
    assert_eq!(s.device.read_object(initial.id.0, 0), Some(s.source.clone()));
    s.lose_commit = true;
    assert_eq!(
        s.run(Request::Remove(RemoveRequest { id: initial.id, expected: replacement.revision })),
        Ok(Outcome::Remove { sequence: None })
    );
}

#[test]
fn formatting_and_archive_receipt_use_the_existing_engine() {
    let disk = formatted_card(TOTAL_BLOCKS, 4);
    let mut s = Session::new(&disk);
    let id = s.device.finish_recording(&[1, 2, 3], "ride");
    let source = ArchiveSource {
        store: StoreId(STORE.0),
        id: ObjectId(id.id.0),
        revision: Revision(id.revision.0),
        payload_len: 3,
        payload_crc: crc32(&[1, 2, 3]),
    };
    s.lose_commit = true;
    assert!(matches!(s.run(Request::ArchiveRide(source)), Ok(Outcome::ArchiveRide(_))));
    let replacement = StoreId([0x31; 16]);
    assert_eq!(
        s.run(Request::Format(FormatRequest { expected: StoreId(STORE.0), replacement })),
        Ok(Outcome::Format(replacement))
    );
    assert_eq!(s.client.store_id(), Some(replacement));
}

#[test]
fn all_request_types_round_trip_through_the_shared_codec() {
    let id = ObjectId(2);
    let revision = Revision(3);
    let store = StoreId([7; 16]);
    let requests = [
        Request::List(ListRequest {
            kind: Some(ObjectKind::Route),
            cursor: Some(ListCursor { id, revision, sequence: 4 }),
        }),
        Request::Status(StatusRequest { id, revision }),
        Request::Get(GetRequest { id, revision }),
        put(id, revision, &[4, 5]),
        Request::Remove(RemoveRequest { id, expected: revision }),
        Request::Cancel(CancelRequest { transfer: RequestId(6) }),
        Request::Arm(ArmRequest { package: id, expected: revision }),
        Request::Format(FormatRequest { expected: store, replacement: StoreId([8; 16]) }),
        Request::ArchiveRide(ArchiveSource { store, id, revision, payload_len: 2, payload_crc: 5 }),
    ];
    for request in requests {
        let mut bytes = [0; MAX_REQUEST_LEN];
        let len = encode_request(&mut bytes, RequestId(22), request).unwrap();
        assert_eq!(decode_request(&bytes[..len]).unwrap().1, request);
        for short in 0..len {
            assert!(decode_request(&bytes[..short]).is_err());
        }
    }
}

#[test]
fn arm_reports_the_engine_handoff_and_does_not_repeat_an_unknown_outcome() {
    for lost_answer in [false, true] {
        let disk = formatted_card(TOTAL_BLOCKS, 7);
        let mut s = Session::new(&disk);
        let package = s.device.seed(ObjectKind::UpdatePackage as u16, &[1, 2, 3], "update");
        s.allow_arm = true;
        s.lose_commit = lost_answer;
        let result =
            s.run(Request::Arm(ArmRequest { package: ObjectId(package.id.0), expected: Revision(package.revision.0) }));
        if lost_answer {
            assert_eq!(result, Err(Error::OutcomeUnknown));
        } else {
            assert!(matches!(result, Ok(Outcome::Arm { reserve, .. }) if reserve.0 != 0));
        }
        let Outcome::Catalog { entries, .. } =
            s.run(Request::List(ListRequest { kind: Some(ObjectKind::RollbackReserve), cursor: None })).unwrap()
        else {
            panic!("reserve catalogue");
        };
        assert_eq!(entries.len(), 1);
    }
}

#[test]
fn cancelled_replacement_preserves_the_old_object_and_reuses_the_link() {
    let disk = formatted_card(TOTAL_BLOCKS, 5);
    let mut s = Session::new(&disk);
    let initial = first_put(&mut s);
    let old = s.source.clone();
    s.source = vec![41; 1500];
    s.cancel_after = Some(224);
    let request = put(initial.id, initial.revision, &s.source);
    assert_eq!(s.run(request), Err(Error::Cancelled));
    assert_eq!(s.device.read_object(initial.id.0, 0), Some(old));
    assert!(s.device.is_quiet());
    assert!(matches!(s.run(request), Ok(Outcome::Put(_))));
    assert_eq!(s.device.read_object(initial.id.0, 0), Some(s.source));
}

#[test]
fn invalid_source_crc_cancels_before_the_last_frame_can_commit() {
    let disk = formatted_card(TOTAL_BLOCKS, 8);
    let mut s = Session::new(&disk);
    let old = first_put(&mut s);
    let installed = s.source.clone();
    s.source = vec![24; 300];
    let Request::Put(mut request) = put(old.id, old.revision, &s.source) else {
        unreachable!();
    };
    request.payload_crc ^= 1;
    assert_eq!(s.run(Request::Put(request)), Err(Error::Protocol));
    assert_eq!(s.device.read_object(old.id.0, 0), Some(installed));
    assert!(s.device.is_quiet());
}

#[test]
fn a_mutating_catalog_restarts_from_the_first_page() {
    let disk = formatted_card(TOTAL_BLOCKS, 6);
    let mut s = Session::new(&disk);
    for _ in 0..4 {
        first_put(&mut s);
    }
    s.mutate_listing = true;
    let Outcome::Catalog { entries, .. } =
        s.run(Request::List(ListRequest { kind: Some(ObjectKind::Route), cursor: None })).unwrap()
    else {
        panic!("LIST result");
    };
    assert_eq!(entries.len(), 5);
    assert!(entries.windows(2).all(|p| p[0].id < p[1].id));
}

fn take_control(client: &mut Client) -> (u64, Vec<u8>) {
    loop {
        match client.next_action().expect("control action") {
            Action::Send { token, channel: Channel::Control, record } => return (token, record),
            Action::ResetSink | Action::Progress { .. } => {}
            other => panic!("unexpected {other:?}"),
        }
    }
}

fn introduced() -> Client {
    let mut client = Client::new(Ceilings::new(256, 128).unwrap(), Options::default());
    client.start(Request::List(ListRequest { kind: None, cursor: None }), 0).unwrap();
    let (token, request) = take_control(&mut client);
    client.event(Event::Written(token), 1);
    let id = decode_request(&request).unwrap().0.request;
    let mut response = [0; 256];
    let writer = ListWriter::start(&mut response, 256, StoreId([1; 16]), 1).unwrap();
    let len = writer.finish(&mut response, id, false).unwrap();
    client.event(Event::Control(&response[..len]), 2);
    assert!(matches!(client.next_action(), Some(Action::Complete(Ok(Outcome::Catalog { .. })))));
    client
}

#[test]
fn download_waits_for_independent_channel_delivery_and_checks_crc() {
    for response_first in [false, true] {
        for corrupt_crc in [false, true] {
            let mut client = introduced();
            client.start(Request::Get(GetRequest { id: ObjectId(5), revision: Revision(2) }), 3).unwrap();
            let (token, request) = take_control(&mut client);
            client.event(Event::Written(token), 4);
            let id = decode_request(&request).unwrap().0.request;
            let payload = [4, 5, 6];
            let mut reply = [0; 64];
            let crc = crc32(&payload) ^ u32::from(corrupt_crc);
            let len = encode_get(&mut reply, id, Revision(2), 3, crc).unwrap();
            let mut stream = [0; 19];
            write_stream(&mut stream, id, 0, 3).unwrap();
            stream[16..].copy_from_slice(&payload);
            if response_first {
                client.event(Event::Control(&reply[..len]), 5);
            }
            client.event(Event::Stream(&stream), 6);
            let Some(Action::WriteSink { token, offset: 0, bytes }) = client.next_action() else {
                panic!("sink write");
            };
            assert_eq!(bytes, payload);
            if !response_first {
                client.event(Event::Control(&reply[..len]), 7);
            }
            assert!(client.is_busy());
            client.event(Event::SinkWritten { token, offset: 0, len: 3 }, 8);
            if corrupt_crc {
                let (_, cancel) = take_control(&mut client);
                let cancel_id = decode_request(&cancel).unwrap().0.request;
                let len = encode_cancel(&mut reply, cancel_id, false).unwrap();
                client.event(Event::Control(&reply[..len]), 9);
                assert!(matches!(client.next_action(), Some(Action::ResetSink)));
                assert!(matches!(client.next_action(), Some(Action::ResetChannels)));
                assert_eq!(client.next_action(), Some(Action::Complete(Err(Error::Protocol))));
            } else {
                assert_eq!(client.next_action(), Some(Action::Progress { done: 3, total: 3 }));
                assert!(matches!(client.next_action(), Some(Action::Complete(Ok(Outcome::Get(_))))));
            }
        }
    }
}

#[test]
fn restored_identity_change_stops_a_pending_operation() {
    let mut client = introduced();
    client.start(Request::Remove(RemoveRequest { id: ObjectId(2), expected: Revision(1) }), 3).unwrap();
    take_control(&mut client);
    client.event(Event::LinkLost, 4);
    assert_eq!(client.next_action(), Some(Action::ResetChannels));
    assert_eq!(client.next_action(), Some(Action::Restore));
    client.event(Event::Restored(Ceilings::new(256, 128).unwrap()), 5);
    let (_, request) = take_control(&mut client);
    let id = decode_request(&request).unwrap().0.request;
    let mut bytes = [0; 256];
    let writer = ListWriter::start(&mut bytes, 256, StoreId([2; 16]), 1).unwrap();
    let len = writer.finish(&mut bytes, id, false).unwrap();
    client.event(Event::Control(&bytes[..len]), 6);
    assert_eq!(client.next_action(), Some(Action::ResetChannels));
    assert_eq!(
        client.next_action(),
        Some(Action::Complete(Err(Error::StoreChanged { previous: StoreId([1; 16]), current: StoreId([2; 16]) })))
    );
}

#[test]
fn timeouts_and_stale_request_ids_do_not_complete_another_operation() {
    let mut client = introduced();
    client.start(Request::Status(StatusRequest { id: ObjectId(5), revision: Revision(2) }), 3).unwrap();
    let (_, request) = take_control(&mut client);
    let id = decode_request(&request).unwrap().0.request;
    let mut reply = [0; 64];
    let len = encode_status(&mut reply, RequestId(id.0 - 1), &StatusResponse::absent()).unwrap();
    client.event(Event::Control(&reply[..len]), 4);
    assert!(client.next_action().is_none());
    client.event(Event::Tick, 15_004);
    assert_eq!(client.next_action(), Some(Action::ResetChannels));
    assert_eq!(client.next_action(), Some(Action::Complete(Err(Error::Timeout))));
    let before = reply;
    for short in 0..len {
        assert!(decode_response(&before[..short]).is_err());
    }
    reply[10] = 1;
    assert!(decode_response(&reply[..len]).is_err());
}

#[test]
fn cancel_that_loses_the_commit_race_waits_for_the_authoritative_result() {
    for answer_arrives in [false, true] {
        let mut client = introduced();
        let bytes = [9];
        client.start(put(ObjectId(2), Revision(1), &bytes), 3).unwrap();
        let (token, request) = take_control(&mut client);
        let put_id = decode_request(&request).unwrap().0.request;
        client.event(Event::Written(token), 4);
        let Some(Action::ReadSource { token, offset, .. }) = client.next_action() else {
            panic!("source read");
        };
        client.event(Event::Source { token, offset, bytes: &bytes }, 5);
        assert!(matches!(client.next_action(), Some(Action::Send { channel: Channel::Stream, .. })));
        client.event(Event::Cancel, 6);
        let (_, cancel) = take_control(&mut client);
        let cancel_id = decode_request(&cancel).unwrap().0.request;
        let mut response = [0; 64];
        let len = encode_cancel(&mut response, cancel_id, false).unwrap();
        client.event(Event::Control(&response[..len]), 7);
        assert!(client.is_busy());
        assert!(client.next_action().is_none());
        if answer_arrives {
            let len = encode_put(&mut response, put_id, ObjectId(2), Revision(2), 1, crc32(&bytes)).unwrap();
            client.event(Event::Control(&response[..len]), 8);
            assert!(matches!(client.next_action(), Some(Action::Complete(Ok(Outcome::Put(_))))));
        } else {
            client.event(Event::Tick, 2007);
            assert_eq!(client.next_action(), Some(Action::ResetChannels));
            assert_eq!(client.next_action(), Some(Action::Complete(Err(Error::OutcomeUnknown))));
        }
    }
}

fn retired_source() -> (Client, u64) {
    let mut client = introduced();
    client.start(put(ObjectId(2), Revision(1), &[9]), 3).unwrap();
    let (token, request) = take_control(&mut client);
    let put_id = decode_request(&request).unwrap().0.request;
    client.event(Event::Written(token), 4);
    let Some(Action::ReadSource { token: source_token, .. }) = client.next_action() else {
        panic!("source read");
    };
    client.event(Event::Cancel, 5);
    let (_, cancel) = take_control(&mut client);
    let cancel_id = decode_request(&cancel).unwrap().0.request;
    let mut response = [0; 64];
    let len = encode_error(
        &mut response,
        Opcode::Put,
        put_id,
        &Refusal::new(ErrorCode::Cancelled, detail::cancelled::BY_CLIENT),
    )
    .unwrap();
    client.event(Event::Control(&response[..len]), 6);
    let len = encode_cancel(&mut response, cancel_id, true).unwrap();
    client.event(Event::Control(&response[..len]), 7);
    assert_eq!(client.next_action(), Some(Action::ResetChannels));
    assert_eq!(client.next_action(), Some(Action::Complete(Err(Error::Cancelled))));
    (client, source_token)
}

#[test]
fn retired_source_and_failure_tokens_cannot_interrupt_another_operation_kind() {
    for request in [
        Request::Get(GetRequest { id: ObjectId(3), revision: Revision(1) }),
        Request::Status(StatusRequest { id: ObjectId(3), revision: Revision(1) }),
        Request::List(ListRequest { kind: None, cursor: None }),
    ] {
        let (mut client, stale_token) = retired_source();
        client.start(request, 8).unwrap();
        let (current_token, _) = take_control(&mut client);
        client.event(Event::Source { token: stale_token, offset: 0, bytes: &[9] }, 9);
        client.event(Event::IoFailed(stale_token), 10);
        assert!(client.is_busy());
        assert!(client.next_action().is_none());
        client.event(Event::IoFailed(current_token), 11);
        // A current failure is still actionable, unlike the retired callbacks.
        assert!(client.next_action().is_some());
    }
}

fn answer_list(client: &mut Client, request: &[u8], store: StoreId, now: u64) {
    let (header, decoded) = decode_request(request).unwrap();
    assert!(matches!(decoded, Request::List(_)));
    let mut response = [0; 256];
    let writer = ListWriter::start(&mut response, 256, store, 1).unwrap();
    let len = writer.finish(&mut response, header.request, false).unwrap();
    client.event(Event::Control(&response[..len]), now);
}

#[test]
fn idle_reconnect_reintroduces_identity_before_a_scoped_read_or_mutation() {
    for request in [
        Request::Get(GetRequest { id: ObjectId(3), revision: Revision(1) }),
        Request::Remove(RemoveRequest { id: ObjectId(3), expected: Revision(1) }),
    ] {
        let mut client = introduced();
        client.event(Event::LinkLost, 3);
        assert_eq!(client.store_id(), None);
        assert_eq!(client.start_scoped(request, Some(StoreId([1; 16])), 4), Err(Error::LinkLost));
        client.event(Event::Restored(Ceilings::new(256, 128).unwrap()), 5);
        client.start_scoped(request, Some(StoreId([1; 16])), 6).unwrap();
        let (_, identity) = take_control(&mut client);
        answer_list(&mut client, &identity, StoreId([2; 16]), 7);
        if matches!(request, Request::Get(_)) {
            assert_eq!(client.next_action(), Some(Action::ResetSink));
        }
        assert_eq!(client.next_action(), Some(Action::ResetChannels));
        assert_eq!(
            client.next_action(),
            Some(Action::Complete(Err(Error::StoreChanged { previous: StoreId([1; 16]), current: StoreId([2; 16]) })))
        );
        assert!(!client.is_busy());
        assert!(client.next_action().is_none());
    }
}

#[test]
fn first_scoped_listing_checks_identity_without_a_cached_store() {
    let mut client = Client::new(Ceilings::new(256, 128).unwrap(), Options::default());
    client.start_scoped(Request::List(ListRequest { kind: None, cursor: None }), Some(StoreId([1; 16])), 0).unwrap();
    let (_, request) = take_control(&mut client);
    answer_list(&mut client, &request, StoreId([2; 16]), 1);
    assert_eq!(client.next_action(), Some(Action::ResetChannels));
    assert_eq!(
        client.next_action(),
        Some(Action::Complete(Err(Error::StoreChanged { previous: StoreId([1; 16]), current: StoreId([2; 16]) })))
    );
    assert_eq!(client.store_id(), None);
}

#[test]
fn first_introduction_pins_identity_for_loss_reconciliation() {
    let mut client = Client::new(Ceilings::new(256, 128).unwrap(), Options::default());
    client.start(put(ObjectId(2), Revision(1), &[9]), 0).unwrap();
    let (_, first) = take_control(&mut client);
    answer_list(&mut client, &first, StoreId([1; 16]), 1);
    let (_, put) = take_control(&mut client);
    assert!(matches!(decode_request(&put).unwrap().1, Request::Put(_)));
    client.event(Event::LinkLost, 2);
    assert_eq!(client.next_action(), Some(Action::ResetChannels));
    assert_eq!(client.next_action(), Some(Action::Restore));
    client.event(Event::Restored(Ceilings::new(256, 128).unwrap()), 3);
    let (_, restored) = take_control(&mut client);
    answer_list(&mut client, &restored, StoreId([2; 16]), 4);
    assert_eq!(client.next_action(), Some(Action::ResetChannels));
    assert_eq!(
        client.next_action(),
        Some(Action::Complete(Err(Error::StoreChanged { previous: StoreId([1; 16]), current: StoreId([2; 16]) })))
    );
}

#[test]
fn cached_store_mismatch_refuses_scoped_reads_and_remove_before_send() {
    let disk = formatted_card(TOTAL_BLOCKS, 23);
    let mut s = Session::new(&disk);
    let installed = first_put(&mut s);
    let bytes = s.source.clone();
    let current = StoreId(STORE.0);
    let previous = StoreId([0xab; 16]);
    assert_eq!(s.client.store_id(), Some(current));
    let get = Request::Get(GetRequest { id: installed.id, revision: installed.revision });
    for request in [
        get,
        Request::Status(StatusRequest { id: installed.id, revision: installed.revision }),
        Request::Remove(RemoveRequest { id: installed.id, expected: installed.revision }),
        Request::List(ListRequest { kind: None, cursor: None }),
    ] {
        assert_eq!(
            s.client.start_scoped(request, Some(previous), s.now),
            Err(Error::StoreChanged { previous, current })
        );
        assert!(!s.client.is_busy());
        assert_eq!(s.client.next_action(), None);
        assert_eq!(s.client.store_id(), Some(current));
    }
    assert_eq!(s.device.read_object(installed.id.0, 0), Some(bytes.clone()));
    assert!(matches!(s.run(get), Ok(Outcome::Get(_))));
    assert_eq!(s.sink, bytes);
}
