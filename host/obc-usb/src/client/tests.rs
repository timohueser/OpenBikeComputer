use std::{
    collections::VecDeque,
    fs::OpenOptions,
    io::Read,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use super::*;
use obc_flat_device::{crc32, Card, Device, Reaction, STORE, TOTAL_BLOCKS};
use obc_link::flat::{wire::*, DisplayName, Link, ObjectId, ObjectKind, Revision};

struct Loopback {
    device: Device<Card>,
    records: VecDeque<Result<(Channel, Vec<u8>), crate::PipeFault>>,
    lose_put_result: bool,
    fail_download: bool,
    download_records: usize,
    fail_restore: bool,
    stream_writes: usize,
    resets: usize,
    restores: usize,
}

impl Loopback {
    fn new(seed: u64) -> Self {
        let device = Device::boot_on(Card::formatted(TOTAL_BLOCKS, seed, STORE), Link::Usb, ceilings());
        Self {
            device,
            records: VecDeque::new(),
            lose_put_result: false,
            fail_download: false,
            download_records: 0,
            fail_restore: false,
            stream_writes: 0,
            resets: 0,
            restores: 0,
        }
    }

    fn enqueue(&mut self, reaction: Reaction) {
        let (channel, record) = match reaction {
            Reaction::Send { channel, bytes } => (channel, bytes),
            Reaction::SendAndReboot { bytes } => (Channel::Control, bytes),
            Reaction::Idle => return,
            Reaction::Close(_) => panic!("native client emits a valid record"),
        };
        if channel == Channel::Stream {
            self.download_records += 1;
            if self.fail_download && self.download_records == 2 {
                let error = if self.fail_restore {
                    crate::PipeFault::closed("disconnected download")
                } else {
                    crate::PipeFault::device("invalid binding padding")
                };
                self.records.push_back(Err(error));
                return;
            }
        }
        if self.lose_put_result
            && channel == Channel::Control
            && matches!(decode_response(&record).unwrap().1, Response::Put(_))
        {
            self.lose_put_result = false;
            self.records.push_back(Err(crate::PipeFault::closed("lost commit answer")));
            self.records.push_back(Err(crate::PipeFault::closed("other endpoint lost")));
            return;
        }
        // The production reassembler sees split prefixes, payloads and padding on incoming bytes.
        let mut wire = (record.len() as u32).to_le_bytes().to_vec();
        wire.extend_from_slice(&record);
        wire.resize(4 + obc_link::flat::padded_record_len(record.len()), 0);
        let mut decoder = usb::Records::new(17);
        for chunk in wire.chunks(17) {
            decoder.append(chunk).unwrap();
            while let Some(record) = decoder.take().unwrap() {
                self.records.push_back(Ok((channel, record)));
            }
        }
    }
}

impl Transport for Loopback {
    fn lost(&mut self) -> bool {
        false
    }

    fn ready(&mut self) -> Option<Result<(Channel, Vec<u8>), crate::PipeFault>> {
        self.records.pop_front()
    }

    async fn send(&mut self, channel: Channel, record: &[u8]) -> Result<(), crate::PipeFault> {
        let wire = usb::frame(channel, record)?;
        if channel == Channel::Stream {
            self.stream_writes += 1;
        }
        let mut decoder = usb::Records::new(3);
        for chunk in wire.chunks(3) {
            decoder.append(chunk)?;
            while let Some(record) = decoder.take()? {
                let reaction = match channel {
                    Channel::Control => self.device.on_control(&record),
                    Channel::Stream => self.device.on_stream(&record),
                };
                self.enqueue(reaction);
            }
        }
        Ok(())
    }

    async fn receive(&mut self) -> Result<(Channel, Vec<u8>), crate::PipeFault> {
        loop {
            if let Some(record) = self.records.pop_front() {
                return record;
            }
            let reaction = self.device.poll();
            self.enqueue(reaction);
            tokio::task::yield_now().await;
        }
    }

    async fn reset(&mut self) -> Result<(), crate::PipeFault> {
        self.resets += 1;
        self.records.clear();
        self.device.link_down(Link::Usb);
        self.device.link_up(Link::Usb, ceilings());
        Ok(())
    }

    async fn restore(&mut self) -> Result<(), crate::PipeFault> {
        self.restores += 1;
        self.records.clear();
        if self.fail_restore {
            return Err(crate::PipeFault::closed("device did not reconnect"));
        }
        self.device.link_down(Link::Usb);
        self.device.link_up(Link::Usb, ceilings());
        Ok(())
    }
}

struct PayloadFile {
    path: PathBuf,
    file: File,
}
impl PayloadFile {
    fn new(bytes: &[u8]) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "obc-native-client-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = OpenOptions::new().create_new(true).read(true).write(true).open(&path).unwrap();
        file.write_all(bytes).unwrap();
        Self { path, file }
    }
    fn bytes(&mut self) -> Vec<u8> {
        self.file.rewind().unwrap();
        let mut bytes = Vec::new();
        self.file.read_to_end(&mut bytes).unwrap();
        bytes
    }
}
impl Drop for PayloadFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn put(bytes: &[u8]) -> Request {
    Request::Put(PutRequest {
        id: ObjectId::NONE,
        expected: Revision::HEAD,
        payload_len: bytes.len() as u64,
        payload_crc: crc32(bytes),
        kind: ObjectKind::MapShard,
        name: DisplayName::new("native map").unwrap(),
    })
}

#[tokio::test]
async fn native_files_round_trip_real_card_with_settled_progress_and_remove() {
    let mut session = Session::new(Loopback::new(901), Options::default());
    let (_, cancel) = watch::channel(false);
    let bytes: Vec<_> = (0..28_037).map(|n| (n * 31) as u8).collect();
    let mut source = PayloadFile::new(&bytes);
    let mut progress = Vec::new();
    let outcome = session
        .execute(put(&bytes), Some(StoreId(STORE.0)), Some(&mut source.file), None, &cancel, |done, total| {
            progress.push((done, total))
        })
        .await
        .unwrap();
    let Outcome::Put(upload) = outcome else {
        panic!("PUT result");
    };
    assert_eq!(progress.last(), Some(&(bytes.len() as u64, bytes.len() as u64)));
    assert!(progress.windows(2).all(|p| p[0].0 < p[1].0 && p[0].1 == p[1].1));
    let mut sink = PayloadFile::new(&vec![0xee; bytes.len() + 99]);
    let get = Request::Get(GetRequest { id: upload.id, revision: upload.revision });
    session.execute(get, Some(StoreId(STORE.0)), None, Some(&mut sink.file), &cancel, |_, _| {}).await.unwrap();
    assert_eq!(sink.bytes(), bytes);
    let catalog = session
        .execute(Request::List(ListRequest { kind: None, cursor: None }), None, None, None, &cancel, |_, _| {})
        .await
        .unwrap();
    assert!(matches!(catalog, Outcome::Catalog { entries, .. } if entries.len() == 1 && entries[0].id == upload.id));
    session
        .execute(
            Request::Remove(RemoveRequest { id: upload.id, expected: upload.revision }),
            Some(StoreId(STORE.0)),
            None,
            None,
            &cancel,
            |_, _| {},
        )
        .await
        .unwrap();
    let catalog = session
        .execute(Request::List(ListRequest { kind: None, cursor: None }), None, None, None, &cancel, |_, _| {})
        .await
        .unwrap();
    assert!(matches!(catalog, Outcome::Catalog { entries, .. } if entries.is_empty()));
    let replacement = StoreId([0x35; 16]);
    let formatted = session
        .execute(
            Request::Format(FormatRequest { expected: StoreId(STORE.0), replacement }),
            None,
            None,
            None,
            &cancel,
            |_, _| {},
        )
        .await
        .unwrap();
    assert_eq!(formatted, Outcome::Format(replacement));
}

#[tokio::test]
async fn native_cancel_and_bad_source_crc_preserve_committed_map() {
    let mut session = Session::new(Loopback::new(902), Options::default());
    let original = vec![41; 12_000];
    let mut initial = PayloadFile::new(&original);
    let (_, initial_cancel) = watch::channel(false);
    let uploaded =
        session.execute(put(&original), None, Some(&mut initial.file), None, &initial_cancel, |_, _| {}).await.unwrap();
    let Outcome::Put(installed) = uploaded else {
        panic!("initial PUT result");
    };
    let bytes = vec![7; 31_000];
    let Request::Put(mut replacement) = put(&bytes) else { unreachable!() };
    replacement.id = installed.id;
    replacement.expected = installed.revision;
    let mut source = PayloadFile::new(&bytes);
    let (stop, cancel) = watch::channel(false);
    let outcome = session
        .execute(Request::Put(replacement), None, Some(&mut source.file), None, &cancel, |done, _| {
            if done > 0 {
                stop.send_replace(true);
            }
        })
        .await;
    assert!(matches!(outcome, Err(Error::Client(ClientError::Cancelled))));
    stop.send_replace(false);
    let mut wrong = Request::Put(replacement);
    let Request::Put(ref mut request) = wrong else { unreachable!() };
    request.payload_crc ^= 1;
    assert!(session.execute(wrong, None, Some(&mut source.file), None, &cancel, |_, _| {}).await.is_err());
    let catalog = session
        .execute(Request::List(ListRequest { kind: None, cursor: None }), None, None, None, &cancel, |_, _| {})
        .await
        .unwrap();
    assert!(
        matches!(catalog, Outcome::Catalog { entries, .. } if entries.len() == 1 && entries[0].revision == installed.revision)
    );
    let mut sink = PayloadFile::new(&[]);
    session
        .execute(
            Request::Get(GetRequest { id: installed.id, revision: installed.revision }),
            None,
            None,
            Some(&mut sink.file),
            &cancel,
            |_, _| {},
        )
        .await
        .unwrap();
    assert_eq!(sink.bytes(), original);
    let writes = session.transport.stream_writes;
    replacement.expected = installed.revision.next().unwrap();
    let refused =
        session.execute(Request::Put(replacement), None, Some(&mut source.file), None, &cancel, |_, _| {}).await;
    assert!(matches!(
        refused,
        Err(Error::Client(ClientError::Remote(Refusal { code: ErrorCode::RevisionConflict, .. })))
    ));
    assert_eq!(session.transport.stream_writes, writes);
    assert_eq!(session.transport.resets, 3);
}

#[tokio::test]
async fn native_restore_reconciles_a_lost_put_answer_without_recreating() {
    let mut session = Session::new(Loopback::new(903), Options::default());
    session.transport.lose_put_result = true;
    let bytes = vec![9; 19_000];
    let mut source = PayloadFile::new(&bytes);
    let (_, cancel) = watch::channel(false);
    let result = session
        .execute(put(&bytes), Some(StoreId(STORE.0)), Some(&mut source.file), None, &cancel, |_, _| {})
        .await
        .unwrap();
    assert!(matches!(result, Outcome::Put(_)));
    assert_eq!(session.transport.restores, 1);
    let catalog = session
        .execute(Request::List(ListRequest { kind: None, cursor: None }), None, None, None, &cancel, |_, _| {})
        .await
        .unwrap();
    assert!(matches!(catalog, Outcome::Catalog { entries, .. } if entries.len() == 1));
}

#[test]
fn v5_encoder_zero_pads_to_the_next_word() {
    let record = usb::frame(Channel::Control, &[1, 2, 3, 4, 5]).unwrap();
    assert_eq!(&record[..4], &[5, 0, 0, 0]);
    assert_eq!(&record[4..9], &[1, 2, 3, 4, 5]);
    assert_eq!(&record[9..], &[0, 0, 0]);
    assert!(usb::frame(Channel::Control, &[0; 257]).is_err());
    assert!(usb::frame(Channel::Stream, &[0; 8209]).is_err());
}

#[test]
fn v5_decoder_waits_for_every_split_prefix_frame_and_padding_byte() {
    let record = usb::frame(Channel::Stream, &[1, 2, 3, 4, 5]).unwrap();
    for split in 0..record.len() {
        let mut decoder = usb::Records::new(record.len());
        decoder.append(&record[..split]).unwrap();
        assert!(decoder.take().unwrap().is_none(), "split {split}");
        decoder.append(&record[split..]).unwrap();
        assert_eq!(decoder.take().unwrap(), Some(vec![1, 2, 3, 4, 5]));
    }
}

#[test]
fn v5_decoder_reads_a_u32_length_rejects_padding_and_accepts_multiple_records() {
    let mut decoder = usb::Records::new(20);
    decoder.append(&65_552u32.to_le_bytes()).unwrap();
    assert!(decoder.take().is_err());
    let mut record = usb::frame(Channel::Stream, &[0xaa]).unwrap();
    *record.last_mut().unwrap() = 1;
    let mut decoder = usb::Records::new(20);
    decoder.append(&record).unwrap();
    assert!(decoder.take().is_err());
    let mut records = usb::frame(Channel::Control, &[1]).unwrap();
    records.extend_from_slice(&usb::frame(Channel::Control, &[2]).unwrap());
    let mut decoder = usb::Records::new(20);
    decoder.append(&records).unwrap();
    // A continuous session permits several records in one USB read.
    assert_eq!(decoder.take().unwrap(), Some(vec![1]));
    assert_eq!(decoder.take().unwrap(), Some(vec![2]));
    assert!(decoder.take().unwrap().is_none());
}

#[tokio::test]
async fn native_transport_or_restore_failure_discards_partial_download() {
    for disconnected in [false, true] {
        let mut session = Session::new(Loopback::new(904), Options::default());
        let (_, cancel) = watch::channel(false);
        let bytes = vec![23; 28_000];
        let mut source = PayloadFile::new(&bytes);
        let result =
            session.execute(put(&bytes), None, Some(&mut source.file), None, &cancel, |_, _| {}).await.unwrap();
        let Outcome::Put(upload) = result else {
            panic!("PUT result");
        };
        session.transport.fail_download = true;
        session.transport.fail_restore = disconnected;
        let mut sink = PayloadFile::new(&[99; 64]);
        let mut progress = Vec::new();
        let result = session
            .execute(
                Request::Get(GetRequest { id: upload.id, revision: upload.revision }),
                None,
                None,
                Some(&mut sink.file),
                &cancel,
                |done, _| progress.push(done),
            )
            .await;
        assert!(matches!(result, Err(Error::Transport(_))));
        assert_eq!(progress, vec![8192]);
        assert!(sink.bytes().is_empty());
        assert_eq!(session.client.store_id(), None);
    }
}
