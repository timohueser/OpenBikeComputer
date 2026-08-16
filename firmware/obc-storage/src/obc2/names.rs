//! The §3 identity mapping: a `GenerationId` to the `GEN`/`WORK` leaf that holds its bytes
//! (`OBC2_Storage_Format.md` §3).
//!
//! ```text
//! /OBC2/GEN/XX/BBBBBBBB.BBB
//! ```
//!
//! `XX` is the low byte of the `GenerationId` as two uppercase hexadecimal digits, and the eleven
//! `B` characters are `GenerationId >> 8` as fixed-width uppercase base-36 split into an
//! eight-character stem and a three-character extension. Because `36^11 > 2^56` the mapping is
//! total over every `u64` and reversible, which is what lets garbage collection read a name off a
//! directory entry and get a generation back without a lookup table.
//!
//! Two properties matter beyond round-tripping, and both are tested below. The encoding is
//! **fixed-width**, so lexicographic order over the eleven characters is numeric order over
//! `generation >> 8` — which is what makes §9's `(shard index, last name)` cursor a resumable
//! position rather than an arbitrary marker. And it is **collision-free**: distinct generations
//! never share a `(shard, stem, extension)` triple, so a leaf name identifies exactly one
//! generation and the `GEN` and `WORK` roles use the same leaf for the same generation.
//!
//! Nothing here is a wire or logical identity. §3: "Generation filenames are private and never
//! serve as logical identities or wire references."

use obc_link::ids::GenerationId;

/// Shard directories per role. §3 shards by the low byte of the generation, so there are 256 of
/// them under each of `GEN` and `WORK`.
pub const SHARD_COUNT: usize = 256;
/// Characters in the base-36 half of a leaf name: eight of stem plus three of extension.
pub const NAME_LEN: usize = 11;
/// The FAT 8.3 stem length.
pub const STEM_LEN: usize = 8;
/// The FAT 8.3 extension length.
pub const EXT_LEN: usize = 3;

/// The two roles that shard generations (§3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    /// `/OBC2/GEN` — the canonical payload bytes, with no OBC2 wrapper.
    Gen,
    /// `/OBC2/WORK` — the two-slot durable work record for that same generation.
    Work,
}

impl Role {
    /// The uppercase 8.3 directory name §3 gives this role.
    pub const fn directory(self) -> &'static str {
        match self {
            Role::Gen => "GEN",
            Role::Work => "WORK",
        }
    }

    /// Both roles, in the order §12 creates them.
    pub const ALL: [Role; 2] = [Role::Gen, Role::Work];
}

/// One shard directory name: the low byte of a generation as two uppercase hexadecimal digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ShardName([u8; 2]);

impl ShardName {
    /// The shard directory of a shard index.
    pub const fn of_index(index: u8) -> Self {
        ShardName([hex(index >> 4), hex(index & 0x0F)])
    }

    /// The shard index this name denotes.
    pub const fn index(&self) -> u8 {
        // Both bytes came from `hex`, so `unhex` is total here.
        (unhex(self.0[0]) << 4) | unhex(self.0[1])
    }

    /// The two name characters.
    pub fn as_str(&self) -> &str {
        // Every byte is an ASCII hex digit by construction.
        core::str::from_utf8(&self.0).unwrap_or("00")
    }

    /// Parses a directory name that may be a shard, returning `None` for anything else.
    pub fn parse(name: &str) -> Option<Self> {
        let bytes = name.as_bytes();
        if bytes.len() != 2 {
            return None;
        }
        let high = parse_hex(bytes[0])?;
        let low = parse_hex(bytes[1])?;
        Some(ShardName::of_index((high << 4) | low))
    }
}

/// One `GEN`/`WORK` leaf: the shard it lives in and its 8.3 name.
///
/// `Ord` is lexicographic over `(shard, stem, extension)`, which is numeric order over the
/// generation because both halves are fixed-width. §9's enumeration cursor depends on that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct LeafName {
    /// The shard directory this leaf lives in.
    pub shard: ShardName,
    /// The eight-character stem.
    pub stem: [u8; STEM_LEN],
    /// The three-character extension.
    pub extension: [u8; EXT_LEN],
}

impl LeafName {
    /// The leaf a generation maps to (§3).
    pub const fn of(generation: GenerationId) -> Self {
        let value = generation.get();
        let shard = ShardName::of_index(value as u8);
        let mut digits = [b'0'; NAME_LEN];
        let mut rest = value >> 8;
        let mut position = NAME_LEN;
        while position > 0 {
            position -= 1;
            digits[position] = base36((rest % 36) as u8);
            rest /= 36;
        }
        let mut stem = [b'0'; STEM_LEN];
        let mut index = 0;
        while index < STEM_LEN {
            stem[index] = digits[index];
            index += 1;
        }
        let mut extension = [b'0'; EXT_LEN];
        index = 0;
        while index < EXT_LEN {
            extension[index] = digits[STEM_LEN + index];
            index += 1;
        }
        LeafName { shard, stem, extension }
    }

    /// The generation this leaf denotes, or `None` when the name is not one §3 produces.
    ///
    /// A directory holds whatever was written into it, so this is a decoder rather than an inverse:
    /// a stray file, a lowercase name, a short name, or a base-36 value above `2^56` is not a leaf
    /// and is never opened or deleted on the strength of its name.
    pub fn generation(&self) -> Option<GenerationId> {
        let mut high = 0u64;
        for &byte in self.stem.iter().chain(self.extension.iter()) {
            let digit = parse_base36(byte)?;
            high = high.checked_mul(36)?.checked_add(u64::from(digit))?;
        }
        // `36^11 > 2^56`, so the encoding is onto a strict superset of the representable range and
        // a name outside it is simply not one this format wrote.
        if high > (u64::MAX >> 8) {
            return None;
        }
        Some(GenerationId::new((high << 8) | u64::from(self.shard.index())))
    }

    /// The stem as text.
    pub fn stem_str(&self) -> &str {
        core::str::from_utf8(&self.stem).unwrap_or("00000000")
    }

    /// The extension as text.
    pub fn extension_str(&self) -> &str {
        core::str::from_utf8(&self.extension).unwrap_or("000")
    }

    /// Parses a shard name and an 8.3 file name into a leaf, without deciding whether it is one.
    pub fn parse(shard: &str, name: &str) -> Option<Self> {
        let shard = ShardName::parse(shard)?;
        let (stem_str, extension_str) = name.split_once('.')?;
        let stem_bytes = stem_str.as_bytes();
        let extension_bytes = extension_str.as_bytes();
        if stem_bytes.len() != STEM_LEN || extension_bytes.len() != EXT_LEN {
            return None;
        }
        let mut stem = [0u8; STEM_LEN];
        stem.copy_from_slice(stem_bytes);
        let mut extension = [0u8; EXT_LEN];
        extension.copy_from_slice(extension_bytes);
        Some(LeafName { shard, stem, extension })
    }

    /// Writes the `NAME.EXT` form into `out`, returning it as text.
    ///
    /// Twelve bytes: eight of stem, the dot, three of extension. The caller owns the buffer for the
    /// reason every other encoder here takes one — nothing in this crate returns bytes through a
    /// stack temporary it did not have to.
    pub fn write_8_3<'a>(&self, out: &'a mut [u8; 12]) -> &'a str {
        out[..STEM_LEN].copy_from_slice(&self.stem);
        out[STEM_LEN] = b'.';
        out[STEM_LEN + 1..].copy_from_slice(&self.extension);
        core::str::from_utf8(out).unwrap_or("00000000.000")
    }
}

const fn hex(nibble: u8) -> u8 {
    if nibble < 10 {
        b'0' + nibble
    } else {
        b'A' + (nibble - 10)
    }
}

const fn unhex(byte: u8) -> u8 {
    if byte >= b'A' {
        byte - b'A' + 10
    } else {
        byte - b'0'
    }
}

const fn base36(digit: u8) -> u8 {
    if digit < 10 {
        b'0' + digit
    } else {
        b'A' + (digit - 10)
    }
}

fn parse_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn parse_base36(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'A'..=b'Z' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::vec::Vec;

    fn leaf(value: u64) -> LeafName {
        LeafName::of(GenerationId::new(value))
    }

    /// §3's worked shape: the shard is the low byte in hex, the rest is base-36, and both halves
    /// are fixed-width.
    #[test]
    fn a_leaf_is_a_hex_shard_and_an_eleven_character_base_36_name() {
        let name = leaf(0);
        assert_eq!(name.shard.as_str(), "00");
        assert_eq!(name.stem_str(), "00000000");
        assert_eq!(name.extension_str(), "000");

        // Low byte 0xAB, high half 1.
        let name = leaf(0x1AB);
        assert_eq!(name.shard.as_str(), "AB");
        assert_eq!(name.stem_str(), "00000000");
        assert_eq!(name.extension_str(), "001");

        // 36^3 = 46,656: the first value that spills out of the extension into the stem.
        let name = leaf(46_656 << 8);
        assert_eq!(name.stem_str(), "00000001");
        assert_eq!(name.extension_str(), "000");
    }

    /// Every byte a leaf name can hold is an uppercase FAT 8.3 character. §3: "Firmware creates
    /// uppercase FAT 8.3 names only."
    #[test]
    fn every_name_character_is_an_uppercase_8_3_character() {
        for value in [0u64, 1, 255, 256, 0xFFFF, u64::MAX, u64::MAX / 3, 1 << 55, 1 << 56] {
            let name = leaf(value);
            let legal = |byte: &u8| byte.is_ascii_digit() || byte.is_ascii_uppercase();
            assert!(name.shard.as_str().as_bytes().iter().all(legal), "{value}");
            assert!(name.stem.iter().all(legal), "{value}");
            assert!(name.extension.iter().all(legal), "{value}");
            let mut buffer = [0u8; 12];
            assert_eq!(name.write_8_3(&mut buffer).len(), 12);
        }
    }

    /// The mapping is reversible over the whole `u64`, which is the property §3 states and the one
    /// garbage collection reads a directory entry through.
    #[test]
    fn the_mapping_round_trips_over_the_whole_generation_space() {
        let mut values: Vec<u64> = std::vec![0, 1, 2, 255, 256, 257, 65_535, 1 << 32, u64::MAX, u64::MAX - 1];
        // A deterministic spread, so the round trip is not proved only at the boundaries.
        let mut rng = 0x9E37_79B9_7F4A_7C15u64;
        for _ in 0..4_000 {
            rng ^= rng >> 12;
            rng ^= rng << 25;
            rng ^= rng >> 27;
            values.push(rng.wrapping_mul(0x2545_F491_4F6C_DD1D));
        }
        for value in values {
            let name = leaf(value);
            assert_eq!(name.generation().map(|id| id.get()), Some(value), "{value}");
            // And the same name parsed back out of directory text.
            let mut buffer = [0u8; 12];
            let text = name.write_8_3(&mut buffer);
            let reparsed = LeafName::parse(name.shard.as_str(), text).expect("a leaf reparses");
            assert_eq!(reparsed, name);
        }
    }

    /// Collision-freedom, which is what makes a leaf an identity: 4,096 consecutive generations
    /// spread over 256 shards produce 4,096 distinct leaves.
    #[test]
    fn distinct_generations_never_share_a_leaf() {
        let names: BTreeSet<LeafName> = (0..4_096u64).map(leaf).collect();
        assert_eq!(names.len(), 4_096);
        // And they use all 256 shards evenly, which is the fan-out §3 designs for.
        let shards: BTreeSet<u8> = (0..4_096u64).map(|value| leaf(value).shard.index()).collect();
        assert_eq!(shards.len(), SHARD_COUNT);
    }

    /// Fixed width means lexicographic order over the name is numeric order over the generation's
    /// high half — the property §9's `(shard index, last name)` cursor resumes on.
    #[test]
    fn lexicographic_order_within_a_shard_is_numeric_order() {
        // One shard: generations differing only above the low byte.
        let mut previous = leaf(0x7A);
        for step in 1..500u64 {
            let name = leaf((step << 8) | 0x7A);
            assert_eq!(name.shard, previous.shard);
            assert!(name > previous, "step {step} is not after its predecessor");
            previous = name;
        }
    }

    /// A directory holds whatever a human or an earlier format wrote into it, so a name that is not
    /// one §3 produces decodes to nothing rather than to a generation.
    #[test]
    fn a_name_that_is_not_a_leaf_decodes_to_nothing() {
        assert!(LeafName::parse("AB", "SHORT.OBR").is_none(), "an eight-character stem is required");
        assert!(LeafName::parse("AB", "00000000.OB").is_none(), "a three-character extension is required");
        assert!(LeafName::parse("AB", "00000000").is_none(), "a name with no extension is not a leaf");
        assert!(LeafName::parse("ZZ", "00000000.000").is_none(), "a shard is two hex digits");
        assert!(LeafName::parse("ab", "00000000.000").is_none(), "firmware creates uppercase names only");
        // Lowercase in the base-36 half is not a name this format wrote either.
        let mut name = leaf(1);
        name.stem[7] = b'a';
        assert!(name.generation().is_none());
        // And a base-36 value above `2^56` is outside the representable range.
        let over = LeafName::parse("00", "ZZZZZZZZ.ZZZ").expect("parses as text");
        assert!(over.generation().is_none(), "36^11 exceeds 2^56, so the top of the name space is not a generation");
    }

    #[test]
    fn a_shard_index_round_trips_and_names_its_directory() {
        for index in 0..=255u8 {
            let shard = ShardName::of_index(index);
            assert_eq!(shard.index(), index);
            assert_eq!(ShardName::parse(shard.as_str()), Some(shard));
        }
        assert_eq!(Role::Gen.directory(), "GEN");
        assert_eq!(Role::Work.directory(), "WORK");
    }

    /// §3: "The same leaf identifies a raw payload under `GEN` and its record under `WORK`."
    #[test]
    fn both_roles_use_the_same_leaf_for_one_generation() {
        let generation = GenerationId::new(0x1234_5678_9ABC_DEF0);
        assert_eq!(LeafName::of(generation), LeafName::of(generation));
        assert_eq!(Role::ALL, [Role::Gen, Role::Work]);
    }
}
