//! Exclusive native protocol session. Files are read by offset and downloads are written in chunks.
//! Do not share its claimed interface with a raw-record reader.

use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    time::Instant,
};

use obc_link::flat::{
    client::{Action, Client, Error as ClientError, Event, Options, Outcome},
    wire::Request,
    Ceilings, Channel, StoreId,
};
use tokio::sync::watch;

mod usb;
use usb::UsbTransport;

#[derive(Debug)]
pub enum Error {
    Client(ClientError),
    File(std::io::Error),
    Transport(crate::PipeFault),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Client(error) => write!(f, "store operation failed: {error:?}"),
            Self::File(error) => write!(f, "payload file failed: {error}"),
            Self::Transport(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for Error {}

/// Owns both record readers. Dropping the client signals and drains pending USB reads.
pub struct NativeClient(Session<UsbTransport>);

impl NativeClient {
    pub async fn open(info: &nusb::DeviceInfo, options: Options) -> Result<Self, Error> {
        let transport = UsbTransport::open(info).await.map_err(Error::Transport)?;
        Ok(Self(Session::new(transport, options)))
    }

    pub fn store_id(&self) -> Option<StoreId> {
        self.0.client.store_id()
    }

    /// The PUT descriptor supplies length and CRC. The core checks the actual file chunks before
    /// sending the final frame. GET truncates the sink before starting and on an unsuccessful result.
    /// The cancellation receiver belongs to this operation; `true` requests protocol cancellation.
    /// Keep this future alive until its result. After abandoning it, close the client before reuse.
    /// Sync a received ride file before asserting durable possession in ARCHIVE_RIDE.
    pub async fn execute(
        &mut self,
        request: Request,
        expected_store: Option<StoreId>,
        source: Option<&mut File>,
        sink: Option<&mut File>,
        cancel: &watch::Receiver<bool>,
        progress: impl FnMut(u64, u64),
    ) -> Result<Outcome, Error> {
        self.0.execute(request, expected_store, source, sink, cancel, progress).await
    }

    /// Stop and join readers before releasing the interface claim.
    pub async fn close(mut self) {
        self.0.transport.close().await;
    }
}

trait Transport {
    fn lost(&mut self) -> bool;
    fn ready(&mut self) -> Option<Result<(Channel, Vec<u8>), crate::PipeFault>>;
    async fn send(&mut self, channel: Channel, record: &[u8]) -> Result<(), crate::PipeFault>;
    async fn receive(&mut self) -> Result<(Channel, Vec<u8>), crate::PipeFault>;
    async fn reset(&mut self) -> Result<(), crate::PipeFault>;
    async fn restore(&mut self) -> Result<(), crate::PipeFault>;
}

struct Session<T> {
    transport: T,
    client: Client,
    options: Options,
    clock: Instant,
    lost: bool,
}

fn ceilings() -> Ceilings {
    Ceilings::for_usb(usb::MAX_RECORD).unwrap()
}

impl<T: Transport> Session<T> {
    fn new(transport: T, options: Options) -> Self {
        Self { transport, client: Client::new(ceilings(), options), options, clock: Instant::now(), lost: false }
    }

    fn now(&self) -> u64 {
        self.clock.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
    }

    fn received(&mut self, record: Result<(Channel, Vec<u8>), crate::PipeFault>) -> Result<(), Error> {
        match record {
            Ok((Channel::Control, bytes)) => self.client.event(Event::Control(&bytes), self.now()),
            Ok((Channel::Stream, bytes)) => self.client.event(Event::Stream(&bytes), self.now()),
            Err(error) if error.code == "closed" => {
                if !self.lost {
                    self.lost = true;
                    self.client.event(Event::LinkLost, self.now());
                }
            }
            Err(error) => return Err(Error::Transport(error)),
        }
        Ok(())
    }

    async fn abort(&mut self, error: Error, sink: Option<&mut File>) -> Error {
        self.client = Client::new(ceilings(), self.options);
        let _ = self.transport.reset().await;
        if let Some(file) = sink {
            if let Err(error) = file.set_len(0) {
                return Error::File(error);
            }
        }
        error
    }

    async fn execute(
        &mut self,
        request: Request,
        expected_store: Option<StoreId>,
        mut source: Option<&mut File>,
        mut sink: Option<&mut File>,
        cancel: &watch::Receiver<bool>,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<Outcome, Error> {
        if matches!(request, Request::Put(_)) && source.is_none()
            || matches!(request, Request::Get(_)) && sink.is_none()
        {
            return Err(Error::Client(ClientError::InvalidInput));
        }
        let expected_store = expected_store.or(self.client.store_id());
        if self.lost || self.transport.lost() {
            self.client.event(Event::LinkLost, self.now());
            self.transport.restore().await.map_err(Error::Transport)?;
            self.client.event(Event::Restored(ceilings()), self.now());
            self.lost = false;
        }
        self.client.start_scoped(request, expected_store, self.now()).map_err(Error::Client)?;
        let mut cancelled = false;
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(50));
        loop {
            if *cancel.borrow() && !cancelled {
                cancelled = true;
                self.client.event(Event::Cancel, self.now());
            }
            loop {
                if let Some(record) = self.transport.ready() {
                    if let Err(error) = self.received(record) {
                        return Err(self.abort(error, sink.as_deref_mut()).await);
                    }
                }
                if *cancel.borrow() && !cancelled {
                    cancelled = true;
                    self.client.event(Event::Cancel, self.now());
                }
                let Some(action) = self.client.next_action() else {
                    break;
                };
                match action {
                    Action::Send { token, channel, record } => match self.transport.send(channel, &record).await {
                        Ok(()) => self.client.event(Event::Written(token), self.now()),
                        Err(error) if error.code == "closed" => {
                            self.received(Err(error))?;
                        }
                        Err(_) => self.client.event(Event::IoFailed(token), self.now()),
                    },
                    Action::ReadSource { token, offset, max_len } => {
                        let mut bytes = vec![0; max_len];
                        let result = source
                            .as_deref_mut()
                            .unwrap()
                            .seek(SeekFrom::Start(offset))
                            .and_then(|_| source.as_deref_mut().unwrap().read(&mut bytes));
                        match result {
                            Ok(len) => {
                                self.client.event(Event::Source { token, offset, bytes: &bytes[..len] }, self.now())
                            }
                            Err(_) => self.client.event(Event::IoFailed(token), self.now()),
                        }
                    }
                    Action::WriteSink { token, offset, bytes } => {
                        let file = sink.as_deref_mut().unwrap();
                        let result = file.seek(SeekFrom::Start(offset)).and_then(|_| file.write_all(&bytes));
                        let event = if result.is_ok() {
                            Event::SinkWritten { token, offset, len: bytes.len() }
                        } else {
                            Event::IoFailed(token)
                        };
                        self.client.event(event, self.now());
                    }
                    Action::ResetSink => {
                        if let Some(file) = sink.as_deref_mut() {
                            if let Err(error) = file.set_len(0) {
                                return Err(self.abort(Error::File(error), None).await);
                            }
                        }
                    }
                    Action::Progress { done, total } => progress(done, total),
                    Action::ResetChannels => {
                        if let Err(error) = self.transport.reset().await {
                            return Err(self.abort(Error::Transport(error), sink.as_deref_mut()).await);
                        }
                    }
                    Action::Restore => match self.transport.restore().await {
                        Ok(()) => {
                            self.lost = false;
                            self.client.event(Event::Restored(ceilings()), self.now());
                        }
                        Err(error) => {
                            return Err(self.abort(Error::Transport(error), sink.as_deref_mut()).await);
                        }
                    },
                    Action::Complete(result) => return result.map_err(Error::Client),
                }
            }
            tokio::select! {
                received = self.transport.receive() => {
                    if let Err(error) = self.received(received) {
                        return Err(self.abort(error, sink.as_deref_mut()).await);
                    }
                }
                _ = tick.tick() => self.client.event(Event::Tick, self.now()),
            }
        }
    }
}

#[cfg(test)]
mod tests;
