# FS7.5-c3b handover — read this if you are resuming cold

**Delete this file before the PR goes ready.** It exists because the session that wrote this branch
was working against a usage ceiling and might not have finished. PR #1454 (draft), branch
`fs75c3b-usb-cutover`, base `develop`. Epic #1256, issue #1420, owner ruling = **Option A** (#1420
comment 5344628216).

## Done, committed and pushed

| Commit | What |
|---|---|
| `1b524324` | `specs:` Option A — `FLAT_Store_Protocol.md` §5.2 rewritten, plus new §5.2.1 (the EP0 vendor request) and §5.2.2 (what each retired selector became). **This is the authority for everything else.** |
| `e77f4b1f` | `obc-link`: `Ceilings::for_usb`, `absorb` writes the whole aligned prefix in one call, `Engine::live_upload` / `take_upload_end` (+4 tests, `boot_on` in the harness). |
| `54ff3a89` | The board cutover: `usb/records.rs`, `usb/v4.rs`, `usb/device_info.rs`, the shared `Lane` in `flat_store.rs`, and **the deletions**. |
| `b2762840` | `flat-exercise` stripped whole (owner directive, #1420 I2 closed-not-built); the bench ingest's death trigger written at its module doc. |

Verified green at `b2762840`:

- board `cargo clippy --locked -- -D warnings` → **0**; `--features debug-uart` → **0**;
  `--no-default-features --features ble` → **0**. (The `flat-exercise` leg is deleted, so three
  legs, not four — `.github/workflows/ci.yml` updated to match.)
- `cargo test --workspace` → **0**. `cargo check --workspace --all-targets` → **0**.
- `resource_guard.py board` + `report`, both profiles → **passed**, baseline re-pinned.
- `python3 tools/check_retired_map_stack.py` → **0**.
- `cargo fmt` four-step clean.
- LOC ledger vs `origin/develop`: production **raw −6,372** (code basis −3,671); storage series **+0**.

## Half-done / in flight when this was written

Two subagents were editing this same worktree in disjoint trees. Check `git status` first — if their
files are present but uncommitted, they landed after the last push and need reviewing and committing.

1. **`builder/` — the TypeScript USB client onto v4.** The device no longer speaks the selector
   envelope, so the builder's client at `builder/app/src/lib/usb/` is stale until this lands: it will
   fail against a real device and its `vectors.test.ts` still loads the three deleted
   `transfer-set-*.bin` fixtures. The brief given was: rewrite `protocol.ts` as v4 codecs, replace
   `transport.ts` with a `records.ts` doing §5.2 length-prefixed framing that **reassembles across
   packets**, rewrite `client.ts` as a v4 client (LIST/STATUS/GET/PUT/REMOVE/CANCEL/ARM + the EP0
   device-info read), make `loopback.ts` a v4 mock device, pin `vectors.test.ts` against
   `specs/vectors/flat-store-v4/**` with per-file sha256 like `lib/dosv3/vectors.test.ts` does, and
   update every `lib/device/*` flow and `components/device/*`. Verify with
   `cd builder/app && npm test` and `npm run check`.
2. **Docs + `firmware/obc-fw-nrf54l/README.md`.** Both still describe the v1 envelope, the volume
   set, the arena's USB staging arm and the deleted upload path. `docs/content/software/companion-link.md`
   is the main one. Verify with `python3 docs/build_docs.py --check-links`.

## Untouched from the brief

- **PR body.** #1454's body is a placeholder scope table. The real one owes: the Option A summary,
  the two-sender `Lane` argument, cross-link busy evidence, the deletion inventory with line counts,
  the gate table (CI's figures), the LOC ledger, dev-window gaps, and honest limitations.
- **On-glass.** Nothing here has run on a device. A USB transfer needs the board session, and the
  builder needs a real device behind it.

## The next three concrete steps

1. `git status` — commit whatever the two agents left, after reading it. Re-run the four gates above.
2. Finish the builder cutover if it is not done. It is the only thing that makes the cable usable
   from the host side, and nothing in CI catches its absence except `npm test`.
3. Write the PR body and flip #1454 out of draft.

## Facts a fresh reader will want and cannot easily re-derive

- **The two-sender `Lane` argument** is at `firmware/obc-fw-nrf54l/src/flat_store.rs`,
  `Lane::reclaim`. c3a's FIFO argument named this slice as owing a re-establishment. It is
  re-established by *not needing it*: one caller per reply slot, `REQUEST_QUEUE = 2 × SENDERS` so
  `Sender::send` never parks (a `call` future dropped while parked would leak the `&'static mut`
  reaction buffer forever), therefore `reclaim` **waits on its own slot** — an observation, not an
  inference about another link's calls.
- **Removing the arena's USB arm freed zero RAM.** #1299 had grown the render arm to exactly the USB
  arm's 131,072 B and the arena is `max(arms)`. What it removed is two exclusion rules and the
  ride-loop grant handshake. An earlier draft of `arena.rs` claimed 13,664 B; that was wrong and is
  corrected.
- **The poll frame dropped 9,664 → 1,036 B on the pinned host** (base vs head, same host, same
  toolchain) — exactly the figure c3a's review recorded as reachable and could not attribute.
  `poll_frame_measured` is deliberately **left at CI's 9,728**: writing a host figure into a row the
  `embedded` job gates is the error c3a spent two rounds withdrawing. CI re-pins it.
- **Resident figures are CI's, adjusted by a host-to-host delta.** Base and head were built back to
  back on one host (305,752 → 302,536, −3,216) and that delta applied to CI's pinned base, giving
  302,544 / cap 302,624 / residual 56,880. `measured_flash` is **not** re-pinned — a flash figure is
  not quotable across hosts.
