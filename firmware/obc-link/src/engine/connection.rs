//! The connection state machine of §5.2, plus the frame-limit derivation of §14.0.
//!
//! "A control connection has exactly two states, and Hello is the only transition between them."
//! Before negotiation the only acceptable opcode is Hello and a device "MUST NOT admit, claim, or
//! resume anything on an unnegotiated connection". After it, Hello repeats only for paging and only
//! with byte-identical negotiation fields, and "At most one control request may be outstanding per
//! direction on each link."
//!
//! The negotiated limits are derived here rather than advertised: §14.0 makes each binding's record
//! ceiling a physical fact, the advertised maxima are clamped to it, and a ceiling below the
//! protocol minimum fails Hello closed rather than inventing a reduced dialect.

use crate::error::{detail, ErrorBody, ErrorCategory, Owner, RetryGuidance};
use crate::frame::{Opcode, MIN_CONTROL_FRAME, MIN_STREAM_FRAME};
use crate::hello::{Hello, LinkKind};
use crate::ids::RequestId;

use super::session::LinkContext;

/// The record ceilings a binding's physical framing imposes (§14.0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkCeilings {
    /// The largest control record this link can carry, header included.
    pub control_frame: u16,
    /// The largest stream record it can carry, header included. On BLE this is the CoC SDU.
    pub stream_frame: u16,
}

/// What a negotiated connection agreed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Negotiated {
    /// The Hello whose negotiation fields every later Hello must repeat byte for byte.
    pub hello: Hello,
    /// `min(client maximum, device maximum, transport ceiling)`.
    pub control_frame: u16,
    /// The same, for the stream channel.
    pub stream_frame: u16,
}

/// Why a control request cannot be admitted on this connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionRefusal {
    /// Any opcode but Hello before negotiation. "creates no state".
    NotNegotiated,
    /// A Hello that changes a negotiation field. There is no renegotiation within a connection.
    Renegotiation,
    /// A second request while one is outstanding. The one in flight is not disturbed.
    Outstanding,
    /// The client's major range does not include this document's.
    UnsupportedMajor,
    /// The link cannot carry the 192-byte control minimum.
    ControlFrameTooSmall,
    /// The link cannot carry the 64-byte stream minimum.
    StreamFrameTooSmall,
}

impl ConnectionRefusal {
    /// The body this refusal answers with, on a connection of `link_kind`.
    pub fn body(self, link_kind: LinkKind) -> ErrorBody<'static> {
        match self {
            ConnectionRefusal::NotNegotiated | ConnectionRefusal::Renegotiation => ErrorBody::bare(
                ErrorCategory::INVALID_DESCRIPTOR,
                detail::descriptor::INVALID_COMBINATION,
                RetryGuidance::REJECT_PERMANENTLY,
            ),
            ConnectionRefusal::Outstanding => {
                let mut body = ErrorBody::bare(
                    ErrorCategory::BUSY,
                    detail::busy::NORMAL_OPERATION_CLAIMS,
                    RetryGuidance::RETRY_AFTER_OWNER_RELEASE,
                );
                // §5.2: "owner set to this connection's own link kind".
                body.owner = Owner::from_u8(link_kind.to_u8());
                body
            }
            ConnectionRefusal::UnsupportedMajor => ErrorBody::bare(
                ErrorCategory::INCOMPATIBLE_VERSION,
                detail::version::UNSUPPORTED_MAJOR,
                RetryGuidance::RETRY_AFTER_USER_ACTION,
            ),
            ConnectionRefusal::ControlFrameTooSmall => ErrorBody::bare(
                ErrorCategory::RESOURCE_LIMIT,
                detail::resource::MINIMUM_CONTROL_FRAME,
                RetryGuidance::RETRY_AFTER_USER_ACTION,
            ),
            ConnectionRefusal::StreamFrameTooSmall => ErrorBody::bare(
                ErrorCategory::RESOURCE_LIMIT,
                detail::resource::MINIMUM_STREAM_FRAME,
                RetryGuidance::RETRY_AFTER_USER_ACTION,
            ),
        }
    }
}

/// One link's control connection.
#[derive(Debug, Clone, Copy)]
pub struct Connection {
    context: Option<LinkContext>,
    ceilings: LinkCeilings,
    negotiated: Option<Negotiated>,
    outstanding: Option<RequestId>,
}

impl Connection {
    /// A closed connection.
    pub const fn closed() -> Self {
        Connection {
            context: None,
            ceilings: LinkCeilings { control_frame: 0, stream_frame: 0 },
            negotiated: None,
            outstanding: None,
        }
    }

    /// Opens a new connection generation, discarding everything the old one negotiated.
    pub fn open(&mut self, context: LinkContext, ceilings: LinkCeilings) {
        *self = Connection { context: Some(context), ceilings, negotiated: None, outstanding: None };
    }

    /// Closes this connection.
    pub fn close(&mut self) {
        *self = Connection::closed();
    }

    /// The connection's identity, when one is open.
    pub const fn context(&self) -> Option<LinkContext> {
        self.context
    }

    /// What this connection negotiated, when it has.
    pub const fn negotiated(&self) -> Option<Negotiated> {
        self.negotiated
    }

    /// True once Hello has been answered with Capabilities.
    pub const fn is_negotiated(&self) -> bool {
        self.negotiated.is_some()
    }

    /// The RequestId in flight, if any.
    pub const fn outstanding(&self) -> Option<RequestId> {
        self.outstanding
    }

    /// Admits a request: §5.2's two-state rule, then its one-outstanding rule.
    ///
    /// The order follows §12's validation precedence — descriptor before owner/resources — so a
    /// query sent before Hello is `invalidDescriptor/invalidCombination` whether or not something
    /// else is in flight.
    pub fn admit(&mut self, opcode: Opcode, request_id: RequestId) -> Result<(), ConnectionRefusal> {
        if !self.is_negotiated() && opcode != Opcode::Hello {
            return Err(ConnectionRefusal::NotNegotiated);
        }
        if self.outstanding.is_some() {
            return Err(ConnectionRefusal::Outstanding);
        }
        self.outstanding = Some(request_id);
        Ok(())
    }

    /// Releases the outstanding slot once this connection's response has been handed to the link.
    pub fn complete(&mut self) {
        self.outstanding = None;
    }

    /// Negotiates, or checks that a repeated Hello is only asking for another page.
    ///
    /// `device_control_frame` and `device_stream_frame` are what the device itself can serve; the
    /// negotiated value is the smaller of the two advertised maxima, clamped to the link's ceiling.
    pub fn negotiate(
        &mut self,
        hello: &Hello,
        device_control_frame: u16,
        device_stream_frame: u16,
    ) -> Result<Negotiated, ConnectionRefusal> {
        if let Some(negotiated) = self.negotiated {
            // §5.2: "A repeated Hello MUST carry byte-identical negotiation fields ... and may
            // differ only in page kind and page index."
            return if negotiated.hello.is_same_negotiation(hello) {
                Ok(negotiated)
            } else {
                Err(ConnectionRefusal::Renegotiation)
            };
        }
        if hello.minimum_major > crate::WIRE_MAJOR || hello.maximum_major < crate::WIRE_MAJOR {
            return Err(ConnectionRefusal::UnsupportedMajor);
        }
        let control_frame = hello.client_max_control_frame.min(device_control_frame).min(self.ceilings.control_frame);
        if usize::from(control_frame) < MIN_CONTROL_FRAME {
            return Err(ConnectionRefusal::ControlFrameTooSmall);
        }
        let stream_frame = hello.client_max_stream_frame.min(device_stream_frame).min(self.ceilings.stream_frame);
        if usize::from(stream_frame) < MIN_STREAM_FRAME {
            return Err(ConnectionRefusal::StreamFrameTooSmall);
        }
        let negotiated = Negotiated { hello: *hello, control_frame, stream_frame };
        self.negotiated = Some(negotiated);
        Ok(negotiated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::session::PrincipalScope;
    use crate::hello::PageKind;

    fn ceilings() -> LinkCeilings {
        LinkCeilings { control_frame: 244, stream_frame: 1_024 }
    }

    fn context() -> LinkContext {
        LinkContext::new(LinkKind::Ble, PrincipalScope::new([7; 16]), 1)
    }

    fn hello() -> Hello {
        Hello {
            minimum_major: 3,
            maximum_major: 3,
            client_max_control_frame: 512,
            client_max_stream_frame: 4_096,
            client_feature_flags: 0,
            page_kind: PageKind::Resources,
            page_index: 0,
        }
    }

    fn open() -> Connection {
        let mut connection = Connection::closed();
        connection.open(context(), ceilings());
        connection
    }

    #[test]
    fn nothing_but_hello_is_admitted_before_negotiation() {
        let mut connection = open();
        let request_id = RequestId::new(1).unwrap();
        assert_eq!(connection.admit(Opcode::QueryCatalog, request_id), Err(ConnectionRefusal::NotNegotiated));
        assert_eq!(connection.admit(Opcode::StartUpload, request_id), Err(ConnectionRefusal::NotNegotiated));
        assert_eq!(connection.outstanding(), None, "a refusal before negotiation creates no state");
        assert!(connection.admit(Opcode::Hello, request_id).is_ok());
    }

    #[test]
    fn a_repeated_hello_may_change_only_the_page_fields() {
        let mut connection = open();
        connection.negotiate(&hello(), 244, 1_024).unwrap();

        let paging = Hello { page_kind: PageKind::Subjects, page_index: 2, ..hello() };
        assert!(connection.negotiate(&paging, 244, 1_024).is_ok());

        let renegotiating = Hello { client_max_stream_frame: 512, ..hello() };
        assert_eq!(connection.negotiate(&renegotiating, 244, 1_024), Err(ConnectionRefusal::Renegotiation));
    }

    #[test]
    fn the_negotiated_limits_are_the_smallest_of_client_device_and_transport() {
        let mut connection = open();
        let negotiated = connection.negotiate(&hello(), 512, 4_096).unwrap();
        assert_eq!((negotiated.control_frame, negotiated.stream_frame), (244, 1_024));

        let mut connection = open();
        let client = Hello { client_max_control_frame: 200, client_max_stream_frame: 128, ..hello() };
        let negotiated = connection.negotiate(&client, 512, 4_096).unwrap();
        assert_eq!((negotiated.control_frame, negotiated.stream_frame), (200, 128));
    }

    #[test]
    fn a_link_below_a_protocol_minimum_fails_hello_closed() {
        let mut connection = Connection::closed();
        connection.open(context(), LinkCeilings { control_frame: 191, stream_frame: 1_024 });
        assert_eq!(connection.negotiate(&hello(), 512, 4_096), Err(ConnectionRefusal::ControlFrameTooSmall));
        assert!(!connection.is_negotiated(), "nothing is admitted on that connection");

        let mut connection = Connection::closed();
        connection.open(context(), LinkCeilings { control_frame: 244, stream_frame: 63 });
        assert_eq!(connection.negotiate(&hello(), 512, 4_096), Err(ConnectionRefusal::StreamFrameTooSmall));

        let mut connection = open();
        let legacy = Hello { minimum_major: 1, maximum_major: 2, ..hello() };
        assert_eq!(connection.negotiate(&legacy, 512, 4_096), Err(ConnectionRefusal::UnsupportedMajor));
    }

    #[test]
    fn a_second_request_is_refused_without_disturbing_the_one_in_flight() {
        let mut connection = open();
        connection.negotiate(&hello(), 244, 1_024).unwrap();
        let first = RequestId::new(4).unwrap();
        connection.admit(Opcode::QueryCatalog, first).unwrap();
        assert_eq!(
            connection.admit(Opcode::GetDeviceStatus, RequestId::new(5).unwrap()),
            Err(ConnectionRefusal::Outstanding)
        );
        assert_eq!(connection.outstanding(), Some(first));
        assert_eq!(ConnectionRefusal::Outstanding.body(LinkKind::Usb).owner, Owner::USB);
        connection.complete();
        assert!(connection.admit(Opcode::GetDeviceStatus, RequestId::new(5).unwrap()).is_ok());
    }

    #[test]
    fn reopening_a_connection_forgets_the_old_negotiation() {
        let mut connection = open();
        connection.negotiate(&hello(), 244, 1_024).unwrap();
        connection.open(LinkContext { generation: 2, ..context() }, ceilings());
        assert!(!connection.is_negotiated());
        assert_eq!(connection.outstanding(), None);
    }
}
