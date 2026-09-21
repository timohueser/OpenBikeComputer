//! OBCU v2 image signatures: Ed25519 over a domain-separated message (`OBCU_Spec.md`).
//!
//! The signed message is:
//!
//! ```text
//! "OBCUv2-sig\0"          11 bytes, the domain-separation context, NUL included
//! fw_version[32]           header bytes 16..48, raw and NUL-padded
//! image_len                header bytes  8..12, u32 little-endian
//! image[0 .. image_len]    the raw application image
//! ```
//!
//! The prefix makes a signature non-transferable: the context keeps it from validating in another
//! protocol that signs raw bytes, and binding `fw_version` and `image_len` keeps a signed image
//! from being re-labelled with a different version or length. `image_crc32` and the scheme marker
//! are not signed: the CRC is a function of the covered bytes, and a rewritten marker only moves
//! the container into a bucket the armer rejects.
//!
//! Verification runs in the app-side armer ([`crate::armer::scan`]), never in the bootloader.

use crate::image::{ImageHeader, FW_VERSION_LEN};

/// Re-exported so the host tools sign with the exact crate version the device verifies with.
pub use ed25519_compact;

use ed25519_compact::{KeyPair, Noise, PublicKey as EdPublicKey, Seed, Signature as EdSignature, VerifyingState};

/// `sig_scheme` value for an unsigned container.
pub const SIG_SCHEME_NONE: u16 = 0;
/// `sig_scheme` value for Ed25519 over the [`signing_prefix`] message.
pub const SIG_SCHEME_ED25519: u16 = 1;

/// Bytes of an Ed25519 signature, which is the size of the container trailer.
pub const SIG_LEN: usize = 64;
pub const PUBKEY_LEN: usize = 32;
/// Bytes of an Ed25519 secret seed, which is what `obc-mkimage keygen` writes.
pub const SEED_LEN: usize = 32;

/// The domain-separation context. The trailing NUL is part of it, so no `fw_version` can extend
/// the context.
pub const SIG_CONTEXT: &[u8; 11] = b"OBCUv2-sig\0";

/// The context, the 32-byte `fw_version` field, and the 4-byte `image_len`.
pub const SIG_PREFIX_LEN: usize = SIG_CONTEXT.len() + FW_VERSION_LEN + 4;

/// The one definition of the message layout: the host signer and [`Verifier`] both go through this
/// function, so they cannot drift. The raw image follows the prefix, unmodified.
pub fn signing_prefix(header: &ImageHeader) -> [u8; SIG_PREFIX_LEN] {
    let mut out = [0u8; SIG_PREFIX_LEN];
    let c = SIG_CONTEXT.len();
    out[..c].copy_from_slice(SIG_CONTEXT);
    out[c..c + FW_VERSION_LEN].copy_from_slice(&header.fw_version);
    out[c + FW_VERSION_LEN..].copy_from_slice(&header.image_len.to_le_bytes());
    out
}

/// A raw Ed25519 public key. The newtype keeps the armer's key a typed parameter, so no key
/// constant is ever swapped behind a feature flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicKey([u8; PUBKEY_LEN]);

impl PublicKey {
    pub const fn from_bytes(bytes: [u8; PUBKEY_LEN]) -> PublicKey {
        PublicKey(bytes)
    }

    /// Parses a key file: 64 hex characters, with an optional trailing newline. It is `const`, so
    /// a malformed key file is a compile error.
    pub const fn from_hex(hex: &[u8]) -> PublicKey {
        PublicKey(hex32(hex))
    }

    pub const fn as_bytes(&self) -> &[u8; PUBKEY_LEN] {
        &self.0
    }
}

/// Parses a 32-byte key file: 64 hex characters, with an optional trailing newline. It is `const`,
/// so a malformed key file fails the build. Both key and seed files have this shape.
pub const fn hex32(hex: &[u8]) -> [u8; 32] {
    assert!(
        hex.len() == 64
            || (hex.len() == 65 && hex[64] == b'\n')
            || (hex.len() == 66 && hex[64] == b'\r' && hex[65] == b'\n'),
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

const fn nibble(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => panic!("OBCU key file contains a non-hex character"),
    }
}

/// The committed test key, used by the host tests, the spec vector, and the simulator's synthetic
/// package. It is not behind a feature, because the armer's key must stay a parameter. Nothing in
/// the firmware's arm path names this module.
pub mod test_key {
    use super::{hex32, PublicKey, SEED_LEN};

    /// The test secret seed. It is in the repo, so never sign an image a real device installs
    /// with it.
    pub const SEED: [u8; SEED_LEN] = hex32(include_bytes!("../keys/test/obcu-test.seed"));
    pub const PUBLIC: PublicKey = PublicKey::from_hex(include_bytes!("../keys/test/obcu-test.pub"));
}

/// The production update-signing public key, compiled into every image.
///
/// The key file still holds a copy of the committed test key. It must be rotated to a production
/// key before the first public release; `keys/README.md` has the commands, and the release workflow
/// refuses to publish while the two files are equal.
pub const RELEASE_PUBKEY: PublicKey = PublicKey::from_hex(include_bytes!("../keys/obcu-release.pub"));

/// Why a signature check failed. The armer folds it into a [`ScanError`](crate::armer::ScanError).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigError {
    /// The signature blob was not [`SIG_LEN`] bytes.
    BadSigLen,
    /// The signature is structurally invalid: a non-canonical scalar, or an `R` off the curve.
    Malformed,
    /// The public key is not a curve point, or is a small-order or identity key.
    BadKey,
    /// Everything parsed, but the signature is not valid for this message and key.
    Mismatch,
}

/// A streaming Ed25519 verifier. Construct it with the header, which absorbs the
/// [`signing_prefix`], feed the raw image in any chunks, then [`finish`](Verifier::finish).
///
/// The armer reads a staged image from the card in small chunks and has no RAM to hold it, so the
/// signature check folds into that same single pass.
pub struct Verifier {
    state: VerifyingState,
}

impl Verifier {
    /// Fails on a malformed key or signature before a single image byte is read.
    pub fn new(key: &PublicKey, header: &ImageHeader, signature: &[u8]) -> Result<Verifier, SigError> {
        if signature.len() != SIG_LEN {
            return Err(SigError::BadSigLen);
        }
        let mut sig_bytes = [0u8; SIG_LEN];
        sig_bytes.copy_from_slice(signature);
        let sig = EdSignature::new(sig_bytes);
        let pk = EdPublicKey::new(*key.as_bytes());
        // Split the two errors for the log: a bad key means this build is wrong, a malformed
        // signature means the file is junk.
        let mut state = pk.verify_incremental(&sig).map_err(|e| match e {
            ed25519_compact::Error::WeakPublicKey | ed25519_compact::Error::InvalidPublicKey => SigError::BadKey,
            _ => SigError::Malformed,
        })?;
        state.absorb(signing_prefix(header));
        Ok(Verifier { state })
    }

    /// Absorbs the next slice of the raw image, in order and with no gaps.
    pub fn absorb(&mut self, chunk: &[u8]) {
        self.state.absorb(chunk);
    }

    pub fn finish(self) -> Result<(), SigError> {
        self.state.verify().map_err(|_| SigError::Mismatch)
    }
}

/// One-shot verification for callers that hold the whole image in memory. Same message, same
/// result as the streaming [`Verifier`].
pub fn verify_image(key: &PublicKey, header: &ImageHeader, image: &[u8], signature: &[u8]) -> Result<(), SigError> {
    let mut v = Verifier::new(key, header, signature)?;
    v.absorb(image);
    v.finish()
}

pub fn public_key_of(seed: &[u8; SEED_LEN]) -> PublicKey {
    let kp = KeyPair::from_seed(Seed::new(*seed));
    let mut out = [0u8; PUBKEY_LEN];
    out.copy_from_slice(kp.pk.as_ref());
    PublicKey(out)
}

/// The only signing path in the repo; `obc-mkimage` calls straight into it.
///
/// It is deterministic: the nonce comes from the seed and a fixed zero `Noise`, so the same seed,
/// version and image always give the same 64 bytes. Spec vectors and reproducible release builds
/// rest on that, so `ed25519-compact`'s `random` feature must stay off across the workspace.
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
        // If this goes red, something enabled `ed25519-compact`'s `random` feature.
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
        assert_eq!(public_key_of(&test_key::SEED), test_key::PUBLIC);
    }

    #[test]
    fn release_key_parses_and_is_the_test_key_for_now() {
        // This goes red when the release key is finally rotated; flip the workflow gate with it.
        assert_eq!(RELEASE_PUBKEY, test_key::PUBLIC, "keys/obcu-release.pub still holds the test key (not rotated)");
    }

    #[test]
    fn hex_parse_is_exact() {
        let k = PublicKey::from_hex(b"71331dda025a9658d00c1ef53947ffcafb30e15e8cc9cb585493653b26dd0af6");
        assert_eq!(k.as_bytes()[0], 0x71);
        assert_eq!(k.as_bytes()[31], 0xf6);
        assert_eq!(k, PublicKey::from_hex(include_bytes!("../keys/test/obcu-test.pub")));
        assert_eq!(k, PublicKey::from_hex(b"71331dda025a9658d00c1ef53947ffcafb30e15e8cc9cb585493653b26dd0af6\r\n"));
        assert!(std::panic::catch_unwind(|| {
            PublicKey::from_hex(b"71331dda025a9658d00c1ef53947ffcafb30e15e8cc9cb585493653b26dd0af6\rX")
        })
        .is_err());
    }
}
