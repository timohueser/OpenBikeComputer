use super::*;

#[derive(Default)]
pub(super) struct Layout {
    pub(super) rows: usize,
    pub(super) checkpoint_len: usize,
    pub(super) records: usize,
}

pub(super) struct Reader {
    head: EntryMeta,
    handle: crate::flat::Handle,
    pub(super) layout: Layout,
    cache: [u8; 512],
    cached_at: usize,
}

type ReadAt<'a> = dyn FnMut(usize, &mut [u8]) -> Result<(), Error> + 'a;

impl Layout {
    #[inline(never)]
    pub(super) fn parse(len: usize, read: &mut ReadAt<'_>) -> Result<(Self, StoreId), Error> {
        let mut bytes = [0; HEADER_LEN];
        let header = &mut bytes[..];
        read(0, header)?;
        let rows = u16::from_le_bytes(header[10..12].try_into().unwrap()) as usize;
        let version = u16::from_le_bytes(header[12..14].try_into().unwrap());
        let checkpoint_len = u16::from_le_bytes(header[14..16].try_into().unwrap()) as usize;
        let end = HEADER_LEN + rows * ROW_LEN + checkpoint_len;
        if &header[..4] != b"OBRM"
            || header[4..10] != [2, 0, 32, 0, 40, 0]
            || !matches!((version, checkpoint_len), (0, 0) | (CHECKPOINT_VERSION, CHECKPOINT_LEN))
            || len < end
            || !(len - end).is_multiple_of(RECORD_LEN)
            || (len - end) / RECORD_LEN > MAX_RECORDS
        {
            return Err(Error::Invalid);
        }
        let identity = StoreId(header[16..32].try_into().unwrap());
        let reader = Self { rows, checkpoint_len, records: (len - end) / RECORD_LEN };
        let mut record = [0; RECORD_LEN];
        if checkpoint_len != 0 {
            read(HEADER_LEN + rows * ROW_LEN, &mut record)?;
            if NavigatorCheckpoint::decode(&record).is_none() {
                return Err(Error::Invalid);
            }
        }
        let mut keys = [0; MAX_RECORDS];
        for i in 0..reader.records {
            read(end + i * RECORD_LEN, &mut record)?;
            let key = TripProgress::decode(&record).ok_or(Error::Invalid)?.key;
            if keys[..i].contains(&key) {
                return Err(Error::Invalid);
            }
            keys[i] = key;
        }
        let mut previous = ObjectId::NONE;
        for i in 0..rows {
            read(HEADER_LEN + i * ROW_LEN, &mut record[..ROW_LEN])?;
            let row = Row::decode(&record[..ROW_LEN])?;
            if row.id <= previous {
                return Err(Error::Invalid);
            }
            previous = row.id;
        }
        if rows > MAX_RIDES {
            return Err(Error::Capacity);
        }
        Ok((reader, identity))
    }
}

impl Reader {
    #[inline(never)]
    pub(super) fn with<D: BlockDevice, T>(
        store: &FlatStore<D>,
        visit: impl FnOnce(Option<&mut Self>) -> Result<T, Error>,
    ) -> Result<T, Error> {
        check_mode(store)?;
        let Some(head) = singleton(store)? else {
            return visit(None);
        };
        let len = usize::try_from(head.payload_len).map_err(|_| Error::Capacity)?;
        if len > MAX_LEN {
            return Err(Error::Capacity);
        }
        let handle = store.open(head.id, Some(head.revision))?;
        let mut reader = Self { head, handle, layout: Layout::default(), cache: [0; 512], cached_at: usize::MAX };
        let result = (|| {
            reader.validate(store, len)?;
            visit(Some(&mut reader))
        })();
        store.close(reader.handle);
        result
    }

    #[inline(never)]
    fn validate<D: BlockDevice>(&mut self, store: &FlatStore<D>, len: usize) -> Result<(), Error> {
        let mut crc = obc_crc::Crc32::new();
        for offset in (0..len).step_by(512) {
            self.fill(store, offset)?;
            crc.update(&self.cache[..(len - offset).min(512)]);
        }
        if crc.finalize() != self.head.payload_crc || len < HEADER_LEN {
            return Err(Error::Invalid);
        }
        let (layout, identity) = Layout::parse(len, &mut |offset, out| self.read(store, offset, out))?;
        if identity != store.store_id() {
            return Err(Error::WrongStore);
        }
        self.layout = layout;
        Ok(())
    }

    fn fill<D: BlockDevice>(&mut self, store: &FlatStore<D>, at: usize) -> Result<(), Error> {
        if self.cached_at != at {
            let want = (self.head.payload_len as usize - at).min(512);
            if store.read(&self.handle, at as u64, &mut self.cache[..want])? != want {
                return Err(Error::Invalid);
            }
            self.cached_at = at;
        }
        Ok(())
    }

    fn read<D: BlockDevice>(
        &mut self,
        store: &FlatStore<D>,
        mut offset: usize,
        mut out: &mut [u8],
    ) -> Result<(), Error> {
        while !out.is_empty() {
            let at = offset / 512 * 512;
            self.fill(store, at)?;
            let from = offset - at;
            let take = out.len().min(512 - from);
            out[..take].copy_from_slice(&self.cache[from..from + take]);
            offset += take;
            out = &mut out[take..];
        }
        Ok(())
    }

    pub(super) fn row<D: BlockDevice>(&mut self, store: &FlatStore<D>, i: usize) -> Result<Row, Error> {
        let mut bytes = [0; ROW_LEN];
        self.read(store, HEADER_LEN + i * ROW_LEN, &mut bytes)?;
        Row::decode(&bytes)
    }

    pub(super) fn checkpoint<D: BlockDevice>(
        &mut self,
        store: &FlatStore<D>,
    ) -> Result<Option<NavigatorCheckpoint>, Error> {
        if self.layout.checkpoint_len == 0 {
            return Ok(None);
        }
        let mut bytes = [0; CHECKPOINT_LEN];
        self.read(store, HEADER_LEN + self.layout.rows * ROW_LEN, &mut bytes)?;
        Ok(NavigatorCheckpoint::decode(&bytes))
    }

    pub(super) fn progress<D: BlockDevice>(&mut self, store: &FlatStore<D>, i: usize) -> Result<TripProgress, Error> {
        let mut bytes = [0; RECORD_LEN];
        self.read(
            store,
            HEADER_LEN + self.layout.rows * ROW_LEN + self.layout.checkpoint_len + i * RECORD_LEN,
            &mut bytes,
        )?;
        TripProgress::decode(&bytes).ok_or(Error::Invalid)
    }
}
