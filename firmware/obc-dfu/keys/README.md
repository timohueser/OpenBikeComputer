# OBCU update-signing keys

Ed25519 keys for the OBCU v2 update container ([`specs/OBCU_Spec.md`](../../../specs/OBCU_Spec.md)
§1.3). Every file here is one line of 64 lowercase hex characters (32 raw bytes) and a newline —
what `obc-mkimage keygen` writes and [`obc_dfu::sig::hex32`](../src/sig.rs) parses at compile time.
A malformed file is a build error.

| File | What it is |
|---|---|
| `obcu-release.pub` | The production public key, `include_bytes!`d into `obc_dfu::sig::RELEASE_PUBKEY`. The armer trusts this key and nothing else. |
| `test/obcu-test.seed` | The test secret seed. Public by construction — it is in the repo. |
| `test/obcu-test.pub` | The test public key. Used by the host tests, `specs/vectors/update-container-v2.bin` and the simulator's synthetic package. |

**Never commit a production seed.** There is no private release key in this repository. The
production seed lives only in the GitHub Actions environment secret `OBCU_SIGNING_SEED`
(environment `release`) and in its offline backup.

## ⚠️ `obcu-release.pub` still holds a copy of the test key

Rotate it to a new production key before the first real release. Until then, anyone can sign an
update this firmware installs: the matching seed is `test/obcu-test.seed`, in the repo. This is
acceptable for development and CI only.

The release workflow refuses to publish while `obcu-release.pub` equals `test/obcu-test.pub`.
`obc_dfu::sig::tests::release_key_parses_and_is_the_test_key_for_now` goes red when the key is
rotated; delete that test and open the workflow gate in the same commit.

### Rotate the key

Run this on a trusted machine, in a directory outside the repository, so the seed can never be
staged for commit.

```bash
mkdir -p ~/obc-release-key && chmod 700 ~/obc-release-key
cargo run -p obc-mkimage --release -- keygen --out-dir ~/obc-release-key --name obcu-release

# Publish the public half.
cp ~/obc-release-key/obcu-release.pub firmware/obc-dfu/keys/obcu-release.pub
cargo test -p obc-dfu

# Give CI the secret half. It is an environment secret, so a run must be approved to read it.
gh api -X PUT repos/timohueser/OpenBikeComputer/environments/release   # once, if absent
gh secret set OBCU_SIGNING_SEED --repo timohueser/OpenBikeComputer --env release \
  < ~/obc-release-key/obcu-release.seed

# Back the seed up offline, then destroy the working copy.
shred -u ~/obc-release-key/obcu-release.seed   # macOS: rm -P
```

A lost seed is recoverable: rotate again and ship firmware with the new public key. But every
device on an older image then refuses the new key's updates until it is updated by hand.

### Verify an artifact

```bash
obc-mkimage inspect UPDATE.BIN                      # against the compiled-in release key
obc-mkimage inspect UPDATE.BIN --pubkey some.pub    # against an explicit key
```

Both exit non-zero on any failure, so `inspect` works as a CI gate.
