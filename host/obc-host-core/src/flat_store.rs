//! Shared host card and revision-pinned sources for native and browser hosts.

use std::{
    cell::RefCell,
    collections::BTreeMap,
    io::{self, Read},
    sync::{Arc, Mutex},
};

use obc_formats::io::{ByteSource, Error};
use obc_storage::flat::{
    BlockDevice, DisplayName, EntryFlags, EntryMeta, FlatStore, Handle, Mode, Mutation, ObjectId, ObjectKind,
    PutSource, Revision, Store, StoreError, StoreId,
};

const BLOCK: u64 = 512;
pub(crate) const PAGE: usize = 16 * 1024;
const CARD_BYTES: u64 = 32 * 1024 * 1024 * 1024;
/// Maximum input buffer used while importing a native map.
pub const IMPORT_BUFFER_BYTES: usize = 16 * 1024;

#[derive(Debug)]
pub enum ImportError {
    Io(io::Error),
    Storage(StoreError),
    Mount(Mode),
    RemountRequired,
}

impl From<io::Error> for ImportError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<StoreError> for ImportError {
    fn from(error: StoreError) -> Self {
        Self::Storage(error)
    }
}
impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "object import: {error}"),
            Self::Storage(error) => write!(f, "object storage: {error:?}"),
            Self::Mount(mode) => write!(f, "host card cannot be mounted: {mode:?}"),
            Self::RemountRequired => write!(f, "host card commit is uncertain; close all readers and reopen"),
        }
    }
}

/// The backing storage is private: callers read the committed object, never raw card offsets.
pub(crate) enum HostMedia {
    Memory(RefCell<BTreeMap<u64, Box<[u8; PAGE]>>>),
    #[cfg(not(target_arch = "wasm32"))]
    File(RefCell<NativeCard>),
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct NativeCard {
    file: std::fs::File,
    pub(crate) _temporary: Option<tempfile::TempPath>,
    #[cfg(test)]
    fail_sync_after: std::cell::Cell<Option<usize>>,
    #[cfg(test)]
    pub(crate) fail_sync_before: std::cell::Cell<Option<usize>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl NativeCard {
    fn locked(file: std::fs::File, temporary: Option<tempfile::TempPath>) -> io::Result<Self> {
        file.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => io::Error::from(io::ErrorKind::WouldBlock),
            std::fs::TryLockError::Error(error) => error,
        })?;
        Ok(Self {
            file,
            _temporary: temporary,
            #[cfg(test)]
            fail_sync_after: std::cell::Cell::new(None),
            #[cfg(test)]
            fail_sync_before: std::cell::Cell::new(None),
        })
    }
    fn sync(&self) -> io::Result<()> {
        #[cfg(test)]
        if let Some(left) = self.fail_sync_before.get() {
            self.fail_sync_before.set(left.checked_sub(1).filter(|&left| left != 0));
            if left == 1 {
                return Err(io::Error::other("injected failure before file sync"));
            }
        }
        self.file.sync_all()?;
        #[cfg(test)]
        if let Some(left) = self.fail_sync_after.get() {
            self.fail_sync_after.set(left.checked_sub(1).filter(|&left| left != 0));
            if left == 1 {
                return Err(io::Error::other("injected failure after successful file sync"));
            }
        }
        Ok(())
    }
}

impl HostMedia {
    fn range(lba: u64, len: usize) -> io::Result<u64> {
        let offset = lba.checked_mul(BLOCK).ok_or(io::ErrorKind::InvalidInput)?;
        if !len.is_multiple_of(BLOCK as usize) || offset.checked_add(len as u64).is_none_or(|end| end > CARD_BYTES) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        Ok(offset)
    }
}

impl BlockDevice for HostMedia {
    type Error = io::Error;
    fn block_count(&self) -> io::Result<u64> {
        Ok(CARD_BYTES / BLOCK)
    }

    fn read(&self, lba: u64, buf: &mut [u8]) -> io::Result<()> {
        let offset = Self::range(lba, buf.len())?;
        match self {
            Self::Memory(pages) => {
                let pages = pages.borrow();
                let mut done = 0;
                while done < buf.len() {
                    let at = offset + done as u64;
                    let within = (at % PAGE as u64) as usize;
                    let take = (PAGE - within).min(buf.len() - done);
                    let out = &mut buf[done..done + take];
                    if let Some(page) = pages.get(&(at / PAGE as u64)) {
                        out.copy_from_slice(&page[within..within + take]);
                    } else {
                        out.fill(0);
                    }
                    done += take;
                }
                Ok(())
            }
            #[cfg(not(target_arch = "wasm32"))]
            Self::File(file) => {
                use std::io::{Seek, SeekFrom};
                let mut file = file.borrow_mut();
                file.file.seek(SeekFrom::Start(offset))?;
                file.file.read_exact(buf)
            }
        }
    }

    fn write(&self, lba: u64, buf: &[u8]) -> io::Result<()> {
        let offset = Self::range(lba, buf.len())?;
        match self {
            Self::Memory(pages) => {
                let mut pages = pages.borrow_mut();
                let mut done = 0;
                while done < buf.len() {
                    let at = offset + done as u64;
                    let within = (at % PAGE as u64) as usize;
                    let take = (PAGE - within).min(buf.len() - done);
                    let page = pages.entry(at / PAGE as u64).or_insert_with(|| Box::new([0; PAGE]));
                    page[within..within + take].copy_from_slice(&buf[done..done + take]);
                    done += take;
                }
                Ok(())
            }
            #[cfg(not(target_arch = "wasm32"))]
            Self::File(file) => {
                use std::io::{Seek, SeekFrom, Write};
                let mut file = file.borrow_mut();
                file.file.seek(SeekFrom::Start(offset))?;
                file.file.write_all(buf)
            }
        }
    }

    fn sync(&self) -> io::Result<()> {
        match self {
            Self::Memory(_) => Ok(()),
            #[cfg(not(target_arch = "wasm32"))]
            Self::File(file) => file.borrow().sync(),
        }
    }
}

pub(crate) struct MountedStore {
    pub(crate) card: FlatStore<HostMedia>,
    remount_required: bool,
    pub(crate) persistent: bool,
}

impl MountedStore {
    pub(crate) fn new(card: FlatStore<HostMedia>, persistent: bool) -> Self {
        Self { card, remount_required: false, persistent }
    }

    /// Confirm a catalog observed through the live OS cache before granting durable authority.
    /// Call under the owner lock after checking the expected head; failure fences all writers.
    pub(crate) fn confirm_durable(&mut self) -> Result<(), StoreError> {
        self.ready()?;
        if self.card.sync_media().is_err() {
            self.remount_required = true;
            return Err(StoreError::Media);
        }
        Ok(())
    }

    /// Catalog publication can become durable before its final sync reports an error.
    pub(crate) fn commit(&mut self, mutations: &[Mutation]) -> Result<u64, StoreError> {
        let result = self.ready()?.commit(mutations);
        if result == Err(StoreError::Media) {
            self.remount_required = true;
        }
        result
    }

    pub(crate) fn ready(&self) -> Result<&FlatStore<HostMedia>, StoreError> {
        if self.remount_required {
            Err(StoreError::Media)
        } else {
            Ok(&self.card)
        }
    }
}

type Owner = Arc<Mutex<MountedStore>>;

pub(crate) struct Lease {
    pub(crate) owner: Owner,
    handle: Option<Handle>,
    len: u64,
    store_id: StoreId,
}

impl Drop for Lease {
    fn drop(&mut self) {
        // A poisoned owner has already failed. Still return its handle during unwind.
        let store = self.owner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        store.card.close(self.handle.take().expect("the last reader owns the handle"));
    }
}

/// Cloneable object bytes pinned to one object revision. The last clone closes the lease.
#[derive(Clone)]
pub struct ObjectSource(pub(crate) Arc<Lease>);

impl ObjectSource {
    pub(crate) fn open(owner: Owner, id: ObjectId, revision: Option<Revision>) -> Result<Self, StoreError> {
        let store = owner.lock().map_err(|_| StoreError::Media)?;
        let card = store.ready()?;
        let handle = card.open(id, revision)?;
        let len = card.handle_len(&handle).expect("a just-opened handle resolves");
        let store_id = card.store_id();
        drop(store);
        Ok(Self(Arc::new(Lease { owner, handle: Some(handle), len, store_id })))
    }

    /// Exact source identity, independent of unrelated catalog commits.
    pub fn same_revision(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0.owner, &other.0.owner)
            && self.store_id() == other.store_id()
            && self.id() == other.id()
            && self.revision() == other.revision()
    }

    /// A retained old revision stays readable, but cannot authorize new planner work.
    pub fn is_current(&self) -> bool {
        let Ok(owner) = self.0.owner.lock() else { return false };
        let Ok(card) = owner.ready() else { return false };
        if card.store_id() != self.store_id() {
            return false;
        }
        card.current_revision(self.id()).is_ok_and(|head| head == Some(self.revision()))
    }

    /// Full current stored identity, independent of a particular mount's owner allocation.
    pub fn fingerprint(&self) -> Option<obc_formats::assistant::PayloadFingerprint> {
        let owner = self.0.owner.lock().ok()?;
        let card = owner.ready().ok()?;
        let entry = card.entries().find(|entry| {
            entry.id == self.id()
                && entry.revision == self.revision()
                && (entry.flags == EntryFlags::NONE || (entry.kind == ObjectKind::Route && entry.flags.is_route_head()))
        })?;
        card.entries_ok().then(|| obc_storage::flat::metadata::fingerprint(entry))
    }

    pub fn store_id(&self) -> StoreId {
        self.0.store_id
    }

    pub fn id(&self) -> ObjectId {
        self.0.handle.as_ref().unwrap().id()
    }
    pub fn revision(&self) -> Revision {
        self.0.handle.as_ref().unwrap().revision()
    }
}

impl ByteSource for ObjectSource {
    fn len(&self) -> u64 {
        self.0.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        // Match StoreSource: check the range before media, then fill across short reads.
        let end = offset.checked_add(buf.len() as u64).ok_or(Error::BadOffset)?;
        if end > self.0.len {
            return Err(Error::BadOffset);
        }
        if buf.is_empty() {
            return Ok(());
        }
        let store = self.0.owner.lock().map_err(|_| Error::Io)?;
        let handle = self.0.handle.as_ref().unwrap();
        let mut done = 0;
        while done < buf.len() {
            match store.card.read(handle, offset + done as u64, &mut buf[done..]) {
                Ok(0) | Err(_) => return Err(Error::Io),
                Ok(n) => done += n,
            }
        }
        Ok(())
    }
}

/// One host card owner. Object readers keep its media and exclusive native file lock
/// alive after the front-end owner drops. An uncertain commit requires a fresh mount.
#[derive(Clone)]
pub struct HostStore(pub(crate) Owner);

impl HostStore {
    #[cfg(test)]
    pub(crate) fn remount_memory_snapshot(&self) -> Self {
        let mut owner = self.0.lock().unwrap();
        let HostMedia::Memory(pages) = owner.card.device() else { panic!("memory card required") };
        let media = HostMedia::Memory(RefCell::new(pages.borrow().clone()));
        owner.remount_required = true;
        Self(Arc::new(Mutex::new(MountedStore::new(FlatStore::mount(media), false))))
    }

    pub fn memory() -> Result<Self, ImportError> {
        Self::new(HostMedia::Memory(RefCell::default()))
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn temporary() -> Result<Self, ImportError> {
        let (file, path) = tempfile::NamedTempFile::new()?.into_parts();
        let card = NativeCard::locked(file, Some(path))?;
        card.file.set_len(CARD_BYTES)?;
        Self::new(HostMedia::File(RefCell::new(card)))
    }

    fn new(media: HostMedia) -> Result<Self, ImportError> {
        let persistent = !matches!(media, HostMedia::Memory(_));
        let mut identity = [0; 16];
        getrandom::getrandom(&mut identity).map_err(|error| io::Error::other(error.to_string()))?;
        Ok(Self(Arc::new(Mutex::new(MountedStore::new(FlatStore::initialize(media, StoreId(identity))?, persistent)))))
    }

    /// Create a new sparse card under an existing directory. Never overwrite a path.
    /// The card is initialized under a temporary sibling and published by a rename, so the
    /// card name never names an incomplete card. When this returns the bytes and the name
    /// are both durable. A crash before the rename leaves no card name and one temporary
    /// sibling. If the rename lands and the directory barrier then fails, the card keeps its
    /// name and creation still reports the failure.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn create_file(path: impl AsRef<std::path::Path>) -> Result<Self, ImportError> {
        let path = path.as_ref();
        let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));
        let (file, temporary) = tempfile::NamedTempFile::new_in(parent)?.into_parts();
        let card = NativeCard::locked(file, None)?;
        card.file.set_len(CARD_BYTES)?;
        // Initialization ends on a file sync barrier, so only the name remains to publish.
        let owner = Self::new(HostMedia::File(RefCell::new(card)))?;
        publish(temporary, path, parent)?;
        Ok(owner)
    }

    /// Mount an existing card, including the common store's recording recovery.
    /// This never initializes or resets a card. Exhausted readable stores remain read-only.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_file(path: impl AsRef<std::path::Path>) -> Result<Self, ImportError> {
        let file = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
        let card = NativeCard::locked(file, None)?;
        let metadata = card.file.metadata()?;
        if !metadata.is_file() || metadata.len() != CARD_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "expected a regular 32 GiB host card").into());
        }
        let store = FlatStore::mount(HostMedia::File(RefCell::new(card)));
        if !store.mode().readable() {
            return Err(ImportError::Mount(store.mode()));
        }
        Ok(Self(Arc::new(Mutex::new(MountedStore::new(store, true)))))
    }

    pub fn store_id(&self) -> Result<StoreId, StoreError> {
        Ok(self.0.lock().map_err(|_| StoreError::Media)?.card.store_id())
    }

    pub fn mode(&self) -> Result<Mode, StoreError> {
        Ok(self.0.lock().map_err(|_| StoreError::Media)?.ready()?.mode())
    }

    pub fn open(&self, id: ObjectId, revision: Revision) -> Result<ObjectSource, StoreError> {
        ObjectSource::open(self.0.clone(), id, Some(revision))
    }

    pub(crate) fn entries(&self) -> Result<Vec<EntryMeta>, StoreError> {
        let store = self.0.lock().map_err(|_| StoreError::Media)?;
        let card = store.ready()?;
        let entries = card
            .entries()
            .filter(|entry| {
                entry.flags == EntryFlags::NONE || (entry.kind == ObjectKind::Route && entry.flags.is_route_head())
            })
            .collect();
        if !card.entries_ok() {
            return Err(StoreError::Media);
        }
        Ok(entries)
    }

    /// Remove several route heads in one commit. A head that moved is skipped, not refused.
    pub(crate) fn remove_routes(&self, heads: &[(ObjectId, Revision)]) -> Result<(), StoreError> {
        let mut owner = self.0.lock().map_err(|_| StoreError::Media)?;
        let store = owner.ready()?;
        let mut batch = Vec::new();
        for &(id, revision) in heads {
            let current = store.entries().any(|entry| {
                entry.id == id
                    && entry.revision == revision
                    && entry.kind == ObjectKind::Route
                    && entry.flags.is_route_head()
            });
            if !store.entries_ok() {
                return Err(StoreError::Media);
            }
            if current {
                batch.push(Mutation::Remove { id, revision });
            }
        }
        if batch.is_empty() {
            return Ok(());
        }
        let result = store.commit(&batch);
        if result == Err(StoreError::Media) {
            owner.remount_required = true;
        }
        result.map(|_| ())
    }

    pub(crate) fn remove(&self, kind: ObjectKind, id: ObjectId, revision: Revision) -> Result<(), StoreError> {
        let mut owner = self.0.lock().map_err(|_| StoreError::Media)?;
        let store = owner.ready()?;
        let head = store.entries().find(|entry| {
            entry.id == id
                && (entry.flags == EntryFlags::NONE || (entry.kind == ObjectKind::Route && entry.flags.is_route_head()))
        });
        if !store.entries_ok() {
            return Err(StoreError::Media);
        }
        let head = head.ok_or(StoreError::NotFound)?;
        if head.kind != kind {
            return Err(StoreError::Invalid);
        }
        if head.revision != revision {
            return Err(StoreError::RevisionConflict { current: head.revision });
        }
        if kind == ObjectKind::Route {
            obc_storage::flat::metadata::check_route_change(store, id).map_err(|error| match error {
                obc_storage::flat::metadata::Error::Store(StoreError::Busy) => StoreError::Busy,
                _ => StoreError::Media,
            })?;
        }
        let result = store.commit(&[Mutation::Remove { id, revision }]);
        if result == Err(StoreError::Media) {
            owner.remount_required = true;
        }
        result.map(|_| ())
    }

    /// Return the committed metadata. Opening a reader is a separate, retryable operation.
    pub(crate) fn import(
        &self,
        kind: ObjectKind,
        previous: Option<(ObjectId, Revision)>,
        input: &mut impl Read,
        len: u64,
        name: DisplayName,
    ) -> Result<EntryMeta, ImportError> {
        self.import_with_capacity(kind, previous, input, len, name, 1)
    }

    pub(crate) fn import_computed_route(&self, bytes: &[u8]) -> Result<EntryMeta, ImportError> {
        self.import_with_capacity(
            ObjectKind::Route,
            None,
            &mut &bytes[..],
            bytes.len() as u64,
            DisplayName::default(),
            2,
        )
    }

    fn import_with_capacity(
        &self,
        kind: ObjectKind,
        previous: Option<(ObjectId, Revision)>,
        input: &mut impl Read,
        len: u64,
        name: DisplayName,
        commits: u64,
    ) -> Result<EntryMeta, ImportError> {
        let mut owner = self.0.lock().map_err(|_| StoreError::Media)?;
        if owner.remount_required {
            return Err(ImportError::RemountRequired);
        }
        let store = &owner.card;
        if kind == ObjectKind::Route {
            if let Some((id, _)) = previous {
                obc_storage::flat::metadata::check_route_change(store, id).map_err(|error| match error {
                    obc_storage::flat::metadata::Error::Store(error) => ImportError::Storage(error),
                    _ => ImportError::RemountRequired,
                })?;
            }
        }
        // A computed route needs one later commit to retract an abandoned publication.
        if !store.has_commit_capacity(commits) {
            return Err(StoreError::ReadOnly.into());
        }
        if matches!(kind, ObjectKind::Route | ObjectKind::Ride) && previous.is_none() {
            let count = store
                .entries()
                .filter(|entry| {
                    entry.kind == kind
                        && ((entry.flags == EntryFlags::NONE
                            || (entry.kind == ObjectKind::Route && entry.flags.is_route_head()))
                            || entry.flags == EntryFlags::RECORDING)
                })
                .count();
            if !store.entries_ok() {
                return Err(StoreError::Media.into());
            }
            let capacity = if kind == ObjectKind::Route { obc_app::MAX_ROUTES } else { obc_app::MAX_RIDES };
            if count >= capacity {
                return Err(StoreError::Busy.into());
            }
        }
        let id = previous.map_or_else(|| store.next_object_id(), |(id, _)| id);
        let revision = previous.map_or(Ok(Revision(1)), |(_, revision)| {
            revision.0.checked_add(1).map(Revision).ok_or(StoreError::ReadOnly)
        })?;
        let mut allocation = store.allocate(len)?;
        let mut committing = false;
        let result = (|| {
            let mut buffer = [0; IMPORT_BUFFER_BYTES];
            let mut crc = obc_crc::Crc32::new();
            let mut remaining = len;
            while remaining != 0 {
                let take = remaining.min(buffer.len() as u64) as usize;
                input.read_exact(&mut buffer[..take])?;
                store.write(&mut allocation, &buffer[..take])?;
                crc.update(&buffer[..take]);
                remaining -= take as u64;
            }
            if input.read(&mut buffer[..1])? != 0 {
                return Err(ImportError::Io(io::ErrorKind::InvalidData.into()));
            }
            let meta = EntryMeta {
                added_at_utc: 0,
                id,
                revision,
                kind,
                flags: EntryFlags::NONE,
                payload_len: len,
                payload_crc: crc.finalize(),
                name,
            };
            let put = Mutation::Put { meta, source: PutSource::Fresh(allocation) };
            committing = true;
            if let Some((_, old_revision)) = previous {
                store.commit(&[Mutation::Remove { id, revision: old_revision }, put])?;
            } else {
                store.commit(&[put])?;
            }
            Ok(meta)
        })();
        if committing && matches!(result, Err(ImportError::Storage(StoreError::Media))) {
            // The final gate can reach disk before its write or sync reports failure.
            // Keep its reservation and refuse reuse until a fresh mount chooses the catalog.
            owner.remount_required = true;
            return Err(ImportError::RemountRequired);
        }
        if result.is_err() {
            store.cancel(allocation);
        }
        result
    }
}

/// Give a complete card its final name. The move refuses an occupied name; it never
/// replaces a file. A rename is durable only after the directory entry reaches storage.
#[cfg(unix)]
fn publish(temporary: tempfile::TempPath, path: &std::path::Path, parent: &std::path::Path) -> io::Result<()> {
    if let Err(refused) = temporary.persist_noclobber(path) {
        // exFAT and many network mounts do not implement an exclusive rename. macOS reports
        // `ENOTSUP`, which `ErrorKind` leaves uncategorized, so anything but an occupied
        // name falls back to a reserved name.
        if refused.error.kind() == io::ErrorKind::AlreadyExists {
            return Err(refused.error);
        }
        reserve_and_rename(refused.path, path)?;
    }
    std::fs::File::open(parent)?.sync_all()
}

/// Publish where an exclusive rename is unavailable. The exclusive create holds the name
/// against every other owner, and the plain rename replaces that reservation with the card.
#[cfg(unix)]
fn reserve_and_rename(temporary: tempfile::TempPath, path: &std::path::Path) -> io::Result<()> {
    std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    temporary.persist(path).map_err(|error| error.error)
}

/// Windows cannot sync a directory. `MOVEFILE_WRITE_THROUGH` returns only once the move
/// is on disk, and the absent `MOVEFILE_REPLACE_EXISTING` keeps an occupied name refused.
#[cfg(windows)]
fn publish(temporary: tempfile::TempPath, path: &std::path::Path, _parent: &std::path::Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};

    fn wide(path: &std::path::Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
    }
    // The move must see an ordinary file: `keep` clears the temporary attribute.
    let from = temporary.keep().map_err(|error| error.error)?;
    let (source, target) = (wide(&from), wide(path));
    // SAFETY: both arguments are null-terminated wide strings that outlive the call.
    let moved = unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), MOVEFILE_WRITE_THROUGH) };
    if moved == 0 {
        let error = io::Error::last_os_error();
        let _ = std::fs::remove_file(&from);
        return Err(error);
    }
    Ok(())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
