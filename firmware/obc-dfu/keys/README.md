# OBCU update-signing keys

Ed25519 keys for the OBCU v2 update container (`specs/OBCU_Spec.md` §1.3, epic #773 / #997).
Every file here is **one line of 64 lowercase hex characters** (32 raw bytes) plus a
trailing newline — the format `obc-mkimage keygen` writes and
[`obc_dfu::sig::hex32`](../src/sig.rs) parses at compile time. A malformed file is a build
error, not a runtime surprise.

| File | What it is |
|---|---|
| `obcu-release.pub` | The **production** public key, `include_bytes!`d into `obc_dfu::sig::RELEASE_PUBKEY` and compiled into every firmware image. The armer trusts this key and nothing else. |
| `test/obcu-test.seed` | The **test** secret seed. Public by construction — it is in the repo. |
| `test/obcu-test.pub` | The test public key. Used by the host tests, the shared spec vector `specs/vectors/update-container-v2.bin`, and the simulator's synthetic `UPDATE.BIN`. |

There is no private release key in this repository and there never will be. The production
seed lives only in the GitHub Actions environment secret `OBCU_SIGNING_SEED` (environment
`release`) and wherever its offline backup is kept.

## ⚠️ **`obcu-release.pub` currently holds a copy of the TEST key.**

**It MUST be rotated to a freshly generated production key before the first real release.**
Until it is, *anyone* can sign an update image this firmware will install — the seed that
matches it is `test/obcu-test.seed`, sitting right there in the repo. This is fine for
development and CI; it is not fine for a device in the field or an internet-sourced OTA.

The U3 release workflow (epic #773) **refuses to publish** while `obcu-release.pub` still
equals `test/obcu-test.pub` — that equality is the gate, so no release can accidentally ship
trusting a public key. `obc_dfu::sig::tests::release_key_parses_and_is_the_test_key_for_now`
pins the same fact from the other side: it goes red the moment the key *is* rotated, which is
the reminder to flip the workflow gate in the same commit.

### Rotating it (the exact commands)

Run this on a trusted machine, in a directory that is **not** the repo (the seed must never be
staged for commit):

```bash
mkdir -p ~/obc-release-key && chmod 700 ~/obc-release-key
cargo run -p obc-mkimage --release -- keygen --out-dir ~/obc-release-key --name obcu-release
# → ~/obc-release-key/obcu-release.seed   (SECRET, mode 0600)
# → ~/obc-release-key/obcu-release.pub    (public)
```

Publish the public half — this is the change that retires the test key:

```bash
cp ~/obc-release-key/obcu-release.pub firmware/obc-dfu/keys/obcu-release.pub
cargo test -p obc-dfu                 # release_key_parses_and_is_the_test_key_for_now now FAILS
                                      # — delete that test in the same commit; it has done its job
```

Give CI the secret half. `OBCU_SIGNING_SEED` is an **environment** secret in the `release`
environment (not a plain repo secret) so a workflow run must be approved before it can read it:

```bash
gh api -X PUT repos/timohueser/OpenBikeComputer/environments/release   # once, if it doesn't exist
gh secret set OBCU_SIGNING_SEED \
  --repo timohueser/OpenBikeComputer \
  --env release \
  < ~/obc-release-key/obcu-release.seed
```

Then back the seed up offline (a password manager, a paper copy, or both) and **delete the
working copy**:

```bash
shred -u ~/obc-release-key/obcu-release.seed   # macOS: `rm -P`
```

Losing the seed is not fatal — you rotate again and ship a new firmware carrying the new
public key — but every device still running an older image will refuse the new key's updates
until it is updated by hand (SD sideload of an image signed with the *old* key, or SWD). Treat
it accordingly.

### Verifying an artifact against a key

```bash
obc-mkimage inspect UPDATE.BIN                      # against the compiled-in release key
obc-mkimage inspect UPDATE.BIN --pubkey some.pub    # against an explicit key
```

Both exit non-zero on any failure — that is what makes `inspect` usable as a CI gate.
