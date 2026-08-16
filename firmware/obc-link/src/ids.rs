//! The mechanically distinct identities of `Device_Object_System_v2.md` §3.
//!
//! Every one of these is its own type with no implicit conversion to another and no arithmetic.
//! That is not stylistic: five of them are `u64` and three are 16 opaque bytes, so the compiler is
//! the only thing standing between "the repository revision" and "the physical generation" once
//! their representations are erased. Construction is always explicit ([`new`](StoreId::new) /
//! `get`), and no `From`, `Into`, `Add`, `Sub`, `Step`, or `PartialOrd`-across-types impl exists to
//! smuggle one into another's place.
//!
//! Ordering is provided only where the contract gives the value an order: [`Revision`] is a
//! monotonic compare-and-swap token and [`LogicalObjectId`] orders catalog entries, so both are
//! `Ord`. [`GenerationId`] and [`WeatherRequestId`] are monotonic domain counters and are `Ord`
//! too. The opaque 16-byte identities are `Eq` but not `Ord`: nothing in the contract ever sorts
//! them, and an accidental sort would invent an order the device does not have.
//!
//! ## Diagnostic spelling
//!
//! §3: "Their diagnostic spelling is 32 lower-case hexadecimal digits in wire-byte order. UUID
//! field reordering is forbidden." [`Display`](core::fmt::Display) on the 16-byte identities emits
//! exactly that, and [`Debug`](core::fmt::Debug) names the type around it so a log line says which
//! identity it is looking at. The integer identities spell as zero-padded lower-case hex of their
//! own width for the same reason: a decimal `4` and a decimal `4` from two different registries
//! look identical in a log, and a typed hex prefix does not.

use core::fmt;

/// Formats `bytes` as lower-case hex in wire-byte order.
fn write_hex(f: &mut fmt::Formatter<'_>, bytes: &[u8]) -> fmt::Result {
    for byte in bytes {
        write!(f, "{byte:02x}")?;
    }
    Ok(())
}

/// Declares one 16-opaque-byte identity.
macro_rules! opaque16 {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name([u8; 16]);

        impl $name {
            /// The all-zero value. Whether zero is legal is the containing message's rule, never
            /// this type's: the contract has no sentinel identities.
            pub const ZERO: Self = Self([0u8; 16]);

            /// Wraps 16 wire bytes verbatim. No field reordering happens here or anywhere.
            pub const fn new(bytes: [u8; 16]) -> Self {
                Self(bytes)
            }

            /// The 16 wire bytes, in wire order.
            pub const fn to_bytes(self) -> [u8; 16] {
                self.0
            }

            /// The 16 wire bytes, borrowed.
            pub const fn as_bytes(&self) -> &[u8; 16] {
                &self.0
            }

            /// True when every byte is zero.
            pub fn is_zero(self) -> bool {
                self.0 == [0u8; 16]
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write_hex(f, &self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "("))?;
                write_hex(f, &self.0)?;
                write!(f, ")")
            }
        }
    };
}

/// Declares one `u64` identity.
///
/// All four are `Ord`, and each for a stated reason: `Revision` is a monotonic compare-and-swap
/// token, `LogicalObjectId` orders catalog entries, and `GenerationId` and `WeatherRequestId` are
/// monotonic domain counters. The 16-byte identities deliberately are not.
macro_rules! opaque64 {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u64);

        impl $name {
            /// The value zero. §3 is explicit that zero is a value and never absence.
            pub const ZERO: Self = Self(0);

            /// Wraps a `u64` from the wire or from the repository that reported it.
            pub const fn new(value: u64) -> Self {
                Self(value)
            }

            /// The underlying `u64`, for encoding it back onto the wire.
            pub const fn get(self) -> u64 {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{:016x}", self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({:016x})"), self.0)
            }
        }
    };
}

/// Declares one nonzero `u32` correlation/capability identity.
macro_rules! nonzero32 {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name(core::num::NonZeroU32);

        impl $name {
            /// Wraps a wire value, rejecting zero. Zero is not a value of this type: the contract
            /// makes it structurally illegal, so it cannot be constructed and then checked later.
            pub const fn new(value: u32) -> Option<Self> {
                match core::num::NonZeroU32::new(value) {
                    Some(value) => Some(Self(value)),
                    None => None,
                }
            }

            /// The underlying nonzero `u32`.
            pub const fn get(self) -> u32 {
                self.0.get()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{:08x}", self.0.get())
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({:08x})"), self.0.get())
            }
        }
    };
}

opaque16! {
    /// 128-bit store identity, born with the first valid OBC2 checkpoint.
    ///
    /// A reformat or a card replacement creates a new value, which is what makes work from a
    /// removed card unable to attach to its replacement (`Device_Object_System_v2.md` §3).
    StoreId
}

opaque16! {
    /// 128-bit idempotency key, chosen by the producer *before* the intent claim.
    ///
    /// It is the lookup key for the claim and for the retained terminal result, and it is
    /// deliberately not part of the canonical intent digest it keys (`Device_Object_Protocol_v3.md`
    /// §11).
    OperationId
}

opaque16! {
    /// 16 opaque bytes the device draws at random when it seals a draft part.
    ///
    /// `Device_Object_Registries_v2.md` §2: it "conveys no authority", is not a `GenerationId`,
    /// content digest, filename, or globally addressable ID, "and it is not derived from any of
    /// them". There is deliberately nothing on this type to decode it with.
    DraftPartRef
}

opaque64! {
    /// Opaque logical identity inside one `StoreId` and `ObjectKind`. No sentinel band exists.
    LogicalObjectId
}

opaque64! {
    /// Monotonic repository/object compare-and-swap token.
    ///
    /// The token to feed back into an `expected Revision` field is always the one the repository
    /// last reported for *that entry* — never the repository revision an acceptance reported as a
    /// diagnostic snapshot (`Device_Object_Protocol_v3.md` §6.1).
    Revision
}

opaque64! {
    /// Store-private immutable payload identity.
    ///
    /// It exists in this crate only so that the type is nameable and mechanically distinct: it is
    /// never exposed as logical identity and no wire message in v3.0 carries one.
    GenerationId
}

opaque64! {
    /// Durable weather-domain request identity. Never a control [`RequestId`], and never the
    /// weather singleton's [`LogicalObjectId`] (`Device_Object_Registries_v2.md` §3).
    WeatherRequestId
}

nonzero32! {
    /// Ephemeral stream capability, scoped to one link kind, principal scope, and connection
    /// generation. Only the transfer coordinator issues or revokes one.
    SessionId
}

nonzero32! {
    /// Control request/response correlation only. It is neither a [`SessionId`] nor an
    /// [`OperationId`], and a zero value is unanswerable (`Device_Object_Protocol_v3.md` §2).
    RequestId
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::format;

    #[test]
    fn sixteen_byte_identities_spell_as_lower_case_hex_in_wire_order() {
        let bytes = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32, 0x10];
        assert_eq!(format!("{}", StoreId::new(bytes)), "0123456789abcdeffedcba9876543210");
        assert_eq!(format!("{}", OperationId::new(bytes)), "0123456789abcdeffedcba9876543210");
        assert_eq!(format!("{:?}", DraftPartRef::new(bytes)), "DraftPartRef(0123456789abcdeffedcba9876543210)");
    }

    #[test]
    fn integer_identities_spell_at_their_own_width() {
        assert_eq!(format!("{}", LogicalObjectId::new(0x2a)), "000000000000002a");
        assert_eq!(format!("{:?}", Revision::new(1)), "Revision(0000000000000001)");
        assert_eq!(format!("{}", SessionId::new(7).unwrap()), "00000007");
    }

    #[test]
    fn nonzero_identities_reject_zero_at_construction() {
        assert!(SessionId::new(0).is_none());
        assert!(RequestId::new(0).is_none());
        assert!(SessionId::new(1).is_some());
    }

    #[test]
    fn zero_is_a_value_for_the_opaque_identities() {
        assert!(StoreId::ZERO.is_zero());
        assert_eq!(LogicalObjectId::ZERO.get(), 0);
        assert_eq!(Revision::ZERO.get(), 0);
    }
}
