//! Concrete borrowed owners for the card's map and volume-set behavior.
//!
//! Persistent handles, extent references, mount state, and the shared upload handle remain fields
//! of [`Storage`]. `Maps` is the boot/runtime view over that state; `MapTransfers` additionally
//! borrows the RRAM id floor and the caller-owned eight-byte [`obc_app::SetUpload`] session.

use super::*;
use obc_ble::{Receiver, SetPart, TransferControl, TransferStatus};

pub(crate) struct Maps<'a> {
    storage: &'a mut Storage,
}

impl<'a> Maps<'a> {
    pub(super) fn new(storage: &'a mut Storage) -> Self {
        Self { storage }
    }

    pub(crate) fn open(&mut self) -> Option<u32> {
        self.storage.open_map()
    }

    #[cfg(has_nav)]
    pub(crate) fn open_terrain(&mut self) -> Option<&'static dyn ByteSource> {
        self.storage.open_terrain()
    }

    pub(crate) fn source(storage: &Storage) -> Option<MapSource<'_>> {
        storage.map_source()
    }

    pub(crate) fn name(self) -> &'a str {
        let storage: &'a Storage = self.storage;
        storage.map_name()
    }

    pub(crate) fn boot_fault(&self) -> obc_app::BootFault {
        self.storage.boot_fault()
    }

    fn next_id(&self) -> u16 {
        self.storage.next_map_id_from_scan()
    }

    pub(crate) fn sweep_aborted_maps(&mut self) -> usize {
        self.storage.sweep_aborted_maps()
    }

    pub(crate) fn sweep_aborted_sets(&mut self) -> usize {
        self.storage.sweep_aborted_sets()
    }
}

/// Link-time map/set operations over the card, durable id floor, and caller-owned set session.
pub(crate) struct MapTransfers<'a> {
    storage: &'a mut Option<Storage>,
    settings: &'a mut crate::settings::RramSettingsStore,
    set_upload: &'a mut Option<obc_app::SetUpload>,
}

impl<'a> MapTransfers<'a> {
    pub(crate) fn new(
        storage: &'a mut Option<Storage>,
        settings: &'a mut crate::settings::RramSettingsStore,
        set_upload: &'a mut Option<obc_app::SetUpload>,
    ) -> Self {
        Self { storage, settings, set_upload }
    }

    pub(crate) fn map_open(&self, desc: &TransferControl) -> Result<Receiver, TransferStatus> {
        let Some(storage) = self.storage.as_ref() else { return Err(TransferStatus::Error) };
        if let Some(status) = TransferStatus::map_announce_reject(
            desc.object_id,
            desc.total_len,
            obc_formats::obcm::HEADER_LEN as u32,
            storage.card_free_bytes(),
            MAP_FREE_HEADROOM,
        ) {
            return Err(status);
        }
        Receiver::new(desc).map_err(|_| TransferStatus::Error)
    }

    pub(crate) fn map_begin(&mut self) -> Option<u16> {
        let scan_next = self.storage.as_mut()?.maps().next_id();
        let id = self.settings.load_map_mark().unwrap_or(0).max(scan_next);
        if id == u16::MAX {
            defmt::warn!("store: map id space exhausted — refusing the upload");
            return None;
        }
        self.settings.save_map_mark(id.saturating_add(1));
        self.storage.as_mut()?.map_upload_begin(id).then_some(id)
    }

    pub(crate) fn map_finish(&mut self, rx: &Receiver, id: u16, magic: [u8; 4]) -> TransferStatus {
        let Some(outcome) = rx.outcome() else { return TransferStatus::Error };
        if outcome.status != TransferStatus::Committed {
            if let Some(storage) = self.storage.as_mut() {
                storage.map_upload_abort(id);
            }
            return outcome.status;
        }
        let Some(storage) = self.storage.as_mut() else { return TransferStatus::Error };
        if storage.map_upload_commit(id, magic).is_none() {
            return TransferStatus::Error;
        }
        if let Some(name) = map_file_name_for(id) {
            storage.save_selected_map(&name);
        }
        TransferStatus::Committed
    }

    pub(crate) fn abort_map(&mut self, id: u16) {
        if let Some(storage) = self.storage.as_mut() {
            storage.map_upload_abort(id);
        }
    }

    pub(crate) fn shard_open(&self, desc: &TransferControl) -> Result<(Receiver, SetPart), TransferStatus> {
        let Some(storage) = self.storage.as_ref() else { return Err(TransferStatus::Error) };
        let Some(part) = SetPart::decode(desc.object_id) else { return Err(TransferStatus::NotFound) };
        let fresh =
            obc_app::shard_announce(self.set_upload.as_ref(), part.shard_count, part.index, SD_SET_MAX_SHARDS as u8)
                .map_err(set_reject_status)?;
        if fresh && storage.next_set_id_from_scan() > obc_formats::obcs::MAX_SET_ID {
            return Err(TransferStatus::StorageFull);
        }
        self.accept_file(desc, obc_formats::obcm::HEADER_LEN as u32)?;
        Receiver::new(desc).map(|rx| (rx, part)).map_err(|_| TransferStatus::Error)
    }

    pub(crate) fn terrain_open(&self, desc: &TransferControl) -> Result<Receiver, TransferStatus> {
        if desc.object_id != TransferControl::NEW_OBJECT_ID {
            return Err(TransferStatus::NotFound);
        }
        obc_app::terrain_announce(self.set_upload.as_ref()).map_err(set_reject_status)?;
        self.accept_file(desc, obc_formats::obct::HEADER_LEN as u32)?;
        Receiver::new(desc).map_err(|_| TransferStatus::Error)
    }

    pub(crate) fn manifest_open(&self, desc: &TransferControl) -> Result<Receiver, TransferStatus> {
        if self.storage.is_none() {
            return Err(TransferStatus::Error);
        }
        if desc.object_id != TransferControl::NEW_OBJECT_ID {
            return Err(TransferStatus::NotFound);
        }
        obc_app::manifest_announce(self.set_upload.as_ref(), desc.total_len).map_err(set_reject_status)?;
        Receiver::new(desc).map_err(|_| TransferStatus::Error)
    }

    fn accept_file(&self, desc: &TransferControl, minimum: u32) -> Result<(), TransferStatus> {
        let Some(storage) = self.storage.as_ref() else { return Err(TransferStatus::Error) };
        if desc.total_len < minimum {
            return Err(TransferStatus::Error);
        }
        if storage.card_free_bytes().is_some_and(|free| desc.total_len as u64 + MAP_FREE_HEADROOM > free) {
            return Err(TransferStatus::StorageFull);
        }
        Ok(())
    }

    pub(crate) fn shard_begin(&mut self, part: SetPart) -> Option<u16> {
        let id = match *self.set_upload {
            Some(session) => session.id(),
            None => {
                let id = self.storage.as_ref()?.next_set_id_from_scan();
                if id > obc_formats::obcs::MAX_SET_ID {
                    defmt::warn!("store: volume-set id space exhausted — refusing the upload");
                    return None;
                }
                if !self.storage.as_mut()?.set_upload_begin(id) {
                    return None;
                }
                *self.set_upload = Some(obc_app::SetUpload::new(id, part.shard_count));
                id
            }
        };
        if self.storage.as_mut().is_some_and(|storage| storage.set_shard_begin(id, part.index as usize)) {
            return Some(id);
        }
        self.set_upload.as_mut()?.clear(part.index);
        None
    }

    pub(crate) fn shard_finish(&mut self, rx: &Receiver, id: u16, part: SetPart, magic: [u8; 4]) -> TransferStatus {
        let Some(outcome) = rx.outcome() else { return TransferStatus::Error };
        if outcome.status != TransferStatus::Committed {
            if let Some(storage) = self.storage.as_mut() {
                storage.set_shard_discard(id, part.index as usize);
            }
            self.forget_shard(part.index);
            return outcome.status;
        }
        let Some(storage) = self.storage.as_mut() else { return TransferStatus::Error };
        if storage.set_shard_commit(id, part.index as usize, magic).is_none() {
            self.forget_shard(part.index);
            return TransferStatus::Error;
        }
        if let Some(session) = self.set_upload.as_mut() {
            session.mark(part.index);
        }
        TransferStatus::Committed
    }

    fn forget_shard(&mut self, index: u8) {
        if let Some(session) = self.set_upload.as_mut() {
            session.clear(index);
        }
    }

    pub(crate) fn terrain_begin(&mut self) -> Option<u16> {
        let id = self.set_upload.as_ref()?.id();
        if self.storage.as_mut().is_some_and(|storage| storage.set_terrain_begin(id)) {
            return Some(id);
        }
        self.set_upload.as_mut()?.clear_terrain();
        None
    }

    pub(crate) fn terrain_finish(&mut self, rx: &Receiver, id: u16, magic: [u8; 4]) -> TransferStatus {
        let Some(outcome) = rx.outcome() else { return TransferStatus::Error };
        if outcome.status != TransferStatus::Committed {
            if let Some(storage) = self.storage.as_mut() {
                storage.set_terrain_discard(id);
            }
            self.forget_terrain();
            return outcome.status;
        }
        let Some(storage) = self.storage.as_mut() else { return TransferStatus::Error };
        if storage.set_terrain_commit(id, magic).is_none() {
            self.forget_terrain();
            return TransferStatus::Error;
        }
        if let Some(session) = self.set_upload.as_mut() {
            session.mark_terrain();
        }
        TransferStatus::Committed
    }

    fn forget_terrain(&mut self) {
        if let Some(session) = self.set_upload.as_mut() {
            session.clear_terrain();
        }
    }

    pub(crate) fn manifest_begin(&mut self) -> Option<u16> {
        let id = self.set_upload.as_ref()?.id();
        self.storage.as_mut()?.set_manifest_begin(id).then_some(id)
    }

    pub(crate) fn manifest_finish(&mut self, rx: &Receiver, id: u16, magic: [u8; 4]) -> TransferStatus {
        let Some(outcome) = rx.outcome() else { return TransferStatus::Error };
        *self.set_upload = None;
        if outcome.status != TransferStatus::Committed {
            if let Some(storage) = self.storage.as_mut() {
                storage.set_upload_abort(id);
            }
            return outcome.status;
        }
        let Some(storage) = self.storage.as_mut() else { return TransferStatus::Error };
        if storage.set_manifest_commit(id, magic).is_none() {
            return TransferStatus::Error;
        }
        if let Some(name) = obc_formats::obcs::manifest_name(id) {
            if let Ok(short) = ShortFileName::create_from_str(name.as_str()) {
                storage.save_selected_map(&short);
            }
        }
        TransferStatus::Committed
    }

    pub(crate) fn abort_set(&mut self) {
        let Some(session) = self.set_upload.take() else { return };
        if let Some(storage) = self.storage.as_mut() {
            storage.set_upload_abort(session.id());
        }
    }
}

const fn set_reject_status(reject: obc_app::SetReject) -> TransferStatus {
    match reject {
        obc_app::SetReject::Part => TransferStatus::NotFound,
        obc_app::SetReject::Shards => TransferStatus::StorageFull,
        obc_app::SetReject::Mismatch | obc_app::SetReject::ManifestEarly | obc_app::SetReject::Length => {
            TransferStatus::Error
        }
    }
}
