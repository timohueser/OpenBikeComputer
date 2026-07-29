//! OBCU **v2 image signatures** — Ed25519 over a domain-separated message (`OBCU_Spec.md` §1.3).
//!
//! v1 shipped with a CRC-32 and nothing else, and reserved header bytes `48..60` "for a future
//! signature-scheme marker if internet-sourced OTA ever lands" (epic #773). It landed. This module
//! is the whole cryptographic surface:
//!
//! - the **scheme marker** constants that live in those reserved bytes ([`SIG_SCHEME_ED25519`]),
//! - the exact **signed message** ([`signing_prefix`] — the one place its byte layout exists, shared
//!   by the host signer and the device verifier so they cannot drift),
//! - the embedded **release public key** ([`RELEASE_PUBKEY`], `keys/obcu-release.pub`),
//! - a **streaming** [`Verifier`] so the armer verifies a ~900 KB image in the same single pass it
//!   already CRCs it in — no second read, no image-sized buffer on a 256 KB part.
//!
//! ## The signed message (normative)
//!
//! ```text
//! "OBCUv2-sig\0"          11 bytes — the domain-separation context (NUL included)
//! fw_version[32]           header bytes 16..48, raw and NUL-padded
//! image_len                header bytes  8..12, u32 little-endian
//! image[0 .. image_len]    the raw application image
//! ```
//!
//! The prefix is what makes a signature non-transferable: the context string keeps an OBCU
//! signature from validating in any other protocol that signs raw bytes, and binding
//! `fw_version` + `image_len` keeps a legitimately-signed image from being **re-labelled** — the
//! attacker cannot take v1.4.0's signature and ship the same bytes announced as v9.9.9, nor lie
//! about the length to make the installer read past the image.
//!
//! `image_crc32` is deliberately *not* in the message: it is a function of the image bytes, which
//! are covered, so signing it would add nothing. The scheme marker is not covered either — a
//! rewritten marker only ever moves the container into a bucket the armer rejects outright
//! ([`ScanError::Unsigned`](crate::armer::ScanError::Unsigned)).
//!
//! ## Where verification runs
//!
//! In the **app-side armer** ([`crate::armer::scan`]), before anything is armed. The 32 KB
//! bootloader deliberately does **not** verify (locked in #773): it is flashed once by probe and
//! can never be updated in the field, so putting the trust root there would freeze it forever, and
//! the CRC + verify-before-erase invariants already make a bad stage a no-op. The armer is the
//! gate, and it rejects *unsigned* containers too — otherwise the signature would be bypassable by
//! simply shipping a v1 wrapper.

use crate::image::{ImageHeader, FW_VERSION_LEN};

/// The upstream Ed25519 implementation, re-exported so the host tools sign with the exact crate
/// (and version) the device verifies with — one dependency, no chance of a host/device split.
pub use ed25519_compact;

use ed25519_compact::{KeyPair, Noise, PublicKey as EdPublicKey, Seed, Signature as EdSignature, VerifyingState};

/// `sig_scheme` value for an **unsigned** container — the v1 layout, and what the reserved bytes
/// of every v1 image already read as (they were pinned to zero).
pub const SIG_SCHEME_NONE: u16 = 0;
/// `sig_scheme` value for **Ed25519** over the [`signing_prefix`] message (OBCU v2).
pub const SIG_SCHEME_ED25519: u16 = 1;

/// Bytes of an Ed25519 signature — the size of the v2 trailer.
pub const SIG_LEN: usize = 64;
/// Bytes of an Ed25519 public key.
pub const PUBKEY_LEN: usize = 32;
/// Bytes of an Ed25519 secret **seed** (what `obc-mkimage keygen` writes and CI holds).
pub const SEED_LEN: usize = 32;

/// The domain-separation context prefixed to every OBCU signed message. The trailing NUL is part
/// of it: it makes the context unambiguously terminated, so no `fw_version` can ever extend it.
pub const SIG_CONTEXT: &[u8; 11] = b"OBCUv2-sig\0";

/// Length of the fixed part of the signed message ([`signing_prefix`]'s output): the context, the
/// 32-byte `fw_version` field, and the 4-byte little-endian `image_len`.
pub const SIG_PREFIX_LEN: usize = SIG_CONTEXT.len() + FW_VERSION_LEN + 4;

/// The fixed prefix of the OBCU v2 signed message for `header` — **the** definition of the layout
/// (both `obc-mkimage`'s signer and [`Verifier`] go through this function, so they cannot drift).
/// The raw image follows it, unmodified.
pub fn signing_prefix(header: &ImageHeader) -> [u8; SIG_PREFIX_LEN] {
    let mut out = [0u8; SIG_PREFIX_LEN];
    let c = SIG_CONTEXT.len();
    out[..c].copy_from_slice(SIG_CONTEXT);
    out[c..c + FW_VERSION_LEN].copy_from_slice(&header.fw_version);
    out[c + FW_VERSION_LEN..].copy_from_slice(&header.image_len.to_le_bytes());
    out
}

/// A raw Ed25519 public key. A newtype (not a bare `[u8; 32]`) so the armer's key **seam** is
/// typed: the board passes [`RELEASE_PUBKEY`], the tests pass a test key, and no `include_bytes!`
/// constant is ever swapped behind a feature flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicKey([u8; PUBKEY_LEN]);

impl PublicKey {
    /// Wrap 32 raw key bytes.
    pub const fn from_bytes(bytes: [u8; PUBKEY_LEN]) -> PublicKey {
        PublicKey(bytes)
    }

    /// Parse a key file's contents: **64 lowercase-or-uppercase hex characters**, optionally
    /// followed by a single newline. `const` so a key can be `include_bytes!`d straight into a
    /// constant, and so a malformed key file is a **compile error**, never a runtime surprise on
    /// glass.
    pub const fn from_hex(hex: &[u8]) -> PublicKey {
        PublicKey(hex32(hex))
    }

    /// The raw 32 key bytes.
    pub const fn as_bytes(&self) -> &[u8; PUBKEY_LEN] {
        &self.0
    }
}

/// Parse a 32-byte key file: **64 hex characters**, optionally followed by one newline. `const`, so
/// a malformed key file fails the *build* rather than the device. Shared by the public-key and
/// secret-seed constants — both files have the same one-line-of-hex shape.
pub const fn hex32(hex: &[u8]) -> [u8; 32] {
    assert!(
        hex.len() == 64 || (hex.len() == 65 && hex[64] == b'\n') || (hex.len() == 66 && hex[64] == b'\r'),
        "OBCU key file must be exactly 64 hex characters (plus an optional trailing newline)"
    );
    let mut out = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        out[i] = (nibble(hex[i * 2]) << 4) | nibble(hex[i * 2 + 1]);
        i += 1;
    }
    out
}

/// One hex digit → its value; a compile-time panic on anything else (see [`hex32`]).
const fn nibble(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => panic!("OBCU key file contains a non-hex character"),
    }
}

/// The **committed test key** (`keys/test/`) — the one the host tests, the shared spec vector, and
/// the simulator's synthetic `UPDATE.BIN` use.
///
/// It is deliberately *not* behind a feature: consts cost nothing in a build that doesn't reference
/// them, and hiding it behind a `cfg` would tempt someone to flip the armer's trusted key with a
/// feature flag instead of passing it through [`crate::armer::scan`]'s key parameter — which is the
/// whole point of that seam. Nothing in the firmware's arm path names this module; the armer trusts
/// [`RELEASE_PUBKEY`] and only that.
pub mod test_key {
    use super::{hex32, PublicKey, SEED_LEN};

    /// The test **secret** seed. Public knowledge by construction — it is in the repo. Never sign
    /// anything you intend a real device to install with it.
    pub const SEED: [u8; SEED_LEN] = hex32(include_bytes!("../keys/test/obcu-test.seed"));
    /// The public key [`SEED`] corresponds to.
    pub const PUBLIC: PublicKey = PublicKey::from_hex(include_bytes!("../keys/test/obcu-test.pub"));
}

/// The **production** update-signing public key, compiled into every image from
/// `firmware/obc-dfu/keys/obcu-release.pub`.
///
/// <div class="warning">
///
/// Today this file still holds a **copy of the committed test key**. It MUST be rotated to a
/// freshly generated production key before the first public release — `keys/README.md` has the
/// exact commands, and the U3 release workflow refuses to publish while the two files are equal.
///
/// </div>
pub const RELEASE_PUBKEY: PublicKey = PublicKey::from_hex(include_bytes!("../keys/obcu-release.pub"));

/// Why a signature check failed. Folded by the armer into its user-facing
/// [`ScanError`](crate::armer::ScanError) buckets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigError {
    /// The signature blob wasn't [`SIG_LEN`] bytes.
    BadSigLen,
    /// The signature is structurally invalid (a non-canonical scalar, an `R` off the curve).
    Malformed,
    /// The public key is unusable — not a curve point, or a small-order/identity key.
    BadKey,
    /// Everything parsed; the signature simply isn't valid for this message and key.
    Mismatch,
}

/// A **streaming** Ed25519 verifier over the OBCU v2 message: construct it with the header (the
/// [`signing_prefix`] is absorbed for you), feed the raw image in whatever chunks the caller
/// already reads, then [`finish`](Verifier::finish).
///
/// The armer needs exactly this shape: a ~900 KB staged image is read from the SD card in 512-byte
/// chunks and CRC'd on the way past, and there is no RAM to hold it. Streaming verification folds
/// the signature check into that same single pass — no second read of the card, no image-sized
/// buffer. The state is ~200 bytes of SHA-512 + curve point, all in the caller's frame.
pub struct Verifier {
    state: VerifyingState,
}

impl Verifier {
    /// Start verifying `header`'s image against `signature` under `key`. Fails fast on a malformed
    /// key or signature — before a single image byte is read.
    pub fn new(key: &PublicKey, header: &ImageHeader, signature: &[u8]) -> Result<Verifier, SigError> {
        if signature.len() != SIG_LEN {
            return Err(SigError::BadSigLen);
        }
        let mut sig_bytes = [0u8; SIG_LEN];
        sig_bytes.copy_from_slice(signature);
        let sig = EdSignature::new(sig_bytes);
        let pk = EdPublicKey::new(*key.as_bytes());
        // `verify_incremental` rejects a bad key (not a curve point, or small-order/identity) and a
        // non-canonical signature scalar up front, before any message byte — split the two for the
        // log, since one means "our build is wrong" and the other "this file is junk".
        let mut state = pk.verify_incremental(&sig).map_err(|e| match e {
            ed25519_compact::Error::WeakPublicKey | ed25519_compact::Error::InvalidPublicKey => SigError::BadKey,
            _ => SigError::Malformed,
        })?;
        state.absorb(signing_prefix(header));
        Ok(Verifier { state })
    }

    /// Absorb the next slice of the **raw image** (in order, no gaps).
    pub fn absorb(&mut self, chunk: &[u8]) {
        self.state.absorb(chunk);
    }

    /// The verdict once the whole image has been absorbed.
    pub fn finish(self) -> Result<(), SigError> {
        self.state.verify().map_err(|_| SigError::Mismatch)
    }
}

/// One-shot verification for callers that already hold the whole image in memory (the host tools
/// and the tests; the armer streams instead). Identical message, identical result.
pub fn verify_image(key: &PublicKey, header: &ImageHeader, image: &[u8], signature: &[u8]) -> Result<(), SigError> {
    let mut v = Verifier::new(key, header, signature)?;
    v.absorb(image);
    v.finish()
}

/// The public key a 32-byte secret seed corresponds to.
pub fn public_key_of(seed: &[u8; SEED_LEN]) -> PublicKey {
    let kp = KeyPair::from_seed(Seed::new(*seed));
    let mut out = [0u8; PUBKEY_LEN];
    out.copy_from_slice(kp.pk.as_ref());
    PublicKey(out)
}

/// Sign `image` under `header` with the secret `seed` — the producer half of [`verify_image`], and
/// the only signing path in the repo (`obc-mkimage sign` / `wrap --sign-seed` call straight into it,
/// so the message layout has exactly one definition).
///
/// **Deterministic**: the nonce is derived from the seed and a fixed zero `Noise`, so the same
/// (seed, version, image) always yields the same 64 bytes. That is what lets a signed container be a
/// byte-pinned spec vector and a release build be reproducible. (It also means `ed25519-compact`'s
/// `random` feature must stay off everywhere in the workspace — it would mix fresh entropy into the
/// nonce. [`signing_is_deterministic`](self) pins that.)
pub fn sign_image(seed: &[u8; SEED_LEN], header: &ImageHeader, image: &[u8]) -> [u8; SIG_LEN] {
    let kp = KeyPair::from_seed(Seed::new(*seed));
    let mut st = kp.sk.sign_incremental(Noise::new([0u8; Noise::BYTES]));
    st.absorb(signing_prefix(header));
    st.absorb(image);
    let mut out = [0u8; SIG_LEN];
    out.copy_from_slice(st.sign().as_ref());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SEED: &[u8; SEED_LEN] = &test_key::SEED;
    const OTHER_SEED: &[u8; SEED_LEN] = b"a completely different signing k";

    #[test]
    fn prefix_layout_is_the_spec() {
        let h = ImageHeader::new(b"abcd", "v1.2.3");
        let p = signing_prefix(&h);
        assert_eq!(p.len(), 47);
        assert_eq!(&p[..11], b"OBCUv2-sig\0");
        assert_eq!(&p[11..43], &h.fw_version);
        assert_eq!(&p[43..47], &4u32.to_le_bytes());
    }

    #[test]
    fn roundtrip_verifies() {
        let image = b"the raw application image bytes".as_slice();
        let h = ImageHeader::new(image, "v1.2.3").signed();
        let sig = sign_image(TEST_SEED, &h, image);
        assert_eq!(verify_image(&public_key_of(TEST_SEED), &h, image, &sig), Ok(()));
    }

    #[test]
    fn signing_is_deterministic() {
        // The property the pinned spec vector and reproducible release builds rest on. If this ever
        // goes red, something enabled `ed25519-compact`'s `random` feature in the workspace.
        let image = b"payload".as_slice();
        let h = ImageHeader::new(image, "v1.2.3").signed();
        assert_eq!(sign_image(TEST_SEED, &h, image), sign_image(TEST_SEED, &h, image));
    }

    #[test]
    fn streaming_matches_one_shot() {
        let image: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        let h = ImageHeader::new(&image, "v1.2.3").signed();
        let sig = sign_image(TEST_SEED, &h, &image);
        let mut v = Verifier::new(&public_key_of(TEST_SEED), &h, &sig).unwrap();
        for chunk in image.chunks(37) {
            v.absorb(chunk);
        }
        assert_eq!(v.finish(), Ok(()));
    }

    #[test]
    fn wrong_key_rejects() {
        let image = b"payload".as_slice();
        let h = ImageHeader::new(image, "v1").signed();
        let sig = sign_image(TEST_SEED, &h, image);
        assert_eq!(verify_image(&public_key_of(OTHER_SEED), &h, image, &sig), Err(SigError::Mismatch));
    }

    #[test]
    fn relabelled_version_or_length_rejects() {
        let image = b"payload".as_slice();
        let h = ImageHeader::new(image, "v1.0.0").signed();
        let sig = sign_image(TEST_SEED, &h, image);
        let key = public_key_of(TEST_SEED);

        let mut relabelled = h;
        relabelled.fw_version = ImageHeader::new(image, "v9.9.9").fw_version;
        assert_eq!(verify_image(&key, &relabelled, image, &sig), Err(SigError::Mismatch));

        let mut stretched = h;
        stretched.image_len += 1;
        assert_eq!(verify_image(&key, &stretched, image, &sig), Err(SigError::Mismatch));
    }

    #[test]
    fn flipped_image_byte_rejects() {
        let mut image = b"the raw application image bytes".to_vec();
        let h = ImageHeader::new(&image, "v1").signed();
        let sig = sign_image(TEST_SEED, &h, &image);
        image[7] ^= 0x01;
        assert_eq!(verify_image(&public_key_of(TEST_SEED), &h, &image, &sig), Err(SigError::Mismatch));
    }

    #[test]
    fn malformed_signature_and_key_are_typed() {
        let h = ImageHeader::new(b"x", "v1").signed();
        assert_eq!(Verifier::new(&public_key_of(TEST_SEED), &h, &[0u8; 63]).err(), Some(SigError::BadSigLen));
        // The all-zero key is the identity point — rejected before any image byte is read.
        assert_eq!(Verifier::new(&PublicKey::from_bytes([0; 32]), &h, &[0u8; 64]).err(), Some(SigError::BadKey));
    }

    #[test]
    fn the_committed_test_key_matches_its_seed() {
        // The two files in keys/test/ are a pair or they are useless.
        assert_eq!(public_key_of(&test_key::SEED), test_key::PUBLIC);
    }

    #[test]
    fn release_key_parses_and_is_the_test_key_for_now() {
        // Pins the committed rotation state: until the release key is rotated (see keys/README.md)
        // it is a copy of the test key, and this assertion is the thing that goes red when someone
        // finally rotates it — the prompt to flip the U3 workflow's gate and this test together.
        assert_eq!(RELEASE_PUBKEY, test_key::PUBLIC, "keys/obcu-release.pub still holds the test key (not rotated)");
    }

    #[test]
    fn hex_parse_is_exact() {
        let k = PublicKey::from_hex(b"71331dda025a9658d00c1ef53947ffcafb30e15e8cc9cb585493653b26dd0af6");
        assert_eq!(k.as_bytes()[0], 0x71);
        assert_eq!(k.as_bytes()[31], 0xf6);
        assert_eq!(k, PublicKey::from_hex(include_bytes!("../keys/test/obcu-test.pub")));
    }
}
