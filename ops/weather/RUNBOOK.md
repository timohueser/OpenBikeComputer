# OBC weather service — runbook

Everything needed to build, run, watch, repair and rebuild the weather bakery. WX18 (#1206) of
epic #1185.

> **The mosaic cutover has not been executed yet (WXR8 #1247).** The repository builds a baker that
> publishes **only** the one mosaic dataset at `wx/v2/`; the **deployed VPS is still running the
> previous build**, so `wx/v1` is what is actually being served right now. Everything below
> describes the service *after* the cutover. **[The cutover](#the-cutover--do-this-once-in-this-order)
> is the section to read first**, and it is the only place the two states are both described.
> Delete that section, and this banner, once it has been executed.

The service is **one stateless publisher**: a small VPS runs `obc-wx-bake` on a systemd timer, each
tick fetches upstream radar/model data, mosaics them onto one global lattice, and swaps one
`manifest.json` in R2. Nothing else exists — no database, no accounts, no per-request compute, no
rider coordinate ever reaching it. That shape is what makes the failure story boring:

> If the box dies, R2 keeps serving the last objects. Frames carry their real timestamps, the
> manifest carries an absolute `stale_after`, and the phone/device stop using an expired generation
> — showing **WEATHER UPDATE NEEDED**, never "dry". An outage costs freshness, never truth. So the
> alarm is e-mail-grade, and lives on GitHub, outside the machine it watches.

One thing did change shape with the mosaic, and it is worth stating before anything else in this
file is read: **the baker now deletes.** After every successful publish it retires the generation
its new manifest no longer names — current plus two, and nothing else (`OBCG_Spec.md` §10.4). That
is a capability the service deliberately did not have until WXR8, so the failure mode "a bug
destroys published data" exists now where it did not before, and §6, §7 and §8 each have a row
about it.

| Where | What |
| :-- | :-- |
| `ops/weather/install.sh` | Idempotent installer: user, dirs, binary, units, journal caps |
| `ops/weather/adapters.conf` | The cadence table — the one place a schedule is defined (one `cycle` row since #1246) |
| `ops/weather/systemd/obc-wx-bake@.service` | The hardened one-shot bake unit (`%i` = the subcommand) |
| `ops/weather/freshness_probe.py` | The external freshness/cost probe; also runnable by hand |
| `ops/weather/tests/` | Its self-tests (`python3 -m unittest discover -s ops/weather/tests`), run by CI |
| `.github/workflows/wx-freshness.yml` | Runs the probe every 15 min, opens/closes one alert issue |
| `host/obc-wx-bake/` | The baker itself (WX5 #1215); `specs/OBCG_Spec.md` is its output contract |

On the box:

| Path | Owner / mode | What |
| :-- | :-- | :-- |
| `/usr/local/bin/obc-wx-bake` | root 0755 | the binary |
| `/etc/obc-wx/r2.env` | **root 0600** | R2 credentials — never in git, never world-readable |
| `/var/lib/obc-wx/` | obc-wx 0750 | state dir: only the `bake.lock` and dry-run scratch |
| `/etc/systemd/system/obc-wx-bake@.service` | root 0644 | the unit template |
| `/etc/systemd/system/obc-wx-bake@cycle.timer` | root 0644 | generated from `adapters.conf` |

---

## The cutover — do this once, in this order

**Status: not executed.** Ordering matters even pre-release, because the bucket is live, the timers
are live, and there are shipped clients reading `wx/v1`. Do not improvise the order: the two
irreversible-feeling steps (retiring the per-adapter timers, deleting the v1 tree) are both late,
and everything before them is a rehearsal you can walk away from.

**C0 — Silence the alarm honestly, before this repository's probe reaches the runner.**
The probe reads `wx/v2/manifest.json` and nothing else now, and the live service does not publish
one yet, so the first scheduled run after this lands would report **UNREACHABLE** and open an alert
issue about a service that is working fine. Clear the repository variable `OBC_WX_BASE_URL`
(GitHub → *Settings* → *Secrets and variables* → *Actions* → *Variables*). The workflow reports
"skipped" and passes — that is the same contract that covers a project with no service deployed at
all. **C8 restores it.** While it is cleared, the only weather monitoring is your own eyes on the
journal; do not leave it cleared overnight.

**C1 — Rehearse into a directory, on the box, publishing nothing — and rehearse the *sweep*.**
`--store <dir>` is a first-class destination, not a debug mode: it writes the identical tree R2
gets. One run is not enough: a fresh directory has no predecessor manifest, so the sweep has nothing
to do, and repeating the command inside the same quarter hour only re-bakes the same generation.
`--now` is what makes four *distinct* generations, which is the point at which the first deletion
happens.

```sh
ssh root@wx
export PATH="$HOME/.cargo/bin:$PATH"      # cargo is NOT on root's default PATH — see C2
REH=/var/lib/obc-wx/rehearsal; rm -rf $REH
# Four anchors walking *forward* to now, so upstream actually has data for each and each cycle is
# newer than the last (a cycle older than the published manifest refuses to publish — by design).
for M in 45 30 15 0; do
  sudo -u obc-wx /tmp/obc-wx-bake cycle --store $REH --now "$(date -u -d "-$M min" +%Y-%m-%dT%H:%M:00Z)"
done
ls $REH/wx/v2/                # EXACTLY three generation directories + manifest.json
python3 ops/weather/freshness_probe.py --manifest $REH/wx/v2/manifest.json
```

The fourth run must print `retired <the first generation> (N objects)` and the listing must show
**three** generations, not four — that is the whole retention contract executed end to end, against
a store you can delete with `rm -rf`, before the first destructive operation this crate has ever had
runs against live R2. The probe must exit `0`.

Then read the tree the way a client does — `rclone serve` answers Range requests, which
`python3 -m http.server` does not, and Range is the whole of corridor extraction:

```sh
rclone serve http --addr 127.0.0.1:8080 $REH &
obc-wx-client --service http://127.0.0.1:8080 --lat 48.0 --lon 7.85   # or the sim's --weather live
```

Nothing has touched R2 at this point, and nothing has to.

**C2 — Deploy the binary.** Build on the box or copy one in (§2). The PATH line above is not
optional advice: `cargo` lives in `$HOME/.cargo/bin` and is not on root's default PATH, so a
rebuild without it fails with `cargo: command not found` and nothing else.

```sh
sudo cp /usr/local/bin/obc-wx-bake /usr/local/bin/obc-wx-bake.prev    # your rollback, C9
export PATH="$HOME/.cargo/bin:$PATH"
sudo ops/weather/install.sh --from-source develop      # or --binary /tmp/obc-wx-bake
```

**C3 — Re-run `install.sh` and let it retire the old timers.** The command above already did; if
you deployed the binary by hand, run it now. One run does both halves: it installs
`obc-wx-bake@cycle.timer` from `adapters.conf`'s single row, and disables and removes every
per-adapter timer that row replaced.

`install.sh` **refuses to run against a binary that predates #1246**, and that check is
load-bearing rather than belt-and-braces. `cycle` is also the name of a subcommand the *old* binary
had — one that baked four v1 products — so the installer's usual "skip a subcommand this binary does
not know" probe cannot tell a misordered cutover from a correct one, and a misordered run would
quietly replace every per-adapter timer with a v1 multi-product cycle on a 15-minute timer. It
therefore tests for a subcommand only the old binary has (`dwd-rv`) and dies with the reason if it
finds one. If you see that message, you skipped C2.

Verify the timer set is exactly one row:

```sh
systemctl list-timers --all 'obc-wx-bake@*'    # expect obc-wx-bake@cycle.timer, *:0/15, and nothing else
```

**C4 — Watch the first real cycle.**

```sh
sudo systemctl start obc-wx-bake@cycle.service      # ALWAYS through the unit — never `obc-wx-bake cycle --r2`
sudo journalctl -u obc-wx-bake@cycle.service -n 80 --no-pager
```

A healthy first publish prints `publishing to r2 bucket obc-wx via https://<account>.r2…`, a line
per mosaic source, then `published … objects / … bytes`. There is **no** `retired …` line on the
first three cycles and that is correct: nothing has fallen off the retention chain yet.

Two things about that command are worth knowing before you type it anywhere else in this runbook.
**Start bakes through the unit, always.** The `flock` that stops two cycles overlapping lives in the
unit's `ExecStart`, not in the binary, so a bare `obc-wx-bake cycle --r2` runs outside it — and the
loser of that race republishes an older manifest over a newer one, naming generations the newer
cycle's sweep already deleted. The baker refuses to publish a manifest older than the one at the key,
so the mistake costs a tick rather than an outage, but the lock is what stops it arising.
**And a hand-started bake trips the cadence guard once** (§5): it stamps whatever phase of the
15-minute step you happened to run it at. That is a true statement about the live manifest, and it
clears itself on the next scheduled tick.

**C5 — Verify the first generation on R2, by hand.** These are §4's checks against the new tree; do
them now rather than trusting the journal.

```sh
BASE=https://wx.openbikecomputer.com
curl -sI $BASE/wx/v2/manifest.json                       # 200, cache-control: public, max-age=60, must-revalidate
curl -s  $BASE/wx/v2/manifest.json | head -c 400         # "version": 2, a generation, key_prefix wx/v2
GEN=$(curl -s $BASE/wx/v2/manifest.json | sed -n 's/.*"generation": "\([^"]*\)".*/\1/p' | head -1)
curl -sI $BASE/wx/v2/$GEN/f0/s0-0.obcg                   # 200, cache-control: …immutable
curl -s -r 0-127 -o /dev/null -w '%{http_code} %{size_download}\n' $BASE/wx/v2/$GEN/f0/s0-0.obcg
                                                          # 206 128 — Range is all corridor extraction is
```

`s0-0` is the shard to check because it is the one guaranteed to exist in every generation: shard
row 0 reaches below `covered_rows.start`, so it always holds no-data cells, and only an
*entirely dry* shard is omitted (`OBCG_Spec.md` §10.3).

**C6 — Move the clients.** The phone app and the simulator have been v2-only since WXR5 (#1244) —
there is no v1 reader left in this repository — so this step is releasing/installing those builds,
not editing anything. Until an installed app is updated it is still reading `wx/v1`, which is why
C7 comes after it and not before.

**C7 — Watch the sweep, then delete the v1 tree by hand.** Wait for **four** cycles (~45 minutes,
one full retention window). By then the fourth publish has retired the first generation, and the
journal says so:

```sh
journalctl -u obc-wx-bake@cycle.service --since -1h --no-pager | grep -E 'retired|retention sweep'
```

Expect one `retired <generation> (N objects)` line per cycle from the fourth on, and **no**
`retention sweep:` warning. Confirm from outside too — this is the check that the sweep is really
collecting rather than reporting that it did:

```sh
python3 ops/weather/freshness_probe.py --url $BASE     # expect "ok  swept: … is gone"
```

Only now delete the old tree. It is a few GB, it has no readers left, and **nothing automatic will
ever do this**: the baker's sweep only touches keys it published, under `wx/v2`, named by a manifest
it wrote. Load the environment exactly as in §4 step 5 (as root, and close the shell afterwards):

```sh
set -a; . /etc/obc-wx/r2.env; set +a
export RCLONE_CONFIG_OBCWX_TYPE=s3 RCLONE_CONFIG_OBCWX_PROVIDER=Cloudflare \
       RCLONE_CONFIG_OBCWX_REGION=auto \
       RCLONE_CONFIG_OBCWX_ENDPOINT="${OBC_WX_R2_ENDPOINT:-https://$OBC_WX_R2_ACCOUNT_ID.r2.cloudflarestorage.com}" \
       RCLONE_CONFIG_OBCWX_ACCESS_KEY_ID="$OBC_WX_R2_ACCESS_KEY_ID" \
       RCLONE_CONFIG_OBCWX_SECRET_ACCESS_KEY="$OBC_WX_R2_SECRET_ACCESS_KEY"

# 1. Re-prove the blast radius. This token must NOT be able to see any other bucket — it is the
#    only thing that bounds a slip of these commands to the weather bucket.
rclone lsd obcwx:obc-maps                    # MUST fail with AccessDenied/403. If it succeeds, STOP.

# 2. Look at what you are about to delete, twice, two different ways.
rclone size obcwx:obc-wx/wx/v1/              # note the object count and the total
rclone purge --dry-run obcwx:obc-wx/wx/v1/   # read the LAST line: same count, and every key under wx/v1/

# 3. Only if step 2's two counts agree and every path printed starts with `wx/v1/`:
rclone purge obcwx:obc-wx/wx/v1/
rclone lsd   obcwx:obc-wx/wx/                # expect only v2/
```

The dangerous mistake here is **not** a typo — it is a *shortened* prefix. `obcwx:obc-wx/wx/` is a
plausible thing to type and takes v2 with it, and a bare `rclone size` on it would print a
believable number rather than an obvious error. That is what the `--dry-run` is for: it names keys,
and `wx/v2/…` appearing in that output is unmissable in a way a byte total is not. The `lsd` in step
1 is the outer bound — if the token can only see `obc-wx`, the worst a slip can reach is a tree the
baker rebuilds within one tick.

`purge` is the right verb here and the wrong one anywhere else: it is a prefix operation, typed by
a human, once, against a prefix nothing publishes to any more. The baker has no such operation and
must never grow one.

**C8 — Turn the alarm back on.** In the same *Variables* screen:

* set `OBC_WX_EXPECT_SOURCES` = `dwd-rv,mrms,opera-cirrus,opera-nimbus,hrrr,icon-eu,gfs` (every row
  of `source::MOSAIC_PRIORITY`);
* **delete** `OBC_WX_EXPECT` — it named v1 products and has nothing left to mean. The probe accepts
  and ignores `--expect` rather than crashing, so a forgotten variable is harmless, but leaving it
  is leaving a lie in the configuration;
* restore `OBC_WX_BASE_URL`.

Then run the workflow once by hand (*Actions → Weather freshness → Run workflow*) and read the
summary rather than waiting for the schedule.

**C9 — Rollback, and what it costs *the clients*, not the baker.** No published object has any
state in it, so the box is trivially reversible at every step — but that is the least interesting
half. What decides how expensive a rollback is, is which tree the phones in people's pockets are
reading, and that changes at **C6**:

| You are at | Rollback | Cost to riders |
| :-- | :-- | :-- |
| before C2 | nothing to undo | none |
| C2–C5 (v2 publishing, clients still on v1) | `cp /usr/local/bin/obc-wx-bake.prev /usr/local/bin/obc-wx-bake`, then re-run `install.sh` **from the old checkout** — it reads `adapters.conf` from the tree it is run out of, so the old per-adapter rows come back with it | **none.** Every shipped client is still reading `wx/v1`, which never stopped being published. The orphaned `wx/v2` tree is collected by the 1-day lifecycle rule (T4); nothing sweeps it, because nothing publishes to it |
| **after C6** (clients updated), v1 still there | the same command | **a weather outage for everyone who updated**, lasting until you roll forward — not one tick. Their build reads `wx/v2` only (v2-only since WXR5), and the rolled-back baker does not publish it. Riders see MET's hourly data and **WEATHER UPDATE NEEDED**, never wrong weather, but they see it for as long as the rollback lasts |
| after C7, v1 deleted | the same command. The old baker is stateless: it finds no `wx/v1/manifest.json`, rebakes from upstream and republishes the tree within one tick of each timer | the same outage as the row above, and **the deletion adds nothing to it** — there is no state in that tree to lose, only one tick to rebuild it. The people *not* affected are the ones who never updated, which after C6 is the minority |

**C6 is the one-way-ish boundary**, and it is the step to be sure about — not C8's variable edit,
which is one-way only in the sense that you have to remember to redo it, and not C7's deletion,
which costs a tick. Roll forward rather than back once clients have moved: the fix for a bad v2
baker is a better v2 baker.

---

## 1. Things only the account owner can do

These need Timo's Hetzner/Cloudflare/GitHub logins. Everything else in this runbook is a script.
Do them in order; the whole list is ~20 minutes.

**T1 — Create the VPS.** Hetzner Cloud (or equivalent), **CX22-class** (2 vCPU / 4 GB / 40 GB),
Debian 12 or 13, region Nuremberg/Falkenstein (near DWD), **SSH key only, no password**. IPv4 is
worth its small surcharge: not every upstream (NOAA in particular) is reachable over IPv6-only.
4 GB of RAM is chosen so `--from-source` can compile the baker on the box; a 2 GB plan works for a
prebuilt binary but will thrash `cargo build`. Nothing else ever runs on this machine.

**T2 — Harden the login.** After first boot: `apt update && apt full-upgrade`, confirm
`PermitRootLogin prohibit-password` and `PasswordAuthentication no` in `/etc/ssh/sshd_config`.
`install.sh` enables `unattended-upgrades` for you if it is not already on.

**T3 — Create the R2 bucket.** Cloudflare dashboard → R2 → *Create bucket* → name **`obc-wx`**,
storage class **Standard** (*not* Infrequent Access: weather expires within a day and IA bills a
minimum storage duration — WX1's frozen decision). It must be a **separate bucket** from map
hosting so lifecycle rules can never collide.

The dialog's *Location* control offers two things that look alike and are not interchangeable:

* an **automatic location hint** — *EU* here — is a placement *preference*. The bucket still lives
  on the account's default S3 endpoint, `https://<account>.r2.cloudflarestorage.com`. Prefer this:
  it is what the baker derives from the account id, so nothing else needs configuring.
* a **jurisdiction** (*European Union*, *FedRAMP*) is a data-residency *guarantee*, and it moves
  the bucket onto its own endpoint — `https://<account>.eu.r2.cloudflarestorage.com`. A
  jurisdiction bucket is simply not visible from the default endpoint, so a correct, correctly
  scoped token gets **403 AccessDenied** on every object. That is exactly how the first real
  deploy failed (2026-08-09).

A jurisdiction bucket is fine to keep — it needs one extra line in `r2.env` naming the endpoint
(**T5**, §3), and nothing else in the service changes.

**T4 — Lifecycle rule, as a backstop.** Bucket → *Settings* → *Object lifecycle rules* → add a rule:
prefix `wx/`, **delete objects 1 day after upload** (24 h). Verify it for real in §4.

**This is no longer the mechanism, and the demotion is the point** (WXR8 #1247). Retention is the
baker's own sweep: after each successful publish it deletes the generation its new manifest no
longer names, keeping current plus two — about 45 minutes of overlap at a 15-minute cadence, which
is what a client that fetched the manifest just before a swap and is still Range-reading needs. A
lifecycle rule cannot express that: R2's granularity is whole days, and expiry is lazy ("typically
removed within 24 hours of the expiration value"), so the shortest rule the control can state
really means up to ~2 days. 24 h was never a retention decision; it was the shortest thing the
setting could say.

What the rule is still for is **leaks the sweep cannot see**: objects from a cycle that died
between publishing and swapping the manifest, and objects of a generation baked under a different
shard grid. Both are unreferenced by any manifest, so no manifest-driven sweep can name them, and
both are exactly what a lazy day-granularity rule is good at. Keep it. Do not shorten it below 24 h
— see §8's "not fetchable" row — and do not treat it as the storage bound: with the sweep working,
the bucket holds ≈ 44 MB (§6), not a day of frames.

**T5 — Scoped API token.** R2 → *Manage API tokens* → *Create API token*:
permission **Object Read & Write**, **specify bucket = `obc-wx` only** (never "all buckets"), no
TTL or a long one you will remember to rotate. Copy the **Access Key ID**, **Secret Access Key**
and your **Account ID**. Paste them into `/etc/obc-wx/r2.env` on the box (§3), nowhere else.

If **T3** left you with a **jurisdiction** bucket, add its endpoint to that same file — the account
id alone cannot tell the baker where the bucket lives:

```sh
OBC_WX_R2_ENDPOINT=https://<account>.eu.r2.cloudflarestorage.com
```

The bucket's own *Settings* page shows the host to use as its **S3 API** address; copy the origin
from there rather than assembling it from memory. With a plain location hint, leave the line out —
the derived `https://<account>.r2.cloudflarestorage.com` is correct. The by-hand `rclone` recipe in
§4 step 5 — and everything that refers back to it — reads the same value out of `r2.env`.

**T6 — Public read + a hostname.** R2 bucket → *Settings* → *Public access*: connect a **custom
domain** (e.g. `wx.openbikecomputer.com`) — that is the origin the phone and the simulator fetch
from, and it gives Cloudflare caching and a stable name independent of the account. (The
`*.r2.dev` development URL is rate-limited and is not a production origin.) Add DNS if the
dashboard does not do it for you.

**T7 — CORS.** Bucket → *Settings* → *CORS policy*. The iOS app uses `URLSession` and needs
nothing, but the browser hosts do (`obc-sim` live mode is native, the web demo and any in-browser
tooling are not), and a missing CORS header is the exact class of bug that already bit the map
delivery path. Allow read-only cross-origin GETs:

```json
[{ "AllowedOrigins": ["*"],
   "AllowedMethods": ["GET", "HEAD"],
   "AllowedHeaders": ["range", "if-none-match", "cache-control"],
   "ExposeHeaders": ["etag", "content-length", "content-range", "cache-control"],
   "MaxAgeSeconds": 3600 }]
```

`Range` and `ETag` must be there: corridor extraction is a header read + a page read + a few tile
reads over `Range`, and the manifest short-circuits on `ETag`.

**T8 — Point the alarm at it.** GitHub → repo → *Settings* → *Secrets and variables* → *Actions* →
*Variables* → new repository **variable** `OBC_WX_BASE_URL` = `https://wx.openbikecomputer.com`
(no trailing slash, no `/wx/v2`). Until this exists, `wx-freshness.yml` reports "skipped" and
passes — a project without a deployed service must not carry a red check about it. There is **no
secret** to add: the probe reads a public object.

Add a second variable, `OBC_WX_EXPECT_SOURCES` = every row of `source::MOSAIC_PRIORITY`
(`dwd-rv,mrms,opera-cirrus,opera-nimbus,hrrr,icon-eu,gfs`) — the sources that must appear in the
manifest's `attribution[]`. Be exact about what it can see. It does **not** catch an upstream
outage: a source whose provider is down stays listed and the mosaic falls through to the next
priority row, which is the designed behaviour and not an alert. What it catches is a **deploy that
went backwards** — an older binary, or a build with a source dropped from the table — silently
coarsening the dataset over whole regions while every freshness check stays green. There is no
dead-timer variable any more, because with one dataset on one timer "the manifest is fresh" and
"the timer is alive" are the same sentence: the timer that would be dead is the one that writes the
manifest.

**T9 — Once a month.** Check the Cloudflare R2 usage page and the VPS invoice against §6. **Look at
stored bytes specifically**: the sweep is the only thing keeping that number at tens of MB, and a
sweep that quietly stopped working looks identical to a healthy service from the manifest. The
probe's own sweep check (§5) is the other witness. Also glance at the Actions tab: GitHub disables
scheduled workflows in a repository with 60 days of no activity, and this repository's alarm is a
scheduled workflow.

---

## 2. Build the binary

Either build it on the box (simplest, needs the 4 GB plan):

```sh
sudo ops/weather/install.sh --from-source develop      # clones to /opt/obc-wx/src, cargo build --release
```

or build it wherever you like and copy it over — it must be a **Linux binary for the box's
architecture** (`x86_64` for CX-class Intel/AMD, `aarch64` for Hetzner's CAX ARM plans):

```sh
# on a Linux host / container with this repo:
cargo build --release --locked -p obc-wx-bake
scp target/release/obc-wx-bake root@wx:/tmp/
```

`obc-wx-bake` links libc dynamically; build it on the same Debian release as the box (or use a
`musl` target) or it will not start. A macOS workstation cannot cross-build it usefully — use the
box or a Debian container.

## 3. Install

```sh
scp -r ops root@wx:/root/          # or git clone the repo on the box
ssh root@wx
sudo /root/ops/weather/install.sh --binary /tmp/obc-wx-bake
```

The installer is idempotent — run it again after any change to `adapters.conf`, the unit template
or the binary. It:

* installs `rclone`, `flock`, `curl` and `unattended-upgrades` if missing;
* creates the `obc-wx` system user (no shell, no home) and `/var/lib/obc-wx`;
* writes `/etc/obc-wx/r2.env` **only if it does not exist**, at 0600 root:root, and never rewrites
  it — your credentials survive every upgrade;
* **refuses outright if the installed binary predates #1246** (it tests for the `dwd-rv` subcommand
  only the old multi-product binary has). See the cutover's C3 for why that check exists and why it
  must not be removed to "unblock" a deploy;
* installs the unit template and generates one timer per row in `adapters.conf` — one row, `cycle`
  — **skipping any row this binary does not know**, which is what lets a row land in the table
  before the binary that answers to it is deployed;
* removes and disables timers for rows that left the table, which is how the per-adapter timers
  retire themselves on the cutover run;
* caps the journal at 200 MB / 1 month;
* finishes with a dry-run bake (fetch + decode, publishes nothing) so a broken upstream or a wrong
  architecture is visible immediately.

**Whatever rclone the distribution packages is fine — there is no version floor — and the reason
that is true is worth one line.** Debian 13 ships **rclone v1.60.1**, and that version answers both
`rclone cat` and `rclone size --json` for a key that *does not exist* with an empty body and exit 0,
writing nothing to stderr. Absence cannot be read off an error message there, so the baker reads the
`count` field of `rclone size --json` instead: `0` is no object, `1` is an object — including a real
zero-byte one, which prints the same `"bytes":0`. Every rclone that has `--json` prints `count`,
which is why this works everywhere rather than pinning a version.

That is also a rule for anyone editing `publish.rs`: **absence is decided by `count`, never by a
message.** A baker that gets it wrong cannot bootstrap a fresh `wx/v2` prefix at all — it reads the
empty body back as a present-but-unparseable manifest, refuses to publish over it (correctly, per
§10.4), and does that on every tick forever. Observed live on 2026-08-11; the regression test is
`publish::tests::a_missing_object_reads_as_absent_not_as_an_empty_body`.

Then fill in the credentials from **T5** and take the first real publish by hand:

```sh
sudo nano /etc/obc-wx/r2.env          # OBC_WX_R2_ACCOUNT_ID / ACCESS_KEY_ID / SECRET_ACCESS_KEY,
                                      # plus OBC_WX_R2_ENDPOINT for a jurisdiction bucket (T3/T5)
sudo systemctl start obc-wx-bake@cycle.service   # through the unit — §7 says why never by hand
sudo journalctl -u obc-wx-bake@cycle.service -n 60 --no-pager
```

A healthy first publish looks like `publishing to r2 bucket obc-wx via https://<account>.r2…`, a
`#<rank> <source>: N source frames` line per mosaic source, then
`published … objects / … bytes (… dry shards omitted); …ms`. A `retired …` line appears from the
fourth cycle on, never before it — there is nothing off the retention chain until then.

If instead every object comes back **403 AccessDenied** with a token you just minted, read the
endpoint in that first line before suspecting the token: a jurisdiction bucket answers only at
`https://<account>.eu.r2.cloudflarestorage.com`, and uncommenting `OBC_WX_R2_ENDPOINT` in `r2.env`
is the whole fix (**T3**/**T5**). A wrong or mis-scoped token gives the same 403, so check the
cheap thing first.

## 4. Verify the deployment (do all seven)

1. **The manifest is public and cacheable.**
   `curl -sI https://wx.openbikecomputer.com/wx/v2/manifest.json` → `200`,
   `cache-control: public, max-age=60, must-revalidate`, `content-type: application/json`.
2. **A shard object is public and immutable.**
   Object keys are arithmetic, not a field: `wx/v2/<generation>/f<offset>/s<col>-<row>.obcg`
   (`OBCG_Spec.md` §10). `curl -sI` the `f0/s0-0.obcg` of the current generation → `200`,
   `cache-control: public, max-age=31536000, immutable`. `s0-0` is the shard guaranteed to exist in
   every generation — it reaches below `covered_rows.start`, so it always holds no-data cells, and
   only an entirely *dry* shard is omitted.
3. **Range reads work** (corridor extraction depends on them):
   `curl -s -r 0-127 -o /dev/null -w '%{http_code} %{size_download}\n' <object-url>` → `206 128`.
4. **The probe is green.**
   `python3 ops/weather/freshness_probe.py --url https://wx.openbikecomputer.com`
5. **The lifecycle rule is real, not assumed.** It is a backstop now (T4), not the storage bound,
   but an unverified backstop is not one. Drop a throwaway object under the same prefix and check
   the next day that R2 removed it. There is no config file on the box: the baker builds its
   rclone remote entirely from the environment, so do the same by hand (as root — the env file is
   root-only — and close the shell afterwards):
   ```sh
   set -a; . /etc/obc-wx/r2.env; set +a
   export RCLONE_CONFIG_OBCWX_TYPE=s3 RCLONE_CONFIG_OBCWX_PROVIDER=Cloudflare \
          RCLONE_CONFIG_OBCWX_REGION=auto \
          RCLONE_CONFIG_OBCWX_ENDPOINT="${OBC_WX_R2_ENDPOINT:-https://$OBC_WX_R2_ACCOUNT_ID.r2.cloudflarestorage.com}" \
          RCLONE_CONFIG_OBCWX_ACCESS_KEY_ID="$OBC_WX_R2_ACCESS_KEY_ID" \
          RCLONE_CONFIG_OBCWX_SECRET_ACCESS_KEY="$OBC_WX_R2_SECRET_ACCESS_KEY"

   date -u; echo lifecycle probe | rclone rcat obcwx:obc-wx/_lifecycle-probe.txt
   rclone lsl obcwx:obc-wx/_lifecycle-probe.txt                # it exists now
   ```
   Set a reminder for ~26 h later and run the `lsl` again: it must report *not found*. (An S3-API
   `HEAD` may also show an `x-amz-expiration` header the same day — a useful hint, not the proof.)
   Note the key is deliberately **outside** `wx/v2/`: the baker's sweep only ever deletes keys it
   composed itself, so a probe object under a generation prefix would sit there confusing a later
   reader rather than being collected.
6. **The retention sweep is collecting.** Four cycles in (~45 min), the journal must show one
   `retired <generation> (N objects)` line per cycle and no `retention sweep:` warning, and the
   probe must print `ok  swept: <generation> is gone`. This is the check that the newest failure
   mode in the service — a sweep that silently stops — has a witness: everything else about a
   non-sweeping service looks perfectly healthy until the bill.
7. **The token cannot touch the map buckets.** With the same environment loaded, a read of the maps
   bucket must fail:
   ```sh
   rclone lsd obcwx:obc-maps        # ← the maps bucket name; MUST fail with AccessDenied/403
   ```
   A success here means the token was created for "all buckets" — delete it and redo **T5**. Note
   the credentials only ever reach rclone through the environment, never through `argv`; that is
   the same rule the baker follows, and it is why none of these commands put a secret in your shell
   history or in `ps`. It matters more than it did: the token can now delete, so its blast radius
   is the one bucket it is scoped to.

Then let the timer run: `systemctl list-timers 'obc-wx-bake@*'`.

## 5. Freshness alerting

`.github/workflows/wx-freshness.yml` fetches `wx/v2/manifest.json` every 15 minutes from GitHub's
infrastructure and alerts when:

* the manifest itself is older than **30 min** — a healthy tick republishes it even when every
  upstream is unchanged, so this is the heartbeat *and* the dead-timer check. With one dataset on
  one timer those stopped being separable questions: the timer that would be dead is the one that
  writes the manifest; or
* the document's own deadlines have passed by more than the **15 min** grace —
  `freshness.next_generation_expected_at` (the service is late, the data is still usable) or
  `freshness.stale_after` (the generation can answer nothing at all); or
* a frame's presence bitmap disagrees with its shard list — clients refuse such a frame
  (`OBCG_Spec.md` §10.3), so the rider silently loses it and this is the only place that can see it
  happening to everyone at once; or
* a source named by `OBC_WX_EXPECT_SOURCES` is missing from `attribution[]` (a deploy that went
  backwards — see T8); or
* the published set or the retained footprint breaks the cost guards (§6); or
* the cycle started more than **7 min** into its own 15-minute step — only a timer running faster
  than the cadence can do that, and it is the one expensive mistake storage cannot see (§6); or
* a generation the manifest no longer names is **still fetchable** — the retention sweep has
  stopped collecting; or
* the manifest is unfetchable/unparseable.

Every threshold that could be read out of the document is read out of the document. What is left
local is the two grace windows, the two cost gates and the cadence guard, and each of those carries
its derivation in a comment beside it.

The alert is **one GitHub issue** labelled `wx-freshness` — opened on the first bad probe (GitHub
e-mails you), left alone while the outage lasts, then commented and closed automatically on the
first good probe. No pager, no second notification, no manual cleanup.

That one-notification promise rests on a baker rule worth knowing: **an outage never publishes a
half-dataset.** A cycle either publishes a complete generation or publishes nothing and leaves the
previous one standing, so the probe keeps saying the same thing until the service genuinely
recovers rather than flapping as sources come and go.

Run it by hand any time:

```sh
python3 ops/weather/freshness_probe.py --url https://wx.openbikecomputer.com
python3 ops/weather/freshness_probe.py --url https://wx.openbikecomputer.com --expect-sources dwd-rv,mrms,gfs
python3 ops/weather/freshness_probe.py --manifest ./manifest.json --now 2026-08-09T18:00:00Z --json
```

Exit codes: `0` fresh, `1` stale / over budget / not sweeping, `2` unreachable.

`--manifest` skips the sweep check, because a local document has no origin to ask. `--mosaic` and
`--expect` are accepted and ignored: they were the two-tree window's flags, and the probe declining
to crash on a stale `OBC_WX_PROBE_ARGS` is worth more than the tidiness of removing them. The probe
has self-tests — `python3 -m unittest discover -s ops/weather/tests` — and CI runs them, because an
unattended alarm's cases are the only thing standing between it and confident silence.

### Drill it (WX15 gate, epic closeout)

The drill must prove the *alert path*, not the probe's arithmetic:

1. `sudo systemctl stop obc-wx-bake@cycle.timer`
2. Wait past the manifest heartbeat — 30 minutes, so ~35 covers it. (Impatient variant, proves the
   same path without waiting: run the workflow manually via *Actions → Weather freshness → Run
   workflow* with `probe_args = --now <three hours from now>`.)
3. Expect a new issue "Weather service: manifest stale" and its e-mail. Check the device story at
   the same time: it must say **WEATHER UPDATE NEEDED**, never "dry".
4. `sudo systemctl start obc-wx-bake@cycle.timer`, then `sudo systemctl start
   obc-wx-bake@cycle.service` to recover immediately (through the unit — §7 says why). Expect one
   `CADENCE` alert on the next probe; it clears on the following scheduled tick.
5. The next scheduled probe comments and closes the issue. **No R2 surgery is needed to recover** —
   if any was, that is a bug, not an operational step.

## 6. Cost

The ceiling is **€10/month all-in**; WX1 froze the split at ≤ €7 for compute with R2 expected to sit
inside the free tier. The measured inputs are WXR1's (#1254), taken on the published lattice —
36,000 × 18,000 cells, 24 shards × 9 frames, tile edge 256, per-tile deflate:

| Quantity | Measured | Source |
| :-- | --: | :-- |
| Published per **wet global** cycle | **14.69 MB** | #1254 (43.60 MB at tile edge 64 — the reason the edge is 256) |
| Objects per cycle | ≤ 216 | 24 shards × 9 frames, minus every all-dry shard |
| Largest single object | 1.92 MB | #1254, wet, tile 256 |
| Cycle wall time | 12.4 s (+1.0 s LZ) on 8 cores | #1254; the 4-vCPU box is slower, the budget is 300 s |
| Peak RSS | ≈ 398 MB | #1254 at `BAKE_THREADS = 4` |

14.69 MB is the **worst case, not the average** — a real cycle omits every shard whose cells are all
dry. Everything below is stated against it anyway.

Projected steady state, one dataset at 96 cycles/day, retention = current + 2 generations:

| Line | Amount | Against |
| :-- | --: | :-- |
| **R2 storage** (the sweep's doing, not the lifecycle rule) | **≈ 44 MB** | 10 GB free → **$0** |
| New objects written/day | ≈ 1.4 GB | — (written and retired; only 44 MB is ever resident) |
| **R2 class A** (writes: ≤ 216 objects + 1 manifest per cycle) | **≈ 633 k/month** | 1 M free → **$0**, and this is the tightest line in the whole budget |
| **R2 class B** (the baker's `head` per object + one manifest `get`, plus rider reads) | ≈ 0.7 M/month | 10 M free → **$0** |
| **R2 `DeleteObject`** (the sweep: ≤ 216/cycle) | ≈ 0.6 M/month | **free operation** → **$0** |
| **R2 egress** | any | free on R2 → **$0** |
| **VPS ingress** (seven upstreams a cycle) | ≈ 45 GB/month | 20 TB included → **$0** |
| **VPS** | CX22-class, ≈ €4–5/month gross | ≤ €7 gate |
| **Total** | **≈ €4–5/month** | ceiling €10 → ≥ €5 margin |

Two rows deserve a sentence each.

**Storage stopped being the interesting number.** The old architecture held a rolling day of frames
— 3–24 GB depending on how many products were live — and the whole cost conversation was about the
lifecycle window. The sweep replaced that with a number three cycles wide: ≈ 44 MB, 0.4 % of the
free tier. Nothing in the budget is storage-constrained any more, which is exactly why T9's monthly
glance at *stored bytes* matters: if that figure is growing, the sweep is broken, and no other line
in this table will tell you.

**Class A writes became the constraint.** Every cycle mints a full set of immutable objects — a
mosaic cell can come from any source, so there is no per-source short-circuit left to skip one — and
217 writes × 96 cycles/day is ≈ 633 k of the 1 M free monthly operations. That is 1.6× headroom, and
it is why the probe carries a **cadence guard**: a timer edited to `*:0/5` does not mint more
generations (they anchor on the quarter hour, so it re-bakes the same one) and does not grow
storage, but it triples the writes and lands at ≈ 1.9 M — the first real bill this service would
ever produce. Storage cannot see that mistake; `generated_at − reference_time` can, from a single
sample, and 7 minutes into a 15-minute step is a phase a correct timer (60 s randomized delay)
cannot reach.

The probe therefore carries exactly **one** storage gate: **30 MB** for the published set, ≈ 2× the
wet-global measurement, catching a dataset that *grew* — a codec regression, an adapter painting
noise into cells that should be dry. The obvious second gate, retained bytes against ~90 MB, was
written and then removed: `retained = set × (1 + len(previous))` and §10.4 caps `len(previous)` at
2, so it is bounded by 3 × the published set *by construction* and cannot fire without the set gate
firing first. It was a restatement of the same measurement wearing a second gate's clothes, and a
gate that arithmetically cannot fire on its own is one people learn to ignore. The retained figure
is still printed, because it is the number the bucket actually holds.

What no arithmetic over a manifest can see is a sweep that stopped — the retained figure is a
projection of what the bucket holds *if the sweep is working*. Two things can see it, and both are
real checks rather than gates: the probe's sweep witness (one `HEAD` against a generation the
manifest no longer names, §5) and T9's monthly look at R2's own stored-bytes figure.

Record the **actual** metered numbers here after the first full month (an epic closeout item):

```
first metered month: ____________  VPS €____  R2 $____  total €____
stored bytes at month end: ____ MB  (expect ≈ 44 MB; anything in GB means the sweep stopped)
upstream fetched per cycle: ____ MB (unmeasured for the seven-source mosaic; read it off a journal line)
```

**If a cost guard fires**, in order of what it most likely is:

1. **The cadence guard** — read `systemctl list-timers 'obc-wx-bake@*'` against `adapters.conf`.
   Someone edited the timer, or a second timer is installed. This is the only one that costs money
   quickly.
2. **The sweep check** — grep the journal for `retention sweep:` warnings. A persistent store error
   there means storage is growing by a full object set every cycle and only the 1-day lifecycle rule
   (T4) is bounding it. Confirm the rule still exists before doing anything else.
3. **The set gate** — the dataset grew. A wet global cycle is 14.7 MB; twice that is not weather,
   it is a regression. Compare the per-frame byte counts in `--json` output against the table above,
   and suspect the codec or an adapter quantizing noise into cells that should be dry.
4. Only then consider trading cadence away. Dropping to `*:0/30` halves the writes and costs 15
   minutes of radar freshness, which is the whole point of the product; it is the last lever, not
   the first.

## 7. Routine operations

**Read the logs.**

```sh
systemctl list-timers 'obc-wx-bake@*'                 # when the cycle last ran and next runs
journalctl -u obc-wx-bake@cycle.service -n 100 --no-pager     # everything, newest last
journalctl -u obc-wx-bake@cycle.service -f                    # follow it
journalctl -u obc-wx-bake@cycle.service -p err --since -24h   # only failures
journalctl -u obc-wx-bake@cycle.service --since -6h | grep -E 'retired|retention sweep'
systemctl show -p NRestarts -p ExecMainStatus obc-wx-bake@cycle.service
```

Every tick prints its own report: one line per mosaic source with its priority rank and how many
frames it contributed, bytes fetched, objects published, dry shards omitted, elapsed ms, then —
from the fourth cycle on — a `retired <generation> (N objects)` line, then any warnings.

**Run a bake by hand only as `systemctl start obc-wx-bake@cycle.service`, never as a bare
`obc-wx-bake cycle --r2`.** The `flock` that serializes cycles is in the unit's `ExecStart`, not in
the binary: a hand-typed invocation runs outside it, and if it overlaps a timer tick the loser
republishes an older manifest over a newer one — a document naming generations the newer cycle's
sweep already deleted, which by `OBCG_Spec.md` §10.3 is an error for every client that falls back to
one. The baker refuses to publish a manifest older than the one already at the key, so the mistake
costs a tick and a journal line instead; the lock is what stops it arising at all. Expect the
cadence guard (§5) to fire once after any hand-started bake — it is stating a true thing about the
live manifest's phase, and it clears on the next scheduled tick.

**Upgrade the binary.** Rolling back is the same command with an older ref, and needs no R2 work:
the published objects a good release made stay valid, and the next tick of an older binary simply
republishes over them.

```sh
sudo systemctl stop obc-wx-bake@cycle.timer
sudo cp /usr/local/bin/obc-wx-bake /usr/local/bin/obc-wx-bake.prev    # your rollback
export PATH="$HOME/.cargo/bin:$PATH"                     # cargo is not on root's default PATH
sudo ops/weather/install.sh --from-source develop        # or --binary /tmp/obc-wx-bake
sudo systemctl start obc-wx-bake@cycle.service           # one bake in the foreground of the log
sudo systemctl start obc-wx-bake@cycle.timer
# rollback: sudo cp /usr/local/bin/obc-wx-bake.prev /usr/local/bin/obc-wx-bake && sudo systemctl start …
```

Rolling **back** across the retention change deserves one sentence: an older binary does not sweep,
so from the moment it takes over, every generation it publishes accumulates. The 1-day lifecycle
rule (T4) bounds that at a day's worth, which is what it used to bound anyway.

**Change the cadence.** Edit `adapters.conf` and re-run `install.sh`. Read §6's Class A row first:
the cadence is a write-rate decision now, not a storage decision, and the free-tier headroom is
1.6×.

**Add or retire a source.** Neither is an ops act any more — the mosaic's sources are a table in the
binary (`source::MOSAIC_PRIORITY`), not rows in a config file, because a mosaic cell can come from
any of them and there is nothing to select. Deploying a build with a different table is the whole
procedure. What *is* an ops act is keeping `OBC_WX_EXPECT_SOURCES` (**T8**) in step with it: add the
id before deploying a build that adds the source, remove it after deploying one that drops it, or
the probe alerts about the difference — which is precisely what that variable is for.

**Rotate the R2 token.** Zero-downtime, because credentials are only read at process start:

1. Cloudflare → R2 → *Manage API tokens* → create a **second** token, same `obc-wx`-only scope.
2. On the box: `sudo nano /etc/obc-wx/r2.env` (still 0600 root:root), paste the new pair.
3. `sudo systemctl start obc-wx-bake@cycle.service` and read the journal — a publish must succeed.
4. Only then delete the old token in the dashboard.
5. If the old token was ever pasted anywhere but that file, treat it as leaked and rotate again.

**Pause the service** (e.g. an upstream is broken and you want quiet):
`sudo systemctl stop obc-wx-bake@cycle.timer`. The published generation expires honestly; the
freshness alarm will fire, which is correct — silence it by closing the issue, not by disabling the
workflow.

## 8. When something breaks

The baker fails **closed**: any error anywhere publishes *nothing* and leaves the previous manifest
and its frames byte-identical. So a failing tick is never a corruption risk; it is only a freshness
risk. Wait for the next tick before intervening.

| Symptom | What it means | Do |
| :-- | :-- | :-- |
| One upstream is broken/changed | the mosaic falls through to the next priority row for the cells that source covered — coarser weather there, not missing weather | nothing on the box. Check the provider. There is no per-source isolation to restore: one dataset means one cycle, and that trade was made deliberately in #1246 |
| `every one of the … baked objects is entirely no-data … refusing to publish a blank cycle` | every source fell out of the skew window at once, or the global floor is gone | the previous generation still stands and is still served. Check the journal's per-source lines for which upstreams reported nothing |
| `rclone: … 403 / AccessDenied` | token wrong, expired, or scoped to another bucket — **or** the bucket has a jurisdiction and the baker is talking to the default endpoint | read the endpoint the journal prints: if the bucket is EU-jurisdiction, set `OBC_WX_R2_ENDPOINT` (**T3**/**T5**). Otherwise §7 rotate; re-check **T5** |
| `… is not fetchable — refusing to swap the manifest in` | an object this cycle just published is not readable back | the manifest is untouched and the previous generation still serves. If it repeats, suspect the lifecycle rule (**T4**) being shorter than a day, or R2 tearing a body |
| `retention sweep: N of generation … could not be deleted` | the sweep hit store errors. **The cycle succeeded** — those objects are unreferenced, nothing serves them | one occurrence is noise. Repeating means storage is growing by a set per cycle and only T4's rule bounds it: check the token still has write scope, and read §6's guard list |
| The probe says a generation is `still fetchable` | the sweep has stopped collecting entirely | grep the journal for `retention sweep:`; if there are no warnings at all, the sweep is not running — check the deployed binary is the one this runbook describes |
| The probe says a source is `MISSING` from `attribution[]` | not weather: the deployed binary's `MOSAIC_PRIORITY` does not have it | check what `install.sh` last deployed; roll forward, or drop the id from `OBC_WX_EXPECT_SOURCES` if the removal was deliberate |
| The probe says `CADENCE` | the timer fires faster than the dataset's cadence — **or somebody just ran a bake by hand**, which stamps whatever phase of the step they ran it at | if you (or a step in this table) started a bake in the last 15 minutes, that is this, and it clears on the next scheduled tick. Otherwise `systemctl list-timers 'obc-wx-bake@*'` against `adapters.conf`: this is the one mistake that reaches a bill (§6) |
| `refusing to publish a manifest that goes backwards` | this cycle is anchored earlier than the generation already published — a clock that stepped back, or a bake started outside the unit's `flock` while another was running | nothing was published and nothing was deleted; the live manifest still stands. `timedatectl` first, then check nobody is running `obc-wx-bake cycle --r2` by hand (§7). It clears itself on the next tick once the clock is right |
| `published but does not parse back` / `read back … not the … just written` | the manifest was written and the read-back did not match — a torn body on the way out | **the sweep did not run**, deliberately: nothing was deleted. The next tick republishes and re-verifies. If it repeats, the bucket or the endpoint is the problem, not the baker |
| Cycle killed, `MemoryMax` in the journal | the bake exceeded its cap; the budget is ≈ 398 MB at `BAKE_THREADS = 4` | that is a bug, not a tuning problem — file it; raise the cap in the unit template only with a measurement |
| Every tick logs `flock`/timeout | a bake is wedged holding the lock | `systemctl stop obc-wx-bake@cycle.service`, check `ps`, then `systemctl start obc-wx-bake@cycle.service` — through the unit, so the replacement takes the same lock |
| `wx/v2/manifest.json exists but did not parse … refusing to publish` | a torn or truncated read of the manifest the baker itself wrote | **do nothing.** This is the §10.4 rule working: publishing an empty retention chain from a torn read would delete the generations in-flight clients are reading. The next tick retries. **Unless it repeats on every single tick against a prefix that is genuinely empty** — a fresh `wx/v2`, or one just reset below — in which case it is not a torn read but the baker failing to tell "absent" from "empty", and §3's note on `rclone size --json count` is the fix |
| Timer "active" but nothing publishes | clock skew, or a paused system | `timedatectl` (NTP on?), then `systemctl start obc-wx-bake@cycle.service`. A clock that stepped *backwards* shows up as `refusing to publish a manifest that goes backwards` rather than as silence |
| Everything green on the box, probe says stale | delivery, not baking: public access, custom domain, DNS or a cached 404 | re-do §4 steps 1–3 |

**Wedged state, last resort.** The published manifest is the service's *only* state, and deleting it
is a full reset — with one consequence that did not exist before the sweep. Load the environment as
in §4 step 5, then `rclone deletefile obcwx:obc-wx/wx/v2/manifest.json` and
`systemctl start obc-wx-bake@cycle.service`.
This is the one operation that depends on the baker reading a *deleted* key as absent rather than as
empty (§3), so do it with a binary that postdates that fix or you have replaced a wedge with a worse
one. The next cycle sees no predecessor, treats itself as a bootstrap, and publishes a complete
generation with an empty `previous_generations` — which means **it will not sweep for three
cycles**, and the generations that were current when you deleted the manifest are now orphaned. They
are unreferenced, nothing serves them, and T4's lifecycle rule collects them within a day. That is
the whole cost. Do this only when the journal says the manifest itself is the problem.

**Total outage** (box gone, provider incident, invoice unpaid): do nothing urgent. R2 keeps serving
the last generation until its own `stale_after` — two hours past its reference time — after which
riders see hourly-only weather from MET, which the phone fetches directly and which does not depend
on this service at all. Nothing deletes those objects in the meantime: the sweep only runs as part
of a successful publish, so a dead baker is a baker that has also stopped deleting. Rebuild with §9
when convenient.

## 9. Rebuild from zero

Target: **under 30 minutes** from "no machine" to "publishing", and the epic requires doing it for
real at least once. Nothing here is stateful — there is no backup to restore, because there is
nothing on the box worth keeping except the contents of `r2.env`.

1. **T1/T2** — new VPS (5 min, mostly waiting for the image).
2. `apt update && apt full-upgrade && apt install -y git` (2 min).
3. `git clone https://github.com/timohueser/OpenBikeComputer.git && cd OpenBikeComputer` (1 min).
4. `sudo ops/weather/install.sh --from-source develop` (5–10 min: the cargo build dominates; use
   `--binary` with a prebuilt file to make it ~1 min).
5. Paste the R2 credentials into `/etc/obc-wx/r2.env` — the **only** secret, and the only thing
   worth keeping in your password manager (5 min including finding them). Keep `OBC_WX_R2_ENDPOINT`
   there too if the bucket has a jurisdiction (**T5**); it is not a secret, but forgetting it turns
   the rebuild into a wall of 403s.
6. `systemctl start obc-wx-bake@cycle.service`, read the journal (1 min).
7. Walk §4's seven checks (5 min; check 6, the sweep, needs four cycles — come back to it).
8. Point DNS at the new box only if you moved the hostname — normally you did not: the public
   origin is R2's custom domain, and it has no idea which machine writes to the bucket. **The
   rebuild needs no DNS change and no client-visible change at all.**

Record the wall-clock time here when it is done for real: `rebuild timed: ____ min on ____`.

## 10. Secrets and privacy

* The R2 credential exists in exactly two places: Cloudflare, and `/etc/obc-wx/r2.env` (0600
  root:root — systemd reads it as root before dropping to the `obc-wx` user, so the service account
  cannot read its own secret off disk). Never in git, never in a shell history, never in a CI
  secret — this repository's CI does **not** publish weather.
* The baker never puts a credential in `argv` (rclone is configured through the child environment)
  and redacts the secret from any output it forwards. Both are pinned by tests in
  `host/obc-wx-bake/src/publish.rs`.
* The service receives **no rider coordinate, no user identifier and no per-user request**. It
  publishes static objects; clients read them. The only third party that ever sees a coordinate is
  MET Norway, called from the phone (WX1/WX4), not from here.
* Access logs: leave R2 access logging **off**. Nothing operational needs it, and switching it on
  would start collecting IP addresses this architecture is designed never to hold.
