use super::*;

impl Client {
    pub(super) fn control(&mut self, bytes: &[u8], now: u64) -> Result<(), Error> {
        if bytes.len() < HEADER_LEN || bytes.len() > self.ceilings.control() {
            return Err(Error::Protocol);
        }
        let id = RequestId(record_id(bytes, 12));
        let expected = self.pending.as_ref().is_some_and(|p| p.id == id);
        let cancelling = self.cancellation.as_ref().is_some_and(|c| c.id == id || c.transfer == id);
        if !expected && !cancelling {
            return Ok(());
        }
        let (header, response) = decode_response(bytes).map_err(|_| Error::Protocol)?;
        if let Some(cancel) = self.cancellation.as_mut() {
            if header.request == cancel.id {
                if header.opcode != Opcode::Cancel || !matches!(response, Response::Cancel(_)) {
                    return Err(Error::Protocol);
                }
                cancel.answered = true;
                cancel.confirmed = matches!(response, Response::Cancel(true));
                // A lost race can answer the original request successfully; cancellation must
                // not replace an authoritative commit with a cancellation result.
            } else if header.request == cancel.transfer {
                if self.pending.as_ref().is_none_or(|p| header.opcode != opcode(p.request)) {
                    return Err(Error::Protocol);
                }
                if matches!(response, Response::Error(_)) {
                    cancel.transfer_answered = true;
                } else if let (Request::Put(put), Response::Put(result)) =
                    (self.operation.as_ref().unwrap().request, response)
                {
                    let op = self.operation.as_ref().unwrap();
                    if result.id.0 == 0
                        || result.revision.0 == 0
                        || result.payload_len != put.payload_len
                        || result.payload_crc != put.payload_crc
                        || op.received != put.payload_len
                        || op.crc.finalize() != put.payload_crc
                        || put.id.0 != 0 && (result.id != put.id || Some(result.revision) != put.expected.next())
                    {
                        return Err(Error::Protocol);
                    }
                    self.finish(Ok(Outcome::Put(result)));
                    return Ok(());
                } else {
                    return Err(cancel.cause);
                }
            }
            if let Some(cancel) = &self.cancellation {
                if cancel.answered && cancel.transfer_answered {
                    self.finish(Err(cancel.cause));
                }
            }
            return Ok(());
        }
        let pending = self.pending.as_ref().ok_or(Error::Protocol)?;
        if header.opcode != opcode(pending.request) {
            return Err(Error::Protocol);
        }
        self.deadline = now.saturating_add(self.options.timeout_ms);
        if let Response::Error(refusal) = response {
            let paged = matches!(pending.request, Request::List(_))
                && matches!(pending.purpose, Purpose::Run | Purpose::FindCreate);
            if refusal.code == ErrorCode::CatalogChanged && paged {
                return self.restart_list(now);
            }
            return Err(Error::Remote(refusal));
        }
        self.answer(response, now)
    }

    fn answer(&mut self, response: Response<'_>, now: u64) -> Result<(), Error> {
        let pending = self.pending.ok_or(Error::Protocol)?;
        match pending.purpose {
            Purpose::Introduce | Purpose::Identity => {
                let Response::List(page) = response else {
                    return Err(Error::Protocol);
                };
                self.observe_store(page.store)?;
                if pending.purpose == Purpose::Identity {
                    self.reconcile(now)
                } else {
                    self.run(now)
                }
            }
            Purpose::Status => {
                let Response::Status(status) = response else {
                    return Err(Error::Protocol);
                };
                self.reconciled_status(status, now)
            }
            Purpose::Run | Purpose::FindCreate if matches!(pending.request, Request::List(_)) => {
                let Response::List(page) = response else {
                    return Err(Error::Protocol);
                };
                self.page(page, now)
            }
            Purpose::RemoveDuplicate => {
                if !matches!(response, Response::Remove(_)) {
                    return Err(Error::Protocol);
                }
                self.remove_duplicate(now)
            }
            Purpose::Run => {
                let request = pending.request;
                let outcome = match (request, response) {
                    (Request::Status(_), Response::Status(s)) => Outcome::Status(s),
                    (Request::Get(get), Response::Get(mut r)) => {
                        if r.revision.0 == 0 || get.revision.0 != 0 && get.revision != r.revision {
                            return Err(Error::Protocol);
                        }
                        r.id = get.id;
                        self.operation.as_mut().unwrap().answer = Some(r);
                        return self.finish_transfer();
                    }
                    (Request::Put(put), Response::Put(r)) => {
                        if r.id.0 == 0
                            || r.revision.0 == 0
                            || put.id.0 != 0 && (r.id != put.id || Some(r.revision) != put.expected.next())
                            || r.payload_len != put.payload_len
                            || r.payload_crc != put.payload_crc
                        {
                            return Err(Error::Protocol);
                        }
                        self.operation.as_mut().unwrap().answer = Some(r);
                        return self.finish_transfer();
                    }
                    (Request::Remove(_), Response::Remove(sequence)) => Outcome::Remove { sequence: Some(sequence) },
                    (Request::Cancel(_), Response::Cancel(value)) => Outcome::Cancel(value),
                    (Request::Arm(_), Response::Arm { reserve, sequence }) => Outcome::Arm { reserve, sequence },
                    (Request::Format(format), Response::Format(store)) if format.replacement == store => {
                        self.store = Some(store);
                        Outcome::Format(store)
                    }
                    (Request::ArchiveRide(_), Response::ArchiveRide(result)) => Outcome::ArchiveRide(result),
                    _ => return Err(Error::Protocol),
                };
                self.finish(Ok(outcome));
                Ok(())
            }
            _ => Err(Error::Protocol),
        }
    }

    fn page(&mut self, page: ListPage<'_>, now: u64) -> Result<(), Error> {
        self.observe_store(page.store)?;
        let pending = self.pending.as_ref().unwrap();
        let Request::List(list) = pending.request else {
            return Err(Error::Protocol);
        };
        let purpose = pending.purpose;
        let op = self.operation.as_mut().unwrap();
        if op.sequence.is_some_and(|sequence| sequence != page.sequence) {
            return self.restart_list(now);
        }
        op.sequence = Some(page.sequence);
        for entry in page.entries() {
            if list.kind.is_some_and(|kind| kind != entry.kind)
                || op.entries.last().is_some_and(|last| (last.id, last.revision) >= (entry.id, entry.revision))
            {
                return Err(Error::Protocol);
            }
            op.entries.push(entry);
        }
        if page.more {
            let last = op.entries.last().ok_or(Error::Protocol)?;
            let cursor = ListCursor { id: last.id, revision: last.revision, sequence: page.sequence };
            return self.send_request(
                Request::List(ListRequest { kind: list.kind, cursor: Some(cursor) }),
                purpose,
                now,
            );
        }
        if purpose == Purpose::FindCreate {
            let Request::Put(put) = op.request else {
                return Err(Error::Protocol);
            };
            op.entries.retain(|entry| {
                entry.kind == put.kind
                    && !entry.flags.has(EntryFlags::RETAINED)
                    && entry.payload_len == put.payload_len
                    && entry.payload_crc == put.payload_crc
                    && entry.name == put.name
            });
            let Some(newest) = op.entries.pop() else {
                return self.run(now);
            };
            op.recovered = Some(TransferResponse {
                id: newest.id,
                revision: newest.revision,
                payload_len: newest.payload_len,
                payload_crc: newest.payload_crc,
            });
            return self.remove_duplicate(now);
        }
        let entries = core::mem::take(&mut op.entries);
        self.finish(Ok(Outcome::Catalog { store: page.store, sequence: page.sequence, entries }));
        Ok(())
    }

    fn restart_list(&mut self, now: u64) -> Result<(), Error> {
        let op = self.operation.as_mut().unwrap();
        if op.restarts >= self.options.list_restarts {
            return Err(Error::CatalogChanged);
        }
        op.restarts += 1;
        op.entries.clear();
        op.sequence = None;
        let p = self.pending.as_ref().unwrap();
        let Request::List(list) = p.request else {
            return Err(Error::Protocol);
        };
        self.send_request(Request::List(ListRequest { kind: list.kind, cursor: None }), p.purpose, now)
    }

    pub(super) fn link_lost(&mut self, now: u64) -> Result<(), Error> {
        self.connected = false;
        self.store = None;
        let op = self.operation.as_mut().unwrap();
        if self.cancellation.is_some() {
            return Err(Error::LinkLost);
        }
        if op.reconnects >= self.options.reconnect_attempts {
            return Err(Error::LinkLost);
        }
        op.reconnects += 1;
        op.entries.clear();
        op.sequence = None;
        op.reset_transfer();
        self.pending = None;
        self.write = None;
        self.actions.clear();
        if matches!(op.request, Request::Get(_)) {
            self.actions.push_back(Action::ResetSink);
        }
        self.restoring = true;
        self.actions.push_back(Action::ResetChannels);
        self.actions.push_back(Action::Restore);
        self.deadline = now.saturating_add(self.options.timeout_ms);
        Ok(())
    }

    fn observe_store(&mut self, current: StoreId) -> Result<(), Error> {
        let op = self.operation.as_ref().unwrap();
        let own_format = matches!(op.request, Request::Format(f) if f.replacement == current);
        if let Some(previous) = op.expected_store {
            if previous != current && !own_format {
                return Err(Error::StoreChanged { previous, current });
            }
        }
        if let Some(previous) = self.store {
            if previous != current {
                return Err(Error::StoreChanged { previous, current });
            }
        }
        self.store = Some(current);
        self.operation.as_mut().unwrap().expected_store.get_or_insert(current);
        Ok(())
    }

    fn reconcile(&mut self, now: u64) -> Result<(), Error> {
        let op = self.operation.as_ref().unwrap();
        if !op.started {
            return self.run(now);
        }
        let request = match op.request {
            Request::Put(put) if put.id.0 != 0 => {
                Request::Status(StatusRequest { id: put.id, revision: put.expected.next().ok_or(Error::NotCommitted)? })
            }
            Request::Put(put) => {
                return self.send_request(
                    Request::List(ListRequest { kind: Some(put.kind), cursor: None }),
                    Purpose::FindCreate,
                    now,
                )
            }
            Request::Remove(remove) => Request::Status(StatusRequest { id: remove.id, revision: remove.expected }),
            Request::Arm(_) => return Err(Error::OutcomeUnknown),
            Request::Format(format) => {
                if self.store == Some(format.replacement) {
                    self.finish(Ok(Outcome::Format(format.replacement)));
                    return Ok(());
                }
                return self.run(now);
            }
            _ => return self.run(now),
        };
        self.send_request(request, Purpose::Status, now)
    }

    fn reconciled_status(&mut self, status: StatusResponse, now: u64) -> Result<(), Error> {
        let request = self.operation.as_ref().unwrap().request;
        match request {
            Request::Put(put) if status.state == ObjectState::Committed => {
                if Some(status.revision) != put.expected.next()
                    || status.payload_len != put.payload_len
                    || status.payload_crc != put.payload_crc
                {
                    return Err(Error::NotCommitted);
                }
                self.finish(Ok(Outcome::Put(TransferResponse {
                    id: put.id,
                    revision: status.revision,
                    payload_len: status.payload_len,
                    payload_crc: status.payload_crc,
                })));
                Ok(())
            }
            Request::Put(_) => self.run(now),
            Request::Remove(_) if status.state == ObjectState::Absent => {
                self.finish(Ok(Outcome::Remove { sequence: None }));
                Ok(())
            }
            Request::Remove(_) if status.state == ObjectState::Committed => self.run(now),
            _ => Err(Error::NotCommitted),
        }
    }

    fn remove_duplicate(&mut self, now: u64) -> Result<(), Error> {
        let op = self.operation.as_mut().unwrap();
        if !self.options.remove_duplicate_creates {
            op.entries.clear();
        }
        if let Some(entry) = op.entries.pop() {
            self.send_request(
                Request::Remove(RemoveRequest { id: entry.id, expected: entry.revision }),
                Purpose::RemoveDuplicate,
                now,
            )
        } else {
            let recovered = op.recovered.take().ok_or(Error::Protocol)?;
            self.finish(Ok(Outcome::Put(recovered)));
            Ok(())
        }
    }

    pub(super) fn cancel(&mut self, cause: Error, now: u64) -> Result<(), Error> {
        if self.cancellation.is_some() {
            return Ok(());
        }
        let operation = self.operation.as_ref().unwrap();
        if let (Request::Put(_), Some(answer)) = (operation.request, operation.answer) {
            if operation.received == answer.payload_len && operation.crc.finalize() == answer.payload_crc {
                self.finish(Ok(Outcome::Put(answer)));
                return Ok(());
            }
        }
        let Some(pending) = &self.pending else {
            return Err(cause);
        };
        if !self.operation.as_ref().unwrap().transfer() || pending.purpose != Purpose::Run {
            return Err(cause);
        }
        let transfer = pending.id;
        let id = self.id()?;
        let mut bytes = vec![0; MAX_REQUEST_LEN];
        let len = encode_request(&mut bytes, id, Request::Cancel(CancelRequest { transfer })).ok_or(Error::Protocol)?;
        bytes.truncate(len);
        self.actions.clear();
        self.send(Channel::Control, bytes, Write::Control)?;
        let transfer_answered = self.operation.as_ref().unwrap().answer.is_some();
        self.cancellation =
            Some(Cancellation { id, transfer, cause, answered: false, confirmed: false, transfer_answered });
        self.deadline = now.saturating_add(self.options.cancel_timeout_ms);
        Ok(())
    }
}

pub(super) fn opcode(request: Request) -> Opcode {
    match request {
        Request::List(_) => Opcode::List,
        Request::Status(_) => Opcode::Status,
        Request::Get(_) => Opcode::Get,
        Request::Put(_) => Opcode::Put,
        Request::Remove(_) => Opcode::Remove,
        Request::Cancel(_) => Opcode::Cancel,
        Request::Arm(_) => Opcode::Arm,
        Request::Format(_) => Opcode::Format,
        Request::ArchiveRide(_) => Opcode::ArchiveRide,
    }
}
