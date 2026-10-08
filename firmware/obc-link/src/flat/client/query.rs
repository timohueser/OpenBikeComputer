use super::*;
use crate::flat::ObjectKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryOutcome {
    Page { store: StoreId, sequence: u64, more: bool, entries: Vec<EntryMeta> },
    Catalog { store: StoreId, sequence: u64, entries: Vec<EntryMeta> },
    Status(StatusResponse),
    Cancel(bool),
}

#[derive(Default)]
struct Catalog {
    entries: Vec<EntryMeta>,
    restarts: u8,
}

pub(super) struct Query {
    handle: QueryId,
    id: RequestId,
    request: Request,
    expected_store: Option<StoreId>,
    introducing: bool,
    catalog: Option<Catalog>,
    pub(super) token: Option<u64>,
    pub(super) deadline: u64,
}

impl Client {
    /// Submit one LIST page, STATUS or CANCEL. CANCEL reports the peer's answer and settles
    /// a matching primary transfer through the same bilateral cancellation path as Event::Cancel.
    /// LIST cursors retain their snapshot sequence; a refused page is not silently restarted.
    pub fn query(&mut self, request: Request, expected_store: Option<StoreId>, now: u64) -> Result<QueryId, Error> {
        self.start_query(request, expected_store, now, None)
    }

    /// Collect a complete snapshot beside the primary operation. Catalog changes restart at
    /// the first page, up to the same restart limit as the primary catalogue request.
    pub fn query_catalog(
        &mut self,
        kind: Option<ObjectKind>,
        expected_store: Option<StoreId>,
        now: u64,
    ) -> Result<QueryId, Error> {
        self.start_query(
            Request::List(ListRequest { kind, cursor: None }),
            expected_store,
            now,
            Some(Catalog::default()),
        )
    }

    /// Complete one query waiter locally. A CANCEL targeting the primary keeps its cancellation
    /// intent: the primary takes ownership of its queued send and in-flight completion token.
    pub fn cancel_query(&mut self, handle: QueryId) -> bool {
        let Some(index) = self.queries.iter().position(|query| query.handle == handle) else {
            return false;
        };
        let query = &self.queries[index];
        let primary_cancel = self.cancellation.as_ref().is_some_and(|cancel| cancel.id == query.id);
        let token = query.token;
        let queued = if primary_cancel {
            self.actions
                .iter()
                .position(|action| matches!(action, Action::Send { token: sent, .. } if Some(*sent) == token))
                .map(|at| (at, self.actions.remove(at).unwrap()))
        } else {
            None
        };
        self.finish_query(index, Err(Error::Cancelled));
        if primary_cancel {
            if let Some(token) = token {
                self.write = Some((token, Write::Control));
            }
            if let Some((at, action)) = queued {
                self.actions.insert(at, action);
            }
        }
        true
    }

    fn start_query(
        &mut self,
        request: Request,
        expected_store: Option<StoreId>,
        now: u64,
        catalog: Option<Catalog>,
    ) -> Result<QueryId, Error> {
        if !matches!(request, Request::List(_) | Request::Status(_) | Request::Cancel(_)) {
            return Err(Error::InvalidInput);
        }
        if !self.connected || self.restoring {
            return Err(Error::LinkLost);
        }
        if let (Some(previous), Some(current)) = (expected_store, self.store) {
            if previous != current {
                return Err(Error::StoreChanged { previous, current });
            }
        }
        let introducing = self.store.is_none() && matches!(request, Request::Status(_) | Request::Cancel(_));
        let sent = if introducing { Request::List(ListRequest { kind: None, cursor: None }) } else { request };
        let id = self.id()?;
        let token = self.send_query(sent, id)?;
        let handle = QueryId(id.0 as u64);
        self.queries.push(Query {
            handle,
            id,
            request,
            expected_store: expected_store.or(self.store),
            introducing,
            catalog,
            token: Some(token),
            deadline: now.saturating_add(self.options.timeout_ms),
        });
        if let Request::Cancel(cancel) = request {
            if self.active_transfer_id() == Some(cancel.transfer) && self.cancellation.is_none() {
                self.begin_query_cancel(id, cancel.transfer, now);
            }
        }
        Ok(handle)
    }

    pub fn next_query_result(&mut self) -> Option<(QueryId, Result<QueryOutcome, Error>)> {
        self.query_results.pop_front()
    }

    fn send_query(&mut self, request: Request, id: RequestId) -> Result<u64, Error> {
        let mut record = vec![0; MAX_REQUEST_LEN];
        let len = encode_request(&mut record, id, request).ok_or(Error::InvalidInput)?;
        if len > self.ceilings.control() {
            return Err(Error::InvalidInput);
        }
        record.truncate(len);
        let token = self.token()?;
        self.actions.push_back(Action::Send { token, channel: Channel::Control, record });
        Ok(token)
    }

    pub(super) fn query_control(&mut self, record: &[u8], now: u64) -> bool {
        if record.len() < HEADER_LEN {
            return false;
        }
        let id = RequestId(record_id(record, 12));
        let Some(index) = self.queries.iter().position(|query| query.id == id) else {
            return false;
        };
        let result = if record.len() > self.ceilings.control() {
            Err(Error::Protocol)
        } else {
            decode_response(record).map_err(|_| Error::Protocol).and_then(|(header, response)| {
                let query = &self.queries[index];
                let expected = if query.introducing { Opcode::List } else { control::opcode(query.request) };
                if header.opcode != expected {
                    Err(Error::Protocol)
                } else {
                    self.query_answer(index, response, now)
                }
            })
        };
        match result {
            Ok(Some(outcome)) => {
                let cancelled = matches!(outcome, QueryOutcome::Cancel(_));
                self.finish_query(index, Ok(outcome));
                if cancelled {
                    self.settle_cancellation();
                }
            }
            Ok(None) => {}
            Err(error) => self.finish_query(index, Err(error)),
        }
        true
    }

    fn query_answer(&mut self, index: usize, response: Response<'_>, now: u64) -> Result<Option<QueryOutcome>, Error> {
        if let Response::Error(refusal) = response {
            if refusal.code == ErrorCode::CatalogChanged && self.queries[index].catalog.is_some() {
                self.restart_catalog(index, now)?;
                return Ok(None);
            }
            return Err(Error::Remote(refusal));
        }
        if let Response::List(page) = response {
            let query = &self.queries[index];
            let previous = query.expected_store.or(self.store);
            if let Some(previous) = previous {
                if previous != page.store {
                    return Err(Error::StoreChanged { previous, current: page.store });
                }
            }
            if let Some(previous) = self.store {
                if previous != page.store {
                    return Err(Error::StoreChanged { previous, current: page.store });
                }
            }
            if let Some(op) = &mut self.operation {
                if let Some(previous) = op.expected_store {
                    if previous != page.store {
                        return Err(Error::StoreChanged { previous, current: page.store });
                    }
                }
                op.expected_store.get_or_insert(page.store);
            }
            self.store = Some(page.store);
            if query.introducing {
                let request = query.request;
                self.queries[index].expected_store = Some(page.store);
                self.advance_query(index, request, now)?;
                return Ok(None);
            }
            let Request::List(list) = query.request else {
                return Err(Error::Protocol);
            };
            if list.cursor.is_some_and(|cursor| cursor.sequence != page.sequence) {
                if query.catalog.is_some() {
                    self.restart_catalog(index, now)?;
                    return Ok(None);
                }
                return Err(Error::CatalogChanged);
            }
            let entries: Vec<_> = page.entries().collect();
            if entries.iter().any(|entry| list.kind.is_some_and(|kind| kind != entry.kind))
                || entries.windows(2).any(|pair| (pair[0].id, pair[0].revision) >= (pair[1].id, pair[1].revision))
                || list.cursor.is_some_and(|cursor| {
                    entries.first().is_some_and(|entry| (entry.id, entry.revision) <= (cursor.id, cursor.revision))
                })
                || page.more && entries.is_empty()
            {
                return Err(Error::Protocol);
            }
            if let Some(catalog) = &mut self.queries[index].catalog {
                catalog.entries.extend(entries);
                if page.more {
                    let last = catalog.entries.last().ok_or(Error::Protocol)?;
                    let cursor = ListCursor { id: last.id, revision: last.revision, sequence: page.sequence };
                    self.advance_query(
                        index,
                        Request::List(ListRequest { kind: list.kind, cursor: Some(cursor) }),
                        now,
                    )?;
                    return Ok(None);
                }
                return Ok(Some(QueryOutcome::Catalog {
                    store: page.store,
                    sequence: page.sequence,
                    entries: core::mem::take(&mut catalog.entries),
                }));
            }
            return Ok(Some(QueryOutcome::Page {
                store: page.store,
                sequence: page.sequence,
                more: page.more,
                entries,
            }));
        }
        match (self.queries[index].request, response) {
            (Request::Status(_), Response::Status(status)) => Ok(Some(QueryOutcome::Status(status))),
            (Request::Cancel(_), Response::Cancel(confirmed)) => {
                if let Some(cancel) = &mut self.cancellation {
                    if cancel.id == self.queries[index].id {
                        cancel.answered = true;
                        cancel.confirmed = confirmed;
                    }
                }
                Ok(Some(QueryOutcome::Cancel(confirmed)))
            }
            _ => Err(Error::Protocol),
        }
    }

    fn advance_query(&mut self, index: usize, request: Request, now: u64) -> Result<(), Error> {
        let id = self.id()?;
        let token = self.send_query(request, id)?;
        let query = &mut self.queries[index];
        query.id = id;
        query.request = request;
        query.token = Some(token);
        query.introducing = false;
        query.deadline = now.saturating_add(self.options.timeout_ms);
        Ok(())
    }

    fn restart_catalog(&mut self, index: usize, now: u64) -> Result<(), Error> {
        let Request::List(list) = self.queries[index].request else {
            return Err(Error::Protocol);
        };
        let catalog = self.queries[index].catalog.as_mut().ok_or(Error::Protocol)?;
        if catalog.restarts >= self.options.list_restarts {
            return Err(Error::CatalogChanged);
        }
        catalog.restarts += 1;
        catalog.entries.clear();
        self.advance_query(index, Request::List(ListRequest { kind: list.kind, cursor: None }), now)
    }

    pub(super) fn query_written(&mut self, token: u64, now: u64) -> bool {
        let Some(query) = self.queries.iter_mut().find(|query| query.token == Some(token)) else {
            return false;
        };
        query.token = None;
        query.deadline = now.saturating_add(self.options.timeout_ms);
        true
    }

    pub(super) fn query_failed(&mut self, token: u64) -> bool {
        let Some(index) = self.queries.iter().position(|query| query.token == Some(token)) else {
            return false;
        };
        self.finish_query(index, Err(Error::Io));
        true
    }

    pub(super) fn expire_queries(&mut self, now: u64) {
        let mut index = 0;
        while index < self.queries.len() {
            if now >= self.queries[index].deadline {
                self.finish_query(index, Err(Error::Timeout));
            } else {
                index += 1;
            }
        }
    }

    fn finish_query(&mut self, index: usize, result: Result<QueryOutcome, Error>) {
        let query = self.queries.remove(index);
        self.actions.retain(|action| !matches!(action, Action::Send { token, .. } if Some(*token) == query.token));
        self.query_results.push_back((query.handle, result));
    }

    pub(super) fn fail_queries(&mut self, error: Error) {
        while !self.queries.is_empty() {
            self.finish_query(0, Err(error));
        }
    }
}
