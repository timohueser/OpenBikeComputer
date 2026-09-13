//! Synchronous writer transport for the actual board recorder over a faulting FlatStore.
use obc_storage::flat::*;
use std::cell::RefCell;

pub type FlatCard = &'static sim::FaultOnce<&'static sim::SparseDisk>;
pub type Reply = crate::signal::Signal<crate::blocking_mutex::raw::CriticalSectionRawMutex, ()>;
#[allow(clippy::large_enum_variant)] // Match the board's owned, inline commit batch.
pub enum Request {
    Allocate { bytes: u64 },
    Commit { batch: heapless::Vec<Mutation, 8> },
    Cancel { allocation: Allocation },
    Journal { checkpoint: RideCheckpoint<'static> },
}
pub enum Outcome {
    Allocated(Allocation),
    Committed(()),
    Done,
}
#[derive(Debug, PartialEq, Eq)]
pub struct Attempt {
    pub id: ObjectId,
    pub revision: Revision,
    pub append: Vec<u8>,
    pub crc: u32,
    pub resume: [u8; RIDE_RESUME_LEN],
}
#[derive(Clone, Copy)]
pub struct Writer {
    pub store: &'static FlatStore<FlatCard>,
    pub attempts: &'static RefCell<Vec<Attempt>>,
}
impl Writer {
    pub fn new(store: &'static FlatStore<FlatCard>) -> Self {
        Self { store, attempts: Box::leak(Box::default()) }
    }
    pub async fn call(&self, request: Request, _reply: &Reply) -> Result<Outcome, StoreError> {
        match request {
            Request::Allocate { bytes } => self.store.allocate(bytes).map(Outcome::Allocated),
            Request::Commit { batch } => self.store.commit(&batch).map(|_| Outcome::Committed(())),
            Request::Cancel { allocation } => {
                self.store.cancel(allocation);
                Ok(Outcome::Done)
            }
            Request::Journal { checkpoint } => {
                self.attempts.borrow_mut().push(Attempt {
                    id: checkpoint.id,
                    revision: checkpoint.revision,
                    append: checkpoint.append.to_vec(),
                    crc: checkpoint.payload_crc,
                    resume: *checkpoint.resume,
                });
                self.store.journal(checkpoint).map(|()| Outcome::Done)
            }
        }
    }
}
