use std::{sync::Arc, time::Duration};

use obc_link::flat::{padded_record_len, record_buffer_len, Channel, Reassembler, RECORD_PREFIX_LEN};
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
};

use super::Transport;
use crate::{Dir, OpenLink, PipeFault, Plane, PRODUCT_ID, VENDOR_ID};

pub(super) const MAX_RECORD: usize = 8_208;
const MAX_CONTROL_OUT: usize = 256;

type Received = Result<(Channel, Vec<u8>), PipeFault>;

pub(super) fn frame(channel: Channel, record: &[u8]) -> Result<Vec<u8>, PipeFault> {
    let ceiling = if channel == Channel::Control { MAX_CONTROL_OUT } else { MAX_RECORD };
    if record.is_empty() || record.len() > ceiling {
        return Err(PipeFault::device("USB record exceeds its channel ceiling or is empty."));
    }
    let mut bytes = Vec::with_capacity(RECORD_PREFIX_LEN + padded_record_len(record.len()));
    bytes.extend_from_slice(&(record.len() as u32).to_le_bytes());
    bytes.extend_from_slice(record);
    bytes.resize(RECORD_PREFIX_LEN + padded_record_len(record.len()), 0);
    Ok(bytes)
}

pub(super) struct Records {
    bytes: Vec<u8>,
    decoder: Reassembler,
    armed: usize,
}

impl Records {
    pub(super) fn new(armed: usize) -> Self {
        Self { bytes: vec![0; record_buffer_len(MAX_RECORD, armed)], decoder: Reassembler::new(MAX_RECORD), armed }
    }

    pub(super) fn append(&mut self, chunk: &[u8]) -> Result<(), PipeFault> {
        if chunk.len() > self.armed {
            return Err(PipeFault::device("USB read exceeds the armed buffer."));
        }
        let at = self.decoder.read_offset(&mut self.bytes, self.armed);
        self.bytes[at..at + chunk.len()].copy_from_slice(chunk);
        self.decoder.filled(chunk.len());
        Ok(())
    }

    pub(super) fn take(&mut self) -> Result<Option<Vec<u8>>, PipeFault> {
        self.decoder
            .take(&self.bytes)
            .map(|span| span.map(|(at, len)| self.bytes[at..at + len].to_vec()))
            .map_err(|error| PipeFault::device(format!("Invalid USB record: {error:?}")))
    }
}

async fn read_or_stop<F: std::future::Future>(
    read: F,
    stop: &mut watch::Receiver<bool>,
    cancel: impl FnOnce(),
) -> Option<F::Output> {
    tokio::pin!(read);
    tokio::select! {
        // The sole IN reader has an uncontended lock. Its first poll subscribes before cancel.
        biased;
        result = &mut read => Some(result),
        _ = stop.changed() => {
            cancel();
            let _ = read.await;
            None
        }
    }
}

struct Readers {
    stop: watch::Sender<bool>,
    tasks: Vec<JoinHandle<()>>,
    received: mpsc::Receiver<Received>,
}

impl Readers {
    fn start(link: &Arc<OpenLink>) -> Self {
        let (stop, _) = watch::channel(false);
        let (send, received) = mpsc::channel(4);
        let tasks = [(Plane::Control, Channel::Control), (Plane::Bulk, Channel::Stream)]
            .into_iter()
            .map(|(plane, channel)| {
                let link = Arc::clone(link);
                let mut stop = stop.subscribe();
                let send = send.clone();
                tokio::spawn(async move {
                    let pipe = link.plane(plane);
                    let armed = pipe.read_len;
                    let mut records = Records::new(armed);
                    loop {
                        if *stop.borrow() {
                            return;
                        }
                        let Some(received) = read_or_stop(pipe.read(), &mut stop, || pipe.cancel(Some(Dir::In))).await else {
                            return;
                        };
                        let result = match received {
                            Ok(chunk) => {
                                if let Err(error) = records.append(&chunk) {
                                let _ = send.send(Err(error)).await;
                                return;
                            }
                            loop {
                                match records.take() {
                                    Ok(Some(record)) => {
                                        tokio::select! {
                                            result = send.send(Ok((channel, record))) => if result.is_err() { return; },
                                            _ = stop.changed() => return,
                                        }
                                    }
                                    Ok(None) => break,
                                    Err(error) => {
                                        let _ = send.send(Err(error)).await;
                                        return;
                                    }
                                }
                            }
                            continue;
                            }
                            Err(error) => Err(error),
                        };
                        let _ = send.send(result).await;
                        return;
                    }
                })
            })
            .collect();
        Self { stop, tasks, received }
    }

    async fn join(mut self) {
        self.stop.send_replace(true);
        // A blocked producer must be released before joining it.
        self.received.close();
        for task in self.tasks.drain(..) {
            let _ = task.await;
        }
    }
}

impl Drop for Readers {
    fn drop(&mut self) {
        self.stop.send_replace(true);
    }
}

pub(super) struct UsbTransport {
    link: Option<Arc<OpenLink>>,
    readers: Option<Readers>,
    serial: Option<String>,
}

impl UsbTransport {
    pub(super) async fn open(info: &nusb::DeviceInfo) -> Result<Self, PipeFault> {
        let (link, _) = crate::open(info, format!("{:?}", info.id())).await?;
        let readers = Readers::start(&link);
        Ok(Self { link: Some(link), readers: Some(readers), serial: info.serial_number().map(str::to_owned) })
    }

    pub(super) async fn close(&mut self) {
        if let Some(readers) = self.readers.take() {
            readers.join().await;
        }
        self.link.take();
    }
}

impl Transport for UsbTransport {
    fn lost(&mut self) -> bool {
        let Some(readers) = &mut self.readers else {
            return true;
        };
        let mut lost = false;
        loop {
            match readers.received.try_recv() {
                Ok(Err(_)) | Err(mpsc::error::TryRecvError::Disconnected) => {
                    lost = true;
                    break;
                }
                Ok(Ok(_)) => {}
                Err(mpsc::error::TryRecvError::Empty) => break,
            }
        }
        lost
    }

    fn ready(&mut self) -> Option<Received> {
        self.readers.as_mut()?.received.try_recv().ok()
    }

    async fn send(&mut self, channel: Channel, record: &[u8]) -> Result<(), PipeFault> {
        let bytes = frame(channel, record)?;
        let link = self.link.as_ref().ok_or_else(|| PipeFault::closed("The device is disconnected."))?;
        let pipe = match channel {
            Channel::Control => &link.control,
            Channel::Stream => &link.bulk,
        };
        let write = pipe.write(&bytes);
        tokio::pin!(write);
        tokio::select! {
            result = &mut write => result,
            _ = tokio::time::sleep(Duration::from_secs(15)) => {
                pipe.cancel(Some(Dir::Out));
                let _ = write.await;
                Err(PipeFault::device("USB write timed out."))
            }
        }
    }

    async fn receive(&mut self) -> Received {
        match &mut self.readers {
            Some(readers) => {
                readers.received.recv().await.unwrap_or_else(|| Err(PipeFault::closed("USB readers stopped.")))
            }
            None => Err(PipeFault::closed("The device is disconnected.")),
        }
    }

    async fn reset(&mut self) -> Result<(), PipeFault> {
        if let Some(readers) = self.readers.take() {
            readers.join().await;
        }
        if let Some(link) = &self.link {
            link.control.reset().await?;
            link.bulk.reset().await?;
            self.readers = Some(Readers::start(link));
        }
        Ok(())
    }

    async fn restore(&mut self) -> Result<(), PipeFault> {
        self.close().await;
        let serial = self
            .serial
            .as_deref()
            .ok_or_else(|| PipeFault::closed("Automatic restore requires a device serial number."))?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let mut devices = nusb::list_devices().await.map_err(|e| PipeFault::device(e.to_string()))?.filter(|d| {
                d.vendor_id() == VENDOR_ID && d.product_id() == PRODUCT_ID && d.serial_number() == Some(serial)
            });
            if let Some(info) = devices.next() {
                if devices.next().is_some() {
                    return Err(PipeFault::device("More than one device has the saved serial number."));
                }
                if let Ok((link, _)) = crate::open(&info, format!("{:?}", info.id())).await {
                    self.readers = Some(Readers::start(&link));
                    self.link = Some(link);
                    return Ok(());
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(PipeFault::closed("The device did not reconnect."));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        future::Future,
        pin::Pin,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Mutex,
        },
        task::{Context, Poll, Waker},
    };

    struct Read {
        epoch: Arc<AtomicUsize>,
        subscribed: Option<usize>,
        polls: Arc<AtomicUsize>,
        wake: Arc<Mutex<Option<Waker>>>,
    }

    impl Future for Read {
        type Output = ();
        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            self.polls.fetch_add(1, Ordering::SeqCst);
            let epoch = self.epoch.load(Ordering::SeqCst);
            let subscribed = *self.subscribed.get_or_insert(epoch);
            if subscribed != epoch {
                return Poll::Ready(());
            }
            *self.wake.lock().unwrap() = Some(cx.waker().clone());
            Poll::Pending
        }
    }

    #[tokio::test]
    async fn already_signalled_stop_arms_and_drains_the_unpolled_read() {
        let (stop, mut stopped) = watch::channel(false);
        assert!(!*stopped.borrow());
        let epoch = Arc::new(AtomicUsize::new(0));
        let polls = Arc::new(AtomicUsize::new(0));
        let wake = Arc::new(Mutex::new(None::<Waker>));
        let read = Read { epoch: epoch.clone(), subscribed: None, polls: polls.clone(), wake: wake.clone() };
        stop.send_replace(true);
        let result = read_or_stop(read, &mut stopped, || {
            assert_eq!(polls.load(Ordering::SeqCst), 1);
            epoch.fetch_add(1, Ordering::SeqCst);
            wake.lock().unwrap().take().unwrap().wake();
        })
        .await;
        assert!(result.is_none());
        assert_eq!(polls.load(Ordering::SeqCst), 2);
    }
}
