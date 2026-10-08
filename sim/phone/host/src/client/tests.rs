use super::*;
use obc_flat_device::{Card, Device, Reaction, STORE, TOTAL_BLOCKS};
use obc_link::flat::Link;

struct Session {
    client: *mut ObcClient,
    device: Device<Card>,
    source: Vec<u8>,
    sink: Vec<u8>,
}

impl Drop for Session {
    fn drop(&mut self) {
        unsafe { obc_client_close(self.client) };
    }
}

impl Session {
    fn new() -> Self {
        let ceilings = Ceilings::new(256, 64).unwrap();
        Self {
            client: obc_client_open(256, 64),
            device: Device::boot_on(Card::formatted(TOTAL_BLOCKS, 71, STORE), Link::Ble, ceilings),
            source: Vec::new(),
            sink: Vec::new(),
        }
    }

    fn deliver(&mut self, reaction: Reaction) {
        match reaction {
            Reaction::Idle => {}
            Reaction::Send { channel, bytes } => unsafe {
                assert_eq!(
                    obc_client_event(
                        self.client,
                        if channel == Channel::Control { 1 } else { 2 },
                        0,
                        0,
                        bytes.as_ptr(),
                        bytes.len(),
                        1
                    )
                    .error,
                    0
                );
            },
            _ => panic!("ordinary client operation does not reboot or close"),
        }
    }

    fn run(&mut self, request: &ObcClientRequest) -> ObcClientResult {
        unsafe { assert_eq!(obc_client_start(self.client, request, false, 0, false, 1).error, 0) };
        for _ in 0..2_000 {
            let action = unsafe { obc_client_next(self.client) };
            match action.kind {
                0 => {
                    let reaction = self.device.poll();
                    self.deliver(reaction);
                }
                1 => {
                    // Copy handle-owned bytes before the next C call can replace them.
                    let bytes = unsafe { slice::from_raw_parts(action.bytes, action.length) }.to_vec();
                    let reaction = if action.channel == 0 {
                        self.device.on_control(&bytes)
                    } else {
                        self.device.on_stream(&bytes)
                    };
                    if action.channel == 0 {
                        assert_eq!(unsafe { obc_client_reads(self.client) } & 1, 0);
                    }
                    unsafe { obc_client_event(self.client, 4, action.token, 0, ptr::null(), 0, 1) };
                    if action.channel == 0 {
                        assert_eq!(unsafe { obc_client_reads(self.client) } & 1, 1);
                    }
                    self.deliver(reaction);
                }
                2 => {
                    let start = action.offset as usize;
                    let end = self.source.len().min(start + action.length);
                    let bytes = &self.source[start..end];
                    unsafe {
                        obc_client_event(self.client, 3, action.token, action.offset, bytes.as_ptr(), bytes.len(), 1)
                    };
                }
                3 => {
                    assert_eq!(action.offset, self.sink.len() as u64);
                    self.sink.extend_from_slice(unsafe { slice::from_raw_parts(action.bytes, action.length) });
                    // SinkWritten length describes settled output, not readable input bytes.
                    unsafe {
                        assert_eq!(
                            obc_client_event(
                                self.client,
                                5,
                                action.token,
                                action.offset,
                                ptr::null(),
                                action.length,
                                1
                            )
                            .error,
                            0
                        )
                    };
                }
                4 => self.sink.clear(),
                5 => assert!(action.offset <= action.total),
                8 => return action.result,
                _ => panic!("unexpected adapter action {}", action.kind),
            }
        }
        panic!("C adapter did not settle");
    }
}

fn request(opcode: u32) -> ObcClientRequest {
    ObcClientRequest {
        opcode,
        kind: 0,
        object_id: 0,
        revision: 0,
        length: 0,
        crc: 0,
        scoped: 0,
        store: [0; 16],
        replacement: [0; 16],
        name: ptr::null(),
        name_len: 0,
    }
}

#[test]
fn c_adapter_preserves_upload_download_catalogue_and_checksum() {
    let mut session = Session::new();
    session.source = (0..193).map(|i| (i * 17) as u8).collect();
    let crc = unsafe { obc_client_crc32(session.source.as_ptr(), session.source.len()) };
    let name = "Ridge route".as_bytes();
    let mut put = request(4);
    put.kind = ObjectKind::Route as u32;
    put.name = name.as_ptr();
    put.name_len = name.len();
    put.length = session.source.len() as u64;
    put.crc = crc;
    let uploaded = session.run(&put);
    assert_eq!(uploaded.error, 0);
    assert_eq!(uploaded.opcode, 4);
    assert_eq!(uploaded.length, put.length);
    assert_eq!(uploaded.crc, crc);
    assert_eq!(session.device.read_object(uploaded.object_id, uploaded.revision).unwrap(), session.source);

    let listed = session.run(&request(1));
    assert_eq!(listed.error, 0);
    assert_eq!(listed.entry_count, 1);
    let entry = unsafe { &*listed.entries };
    assert_eq!(entry.object_id, uploaded.object_id);
    assert_eq!(&entry.name[..entry.name_len], name);

    let mut get = request(3);
    get.object_id = uploaded.object_id;
    get.revision = uploaded.revision;
    get.scoped = 1;
    get.store = listed.store;
    let downloaded = session.run(&get);
    assert_eq!(downloaded.error, 0);
    assert_eq!(downloaded.opcode, 3);
    assert_eq!(downloaded.revision, uploaded.revision);
    assert_eq!(session.sink, session.source);
    assert_eq!(downloaded.crc, crc);
}
