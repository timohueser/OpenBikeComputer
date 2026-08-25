//! The one settings-blob table.
//!
//! Every persisted settings *field* used to be the same seven things restated: a struct field, a
//! `DEFAULT` entry, a link in the offset chain, an `encode` line, a `decode` line, maybe a
//! `sanitize` clamp, maybe an `adopt_ble_fields` assignment. [`settings_table!`] takes one row per
//! field and writes all seven, so **the declaration is the contract**: the blob's layout is the
//! table's order, and a field can no longer be declared in one place and forgotten in another.
//!
//! Deliberately a dumb token-pasting table, not a framework — the sibling of
//! [`setting_enum!`](crate::settings_enum) and [`screens!`](crate::screen). The macro knows
//! nothing about *bytes*: how a value packs is [`SettingCodec`], one impl per field kind, all of
//! them below — so the blob's six kinds are readable in one place instead of spread through a
//! 76-line `encode`.
//!
//! What is **not** here: the version byte's meaning, the supported floor, the CRC and the
//! `ENCODED_LEN` rounding all stay in [`settings`](crate::settings) next to the migration rule they
//! belong to. The macro emits the fixed framing (`b[0] = VERSION`, the trailing CRC, the
//! `MIN_SUPPORTED..=VERSION` gate) and the per-field loops between them.

use crate::retention::RideRetention;
use crate::settings::{DeviceName, SavedSensor, DEVICE_NAME_MAX, SENSOR_SLOTS};
use crate::stat_fields::{StatFieldList, MAX_STAT_FIELDS};
use crate::weather_alerts::{AlertMark, AlertMarks, ALERT_CLASSES};
use obc_ports::DateTime;

/// How one settings field packs into its own slice of the blob.
///
/// The slice handed to [`write`](SettingCodec::write) and [`read`](SettingCodec::read) is exactly
/// [`LEN`](SettingCodec::LEN) bytes — the field's span, already cut by the generated codec — so an
/// impl never does offset arithmetic and cannot reach a neighbour's bytes. `LEN` is an associated
/// const, which is what lets the generated offset chain stay `const`.
///
/// `read` is total: a corrupt-but-CRC-valid byte sanitises to something usable (the stored value's
/// own fallback), never a panic and never garbage — the blob is device state, and a bit-flip the
/// CRC missed must still boot the device.
///
/// `write` is handed a **zeroed** span — the generated `encode` is its only caller and always
/// starts from a fresh zeroed blob — so an impl writes only the bytes it has content for and
/// leaves an absent slot, a short name's padding, or a reserved byte as the zeros they already
/// are. That is the layout on disk, not an accident of the buffer.
pub(crate) trait SettingCodec: Sized {
    /// The field's byte length in the blob.
    const LEN: usize;
    /// Pack `self` into its `LEN`-byte span.
    fn write(&self, dst: &mut [u8]);
    /// Rebuild from a `LEN`-byte span, sanitising anything unrepresentable.
    fn read(src: &[u8]) -> Self;
}

impl SettingCodec for u8 {
    const LEN: usize = 1;
    #[inline]
    fn write(&self, dst: &mut [u8]) {
        dst[0] = *self;
    }
    #[inline]
    fn read(src: &[u8]) -> Self {
        src[0]
    }
}

impl SettingCodec for bool {
    const LEN: usize = 1;
    #[inline]
    fn write(&self, dst: &mut [u8]) {
        dst[0] = *self as u8;
    }
    /// Any non-zero byte reads as `true` — a stored flag is never "corrupt", only set or clear.
    #[inline]
    fn read(src: &[u8]) -> Self {
        src[0] != 0
    }
}

/// The little-endian integer kinds — identical but for the type, so they are written once.
macro_rules! impl_le_scalar {
    ($($T:ty),+) => { $(
        impl SettingCodec for $T {
            const LEN: usize = core::mem::size_of::<$T>();
            #[inline]
            fn write(&self, dst: &mut [u8]) {
                dst.copy_from_slice(&self.to_le_bytes());
            }
            #[inline]
            fn read(src: &[u8]) -> Self {
                <$T>::from_le_bytes([src[0], src[1]])
            }
        }
    )+ };
}

impl_le_scalar!(u16, i16);

impl SettingCodec for DateTime {
    const LEN: usize = 6;
    #[inline]
    fn write(&self, dst: &mut [u8]) {
        dst[0..2].copy_from_slice(&self.year.to_le_bytes());
        dst[2] = self.month;
        dst[3] = self.day;
        dst[4] = self.hour;
        dst[5] = self.minute;
    }
    /// Read verbatim — the storage range (2020–2099, a leap-aware day) is app policy, applied by
    /// the table's `sanitize_with` hook after the whole blob is read.
    #[inline]
    fn read(src: &[u8]) -> Self {
        DateTime {
            year: u16::from_le_bytes([src[0], src[1]]),
            month: src[2],
            day: src[3],
            hour: src[4],
            minute: src[5],
        }
    }
}

impl SettingCodec for DeviceName {
    const LEN: usize = 1 + DEVICE_NAME_MAX;

    #[inline]
    fn write(&self, dst: &mut [u8]) {
        let name = self.as_str().as_bytes();
        dst[0] = name.len() as u8;
        dst[1..1 + name.len()].copy_from_slice(name);
    }

    /// A stored length past the cap (corrupt-but-CRC-valid input) sanitises to the factory name,
    /// exactly like invalid UTF-8 inside [`DeviceName::from_bytes`] — never a garbage prefix.
    #[inline]
    fn read(src: &[u8]) -> Self {
        match src[0] as usize {
            n if n <= DEVICE_NAME_MAX => DeviceName::from_bytes(&src[1..1 + n]),
            _ => DeviceName::EMPTY,
        }
    }
}

impl SettingCodec for StatFieldList {
    const LEN: usize = 1 + MAX_STAT_FIELDS;

    #[inline]
    fn write(&self, dst: &mut [u8]) {
        let (len, ids) = self.encode();
        dst[0] = len;
        dst[1..].copy_from_slice(&ids);
    }

    /// The selection sanitises inside [`StatFieldList::decode`] as it is parsed — an unknown
    /// discriminant is dropped rather than loaded as a garbage tile.
    #[inline]
    fn read(src: &[u8]) -> Self {
        StatFieldList::decode(src[0], &src[1..])
    }
}

/// Bytes per saved-sensor slot: `present(1) · addr_kind(1) · addr[6]`.
pub(crate) const SAVED_SENSOR_LEN: usize = 8;

impl SettingCodec for [SavedSensor; SENSOR_SLOTS] {
    const LEN: usize = SENSOR_SLOTS * SAVED_SENSOR_LEN;

    #[inline]
    fn write(&self, dst: &mut [u8]) {
        for (q, slot) in self.iter().enumerate() {
            let off = q * SAVED_SENSOR_LEN;
            dst[off] = slot.present as u8;
            dst[off + 1] = slot.addr_kind;
            dst[off + 2..off + 2 + 6].copy_from_slice(&slot.addr);
        }
    }

    /// An absent slot (`present == 0`) reads as [`SavedSensor::EMPTY`] regardless of the stored
    /// address; a present slot keeps its address and normalises `addr_kind` to `0`/`1` (`!= 0` =
    /// random), matching how the board maps it to `AddrKind` — so a bit-flip never mis-picks the
    /// address kind.
    #[inline]
    fn read(src: &[u8]) -> Self {
        let mut slots = [SavedSensor::EMPTY; SENSOR_SLOTS];
        for (q, slot) in slots.iter_mut().enumerate() {
            let off = q * SAVED_SENSOR_LEN;
            if src[off] != 0 {
                let mut addr = [0u8; 6];
                addr.copy_from_slice(&src[off + 2..off + 2 + 6]);
                *slot = SavedSensor::saved((src[off + 1] != 0) as u8, addr);
            }
        }
        slots
    }
}

/// Bytes per persisted alert mark: `flags(1) · onset i64 LE(8) · lat i32 LE(4) · lon i32 LE(4)
/// · severity(1)`. The leading byte is a flag *pair*, not a bool: bit 0 = the slot holds a mark,
/// bit 1 = that mark has a position. Position presence has to survive the write — a mark fired
/// before the first GPS fix has no coordinate, and dedup must compare it by time alone rather
/// than by ground distance to a fabricated `(0, 0)` (see [`crate::weather_alerts::same_event`]).
/// Both bits fit the byte that was already there, so the record and the blob keep their size.
pub(crate) const ALERT_MARK_LEN: usize = 18;
/// `flags` bit 0: this slot holds a mark at all.
pub(crate) const ALERT_MARK_PRESENT: u8 = 1 << 0;
/// `flags` bit 1: the stored `lat`/`lon` are a real position (else the mark has none).
pub(crate) const ALERT_MARK_HAS_POS: u8 = 1 << 1;

impl SettingCodec for AlertMarks {
    const LEN: usize = ALERT_CLASSES * ALERT_MARK_LEN;

    #[inline]
    fn write(&self, dst: &mut [u8]) {
        for (slot, mark) in self.iter().enumerate() {
            let off = slot * ALERT_MARK_LEN;
            if let Some(mark) = mark {
                dst[off] = ALERT_MARK_PRESENT;
                dst[off + 1..off + 9].copy_from_slice(&mark.onset.to_le_bytes());
                if let Some((lat, lon)) = mark.pos {
                    dst[off] |= ALERT_MARK_HAS_POS;
                    dst[off + 9..off + 13].copy_from_slice(&lat.to_le_bytes());
                    dst[off + 13..off + 17].copy_from_slice(&lon.to_le_bytes());
                }
                dst[off + 17] = mark.severity;
            }
        }
    }

    /// An absent slot ([`ALERT_MARK_PRESENT`] clear) reads as `None` regardless of the stored
    /// payload; a present slot without [`ALERT_MARK_HAS_POS`] keeps its position *absent* rather
    /// than reading the zeroed coordinate bytes as null island. Every stored value is a legal mark
    /// (any onset/position/severity is comparable), so there is no range clamp to apply.
    #[inline]
    fn read(src: &[u8]) -> Self {
        let mut marks: AlertMarks = [None; ALERT_CLASSES];
        for (slot, mark) in marks.iter_mut().enumerate() {
            let off = slot * ALERT_MARK_LEN;
            if src[off] & ALERT_MARK_PRESENT != 0 {
                let pos = (src[off] & ALERT_MARK_HAS_POS != 0).then(|| {
                    (
                        i32::from_le_bytes(src[off + 9..off + 13].try_into().unwrap()),
                        i32::from_le_bytes(src[off + 13..off + 17].try_into().unwrap()),
                    )
                });
                *mark = Some(AlertMark {
                    onset: i64::from_le_bytes(src[off + 1..off + 9].try_into().unwrap()),
                    pos,
                    severity: src[off + 17],
                });
            }
        }
        marks
    }
}

impl SettingCodec for RideRetention {
    const LEN: usize = 1;
    #[inline]
    fn write(&self, dst: &mut [u8]) {
        dst[0] = self.as_u8();
    }
    #[inline]
    fn read(src: &[u8]) -> Self {
        RideRetention::from_u8(src[0])
    }
}

/// Give a [`setting_enum!`](crate::settings_enum) type its one-byte codec: the declared
/// discriminant *is* the stored byte, and an unknown byte sanitises to the enum's default through
/// `from_byte`. Invoked once by `setting_enum!`, so a declared enum is settings-blob-ready.
macro_rules! setting_enum_codec {
    ($Name:ident) => {
        impl $crate::settings_table::SettingCodec for $Name {
            const LEN: usize = 1;
            #[inline]
            fn write(&self, dst: &mut [u8]) {
                dst[0] = *self as u8;
            }
            #[inline]
            fn read(src: &[u8]) -> Self {
                Self::from_byte(src[0])
            }
        }
    };
}

/// Declare the persisted settings set: one row per field, in **blob order**.
///
/// The live declaration is [`Settings`](crate::settings::Settings) in `settings.rs` — read that
/// for the real thing. One row of each shape:
///
/// ```ignore
/// /// Metric or imperial readouts.
/// units: Units = Units::Metric, since(16), ble_writable, reserved(1);
/// /// Local time's offset from UTC, in minutes.
/// utc_offset_min: i16 = 0, since(16), range(UTC_OFFSET_MIN, UTC_OFFSET_MAX);
/// /// The last time source's UTC set-point.
/// clock: DateTime = DateTime::DEFAULT, since(16), sanitize_with(DateTimeEditorExt::sanitize);
/// ```
///
/// A row is `name: Type = default, since(v)` — `since` is **required** and positional, because a
/// marker whose absence silently means "v1" is a default nobody can see. It is the version that
/// first wrote this field's bytes: a blob stamped older than that decodes the field as its declared
/// `default` instead of reading bytes the writer never wrote (see the generated `decode`). Rows are
/// in append order, so the column is non-decreasing and no greater than `VERSION` — both are
/// compile errors.
///
/// After `since` come any of four optional markers:
///
/// - `ble_writable` — the phone owns this field, so the generated `adopt_ble_fields` pulls it
///   across. Its absence is what makes every other field device-only.
/// - `range(MIN, MAX)` — clamp on decode. Its **absence is meaningful**: a field with no `range`
///   is deliberately never clamped, and says why in its own doc.
/// - `sanitize_with(path::to::fn)` — call `f(&mut field)` on decode instead of a clamp.
/// - `reserved(n)` — `n` frozen bytes follow this field in the blob: a retired field's tombstone,
///   written as zeros and ignored on decode, holding the layout of every field after it.
///
/// Generated: the struct (with every row's doc verbatim), `Default`, the named `const` default,
/// the `off::…` offset chain and `off::END`, `payload_len`, `encode`, `decode`, `sanitize` and
/// `adopt_ble_fields`. The last five sections carry only their doc and name — the prose about
/// *this* blob belongs at the declaration, not in the generator.
///
/// The offset chain starts at byte 1: byte 0 is the version, which is not a field and is written
/// by the generated `encode` directly.
macro_rules! settings_table {
    (
        $(#[$sm:meta])*
        $svis:vis struct $S:ident {
            $(
                $(#[$fm:meta])*
                $name:ident : $ty:ty = $default:expr, since($sv:literal)
                $(, $( $mk:ident $(( $($arg:tt)* ))? ),+ )?
                ;
            )+
        }

        $(#[$dm:meta])*
        $dvis:vis const $DEFAULT:ident;

        $(#[$am:meta])*
        $avis:vis fn $adopt:ident;

        $(#[$zm:meta])*
        $zvis:vis fn $sanitize:ident;

        $(#[$em:meta])*
        $evis:vis fn $encode:ident;

        $(#[$cm:meta])*
        $cvis:vis fn $decode:ident;
    ) => {
        $(#[$sm])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        $svis struct $S {
            $( $(#[$fm])* pub $name: $ty, )+
        }

        impl Default for $S {
            fn default() -> Self {
                Self::$DEFAULT
            }
        }

        impl $S {
            $(#[$dm])*
            $dvis const $DEFAULT: $S = $S { $( $name: $default, )+ };

            $(#[$am])*
            $avis fn $adopt(&mut self, other: &$S) {
                $($( $( $crate::settings_table::settings_table!(@adopt $mk $(( $($arg)* ))?, self, other, $name); )+ )?)+
            }

            $(#[$zm])*
            $zvis fn $sanitize(&mut self) {
                $($( $( $crate::settings_table::settings_table!(@sanitize $mk $(( $($arg)* ))?, self, $name); )+ )?)+
            }

            /// Table-driven coverage for every declared field (see the one settings-table test):
            /// `other` moves **every** row off `base`, and `adopted` — `base` after
            #[doc = concat!("[`", stringify!($adopt), "`](Self::", stringify!($adopt), ") of `other` — took the fields named in `ble_writable` and no others.")]
            ///
            /// Generated with the table so a new row is covered the moment it is declared, but the
            /// **expectation is not**: `ble_writable` is a hand-written list of names, held against
            /// the generated behaviour the same way the hand-written offset literals are held
            /// against the generated chain. Deriving it from the row's marker would only restate
            /// the token that produced the behaviour, and could not fail. Adding or dropping a
            /// marker without editing that list fails here, by field name.
            #[cfg(test)]
            pub(crate) fn assert_field_table(base: &$S, other: &$S, adopted: &$S, ble_writable: &[&str]) {
                let mut named = 0;
                $(
                    assert_ne!(base.$name, other.$name, concat!(stringify!($name), " is never moved off its default"));
                    if ble_writable.contains(&stringify!($name)) {
                        named += 1;
                        assert_eq!(adopted.$name, other.$name, concat!(stringify!($name), " is ble_writable → adopted"));
                    } else {
                        assert_eq!(adopted.$name, base.$name, concat!(stringify!($name), " is device-only → untouched"));
                    }
                )+
                assert_eq!(named, ble_writable.len(), "every name in `ble_writable` is a declared field");
            }
        }

        /// Each field's byte offset in the blob, summed from the table's declared lengths — the
        /// chain that used to be twenty-two hand-linked consts. `END` is the first byte past the
        /// last field. Pinned against hand-written literals at the declaration.
        #[allow(non_upper_case_globals)]
        mod off {
            use super::*;
            $crate::settings_table::settings_table!(@offsets 1; $( $name: $ty, [ $( $( $mk $(( $($arg)* ))? , )+ )? ]; )+);
        }

        /// The append-only law as build errors. No assert restates the token that generated it:
        /// each holds the generated `since` column against a **different** declaration — the row
        /// above it, and the hand-written `VERSION` / `MIN_SUPPORTED` consts.
        const _: () = {
            const SINCE: &[u8] = &[ $( $sv, )+ ];
            assert!(MIN_SUPPORTED <= VERSION, "the supported floor is newer than the version being written");
            // Without this, a first row newer than the floor collapses `payload_len(floor)` to 1,
            // and `decode` launders a blob whose CRC covers byte 0 alone into an all-default
            // `Some`. The floor has to sit inside the table it decodes.
            assert!(SINCE[0] <= MIN_SUPPORTED, "the oldest supported version predates the first row");
            let mut i = 0;
            while i < SINCE.len() {
                assert!(SINCE[i] <= VERSION, "a row is `since` a version newer than VERSION — bump VERSION with the row");
                assert!(i == 0 || SINCE[i - 1] <= SINCE[i], "rows are append-only: a row's `since` may not precede the row above it");
                i += 1;
            }
        };

        /// The payload length of a blob written by version `v`: the offset past the last row that
        /// version declared. A running sum in declaration order, like the `off::…` chain — and
        /// pinned against hand-written literals at the declaration for the same reason it is, since
        /// an assert derived from the `since` column could not fail.
        ///
        /// Only `MIN_SUPPORTED..=VERSION` is ever decoded; a lower `v` still answers, which is what
        /// gives the literal block something to catch a mistyped `since` with.
        const fn payload_len(v: u8) -> usize {
            let mut len = 1; // byte 0 is the version, which is not a field
            $(
                if v >= $sv {
                    len = off::$name
                        + <$ty as $crate::settings_table::SettingCodec>::LEN
                        $($( + $crate::settings_table::settings_table!(@gap $mk $(( $($arg)* ))?) )+)?;
                }
            )+
            len
        }

        $(#[$em])*
        $evis fn $encode(s: &$S) -> [u8; ENCODED_LEN] {
            let mut b = [0u8; ENCODED_LEN];
            b[0] = VERSION;
            $(
                <$ty as $crate::settings_table::SettingCodec>::write(
                    &s.$name,
                    &mut b[off::$name..off::$name + <$ty as $crate::settings_table::SettingCodec>::LEN],
                );
            )+
            let crc = $crate::store_meta::crc16(&b[0..PAYLOAD_LEN]);
            b[PAYLOAD_LEN..PAYLOAD_LEN + 2].copy_from_slice(&crc.to_le_bytes());
            b
        }

        $(#[$cm])*
        $cvis fn $decode(bytes: &[u8]) -> Option<$S> {
            // Everything below is relative to the **stored** version, not the running one: its
            // payload length, its encoded length, and the span its CRC covers. That is what lets a
            // blob written before the last field was appended still be read.
            let v = *bytes.first()?;
            if !(MIN_SUPPORTED..=VERSION).contains(&v) {
                return None;
            }
            let plen = payload_len(v);
            if bytes.len() < encoded_len(plen) {
                return None;
            }
            let b = &bytes[..plen + 2];
            let crc = u16::from_le_bytes([b[plen], b[plen + 1]]);
            if crc != $crate::store_meta::crc16(&b[0..plen]) {
                return None;
            }
            let mut s = $S {
                $(
                    $name: if v >= $sv {
                        <$ty as $crate::settings_table::SettingCodec>::read(
                            &b[off::$name..off::$name + <$ty as $crate::settings_table::SettingCodec>::LEN],
                        )
                    } else {
                        // The declared default, verbatim — the same token `DEFAULT` is built from,
                        // so a tail-defaulted field costs no `.rodata` projection of the whole
                        // struct.
                        $default
                    },
                )+
            };
            s.$sanitize();
            Some(s)
        }
    };

    // The offset chain: each row's offset is the previous one plus that field's `SettingCodec::LEN`
    // plus any `reserved` bytes it carries. A muncher, so every link is written once.
    (@offsets $acc:expr;) => {
        /// The first byte past the last declared field — the payload length.
        pub const END: usize = $acc;
    };
    (@offsets $acc:expr; $name:ident: $ty:ty, [ $( $mk:ident $(( $($arg:tt)* ))? , )* ]; $($rest:tt)*) => {
        pub const $name: usize = $acc;
        settings_table!(
            @offsets $name + <$ty as $crate::settings_table::SettingCodec>::LEN
                $( + $crate::settings_table::settings_table!(@gap $mk $(( $($arg)* ))?) )*;
            $($rest)*
        );
    };

    // Marker dispatch. Each pass asks every marker what it contributes and gets nothing from the
    // markers that belong to another pass.
    (@gap reserved($n:literal)) => { $n };
    (@gap $mk:ident $(( $($arg:tt)* ))?) => { 0 };

    (@adopt ble_writable, $s:ident, $o:ident, $name:ident) => { $s.$name = $o.$name; };
    (@adopt $mk:ident $(( $($arg:tt)* ))?, $s:ident, $o:ident, $name:ident) => {};

    (@sanitize range($lo:expr, $hi:expr), $s:ident, $name:ident) => { $s.$name = $s.$name.clamp($lo, $hi); };
    (@sanitize sanitize_with($f:path), $s:ident, $name:ident) => { $f(&mut $s.$name); };
    (@sanitize $mk:ident $(( $($arg:tt)* ))?, $s:ident, $name:ident) => {};

}

pub(crate) use setting_enum_codec;
pub(crate) use settings_table;
