# OBC weather service — runbook

Everything needed to build, run, watch, repair and rebuild the weather bakery. WX18 (#1206) of
epic #1185.

The service is **one stateless publisher**: a small VPS runs `obc-wx-bake` on systemd timers, each
tick fetches upstream radar/model data, bakes OBCG frames, and swaps one `manifest.json` in R2.
Nothing else exists — no database, no accounts, no per-request compute, no rider coordinate ever
reaching it. That shape is what makes the failure story boring:

> If the box dies, R2 keeps serving the last objects. Frames carry their real timestamps, products
> carry a `staleness_deadline`, and the phone/device stop using an expired product — showing
> **WEATHER UPDATE NEEDED**, never "dry". An outage costs freshness, never truth. So the alarm is
> e-mail-grade, and lives on GitHub, outside the machine it watches.

| Where | What |
| :-- | :-- |
| `ops/weather/install.sh` | Idempotent installer: user, dirs, binary, units, journal caps |
| `ops/weather/adapters.conf` | The cadence table — the one place a schedule is defined |
| `ops/weather/systemd/obc-wx-bake@.service` | The hardened one-shot bake unit (`%i` = adapter) |
| `ops/weather/freshness_probe.py` | The external probe; also runnable by hand |
| `.github/workflows/wx-freshness.yml` | Runs the probe every 15 min, opens/closes one alert issue |
| `host/obc-wx-bake/` | The baker itself (WX5 #1215); `specs/OBCG_Spec.md` is its output contract |

On the box:

| Path | Owner / mode | What |
| :-- | :-- | :-- |
| `/usr/local/bin/obc-wx-bake` | root 0755 | the binary |
| `/etc/obc-wx/r2.env` | **root 0600** | R2 credentials — never in git, never world-readable |
| `/var/lib/obc-wx/` | obc-wx 0750 | state dir: only the `bake.lock` and dry-run scratch |
| `/etc/systemd/system/obc-wx-bake@.service` | root 0644 | the unit template |
| `/etc/systemd/system/obc-wx-bake@<adapter>.timer` | root 0644 | generated per adapter |

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
location hint EU, storage class **Standard** (*not* Infrequent Access: weather expires in ~48 h and
IA bills a minimum storage duration — WX1's frozen decision). It must be a **separate bucket** from
map hosting so lifecycle rules can never collide.

**T4 — Lifecycle rule.** Bucket → *Settings* → *Object lifecycle rules* → add a rule:
prefix `wx/`, **delete objects 2 days after upload** (48 h). This is the only thing keeping storage
bounded — nothing in the baker ever deletes an object. Verify it for real in §4.

**T5 — Scoped API token.** R2 → *Manage API tokens* → *Create API token*:
permission **Object Read & Write**, **specify bucket = `obc-wx` only** (never "all buckets"), no
TTL or a long one you will remember to rotate. Copy the **Access Key ID**, **Secret Access Key**
and your **Account ID**. Paste them into `/etc/obc-wx/r2.env` on the box (§3), nowhere else.

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
(no trailing slash, no `/wx/v1`). Until this exists, `wx-freshness.yml` reports "skipped" and
passes — a project without a deployed service must not carry a red check about it. There is **no
secret** to add: the probe reads a public object.

Add a second variable, `OBC_WX_EXPECT` = `dwd-rv,icon-eu` — the products that must be *listed*.
Freshness alone cannot see a dead timer: when an upstream breaks, its product stays listed and goes
visibly expired, but when a **timer** is disabled, renamed or never installed, nothing complains —
the remaining products keep publishing a perfectly fresh manifest without it. Keep this list in
step with the live rows of `adapters.conf` (so it grows when WX6 lands, and shrinks only when you
retire an adapter on purpose — §7).

**T9 — Once a month.** Check the Cloudflare R2 usage page and the VPS invoice against §6. Also
glance at the Actions tab: GitHub disables scheduled workflows in a repository with 60 days of no
activity, and this repository's alarm is a scheduled workflow.

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
* installs the unit template and generates one timer per adapter in `adapters.conf`, **skipping any
  adapter this binary does not know** (that is why the WX6 rows are harmless today and become live
  the moment a WX6 binary is installed and the installer is re-run);
* removes and disables timers for adapters that left the table;
* caps the journal at 200 MB / 1 month;
* finishes with a dry-run bake (fetch + decode, publishes nothing) so a broken upstream or a wrong
  architecture is visible immediately.

Then fill in the credentials from **T5** and take the first real publish by hand:

```sh
sudo nano /etc/obc-wx/r2.env          # OBC_WX_R2_ACCOUNT_ID / ACCESS_KEY_ID / SECRET_ACCESS_KEY
sudo systemctl start obc-wx-bake@dwd-rv.service
sudo journalctl -u obc-wx-bake@dwd-rv.service -n 60 --no-pager
```

A healthy first publish looks like `publishing to r2 bucket obc-wx via https://<account>.r2…`,
then `dwd-rv: baked 9 frames`, then `published 10 objects / … bytes; …ms`. Two seconds is normal.

## 4. Verify the deployment (do all six)

1. **The manifest is public and cacheable.**
   `curl -sI https://wx.openbikecomputer.com/wx/v1/manifest.json` → `200`,
   `cache-control: public, max-age=60, must-revalidate`, `content-type: application/json`.
2. **A frame is public and immutable.**
   Take a `frames[].key` from the manifest and `curl -sI` it → `200`,
   `cache-control: public, max-age=31536000, immutable`.
3. **Range reads work** (corridor extraction depends on them):
   `curl -s -r 0-127 -o /dev/null -w '%{http_code} %{size_download}\n' <frame-url>` → `206 128`.
4. **The probe is green.**
   `python3 ops/weather/freshness_probe.py --url https://wx.openbikecomputer.com`
5. **The lifecycle rule is real, not assumed.** Drop a throwaway object under the same prefix and
   check in two days that R2 removed it. There is no config file on the box: the baker builds its
   rclone remote entirely from the environment, so do the same by hand (as root — the env file is
   root-only — and close the shell afterwards):
   ```sh
   set -a; . /etc/obc-wx/r2.env; set +a
   export RCLONE_CONFIG_OBCWX_TYPE=s3 RCLONE_CONFIG_OBCWX_PROVIDER=Cloudflare \
          RCLONE_CONFIG_OBCWX_REGION=auto \
          RCLONE_CONFIG_OBCWX_ENDPOINT="https://$OBC_WX_R2_ACCOUNT_ID.r2.cloudflarestorage.com" \
          RCLONE_CONFIG_OBCWX_ACCESS_KEY_ID="$OBC_WX_R2_ACCESS_KEY_ID" \
          RCLONE_CONFIG_OBCWX_SECRET_ACCESS_KEY="$OBC_WX_R2_SECRET_ACCESS_KEY"

   date -u; echo lifecycle probe | rclone rcat obcwx:obc-wx/wx/v1/_lifecycle-probe.txt
   rclone lsl obcwx:obc-wx/wx/v1/_lifecycle-probe.txt          # it exists now
   ```
   Set a reminder for ~50 h later and run the `lsl` again: it must report *not found*. Until you
   have seen that, the rule is unverified and storage is unbounded. (An S3-API `HEAD` may also show
   an `x-amz-expiration` header the same day — a useful hint, not the proof.)
6. **The token cannot touch the map buckets.** With the same environment loaded, a read of the maps
   bucket must fail:
   ```sh
   rclone lsd obcwx:obc-maps        # ← the maps bucket name; MUST fail with AccessDenied/403
   ```
   A success here means the token was created for "all buckets" — delete it and redo **T5**. Note
   the credentials only ever reach rclone through the environment, never through `argv`; that is
   the same rule the baker follows, and it is why none of these commands put a secret in your shell
   history or in `ps`.

Then let the timers run: `systemctl list-timers 'obc-wx-bake@*'`.

## 5. Freshness alerting

`.github/workflows/wx-freshness.yml` fetches the manifest every 15 minutes from GitHub's
infrastructure and alerts when:

* the manifest itself is older than **30 min** (a healthy tick republishes it even when every
  upstream is unchanged — that is the heartbeat), or
* any product is past its own `staleness_deadline` + **15 min** grace, or
* a product named by `OBC_WX_EXPECT` is **not listed at all** (a dead timer, not a dead upstream —
  see T8), or
* the manifest is unfetchable/unparseable, or
* the published set or the projected 48 h rolling footprint breaks the cost guards (§6).

The alert is **one GitHub issue** labelled `wx-freshness` — opened on the first bad probe (GitHub
e-mails you), left alone while the outage lasts, then commented and closed automatically on the
first good probe. No pager, no second notification, no manual cleanup.

That one-notification promise depends on a baker rule worth knowing: **an outage never removes a
product from the manifest.** A stalled product stays listed and goes visibly expired, so the probe
keeps saying "stale" until the product genuinely recovers. (Dropping expired entries instead would
make the manifest flicker the product present/absent, flapping this issue open and closed — and it
would cost the product's own timer its upstream short-circuit, turning a provider outage into a
re-download loop against the provider. `specs/OBCG_Spec.md` §10 carries the rule.)

Run it by hand any time:

```sh
python3 ops/weather/freshness_probe.py --url https://wx.openbikecomputer.com --expect dwd-rv,icon-eu
python3 ops/weather/freshness_probe.py --manifest ./manifest.json --now 2026-08-09T18:00:00Z
```

Exit codes: `0` fresh, `1` stale / missing / over budget, `2` unreachable.

### Drill it (WX15 gate, epic closeout)

The drill must prove the *alert path*, not the probe's arithmetic:

1. `sudo systemctl stop 'obc-wx-bake@*.timer'`
2. Wait past the tightest deadline — RV is run + 30 min, so ~45 minutes covers deadline + grace.
   (Impatient variant, proves the same path without waiting: run the workflow manually via
   *Actions → Weather freshness → Run workflow* with `probe_args = --now <two hours from now>`.)
3. Expect a new issue "Weather service: manifest stale" and its e-mail. Check the device story at
   the same time: it must say **WEATHER UPDATE NEEDED**, never "dry".
4. `sudo systemctl start 'obc-wx-bake@*.timer'` and run one bake by hand to recover immediately.
5. The next scheduled probe comments and closes the issue. **No R2 surgery is needed to recover** —
   if any was, that is a bug, not an operational step.

## 6. Cost

The ceiling is **€10/month all-in**; WX1 froze the split at ≤ €7 for compute with R2 expected to
sit inside the free tier. Measured inputs (live cycle, #1215, plus the published fixture manifest):

| Quantity | Measured | Source |
| :-- | --: | :-- |
| Upstream fetched per full cycle | 7.10 MB | #1215 live run |
| Published per full cycle | 2.28 MB | #1215 live run |
| `dwd-rv` published set (9 frames) | 0.84 MB/run | fixture manifest |
| `icon-eu` published set (12 frames) | 1.59 MB/run | fixture manifest |
| Peak RSS | 52 MB | #1215 live run (budget: < 1 GB) |
| Cycle wall time | 2.0 s (0.31 s unchanged) | #1215 live run |

Projected steady state, today's two adapters (RV 288 runs/day, ICON 4 runs/day):

| Line | Amount | Against |
| :-- | --: | :-- |
| New objects/day | ≈ 250 MB | — |
| **R2 storage** (48 h rolling) | ≈ 0.5 GB | 10 GB free → **$0** |
| **R2 class A** (writes: frames + one manifest per tick) | ≈ 92 k/month | 1 M free → **$0** |
| **R2 class B** (the baker's own head/get checks + rider reads) | ≈ 0.3 M/month | 10 M free → **$0** |
| **R2 egress** | any | free on R2 → **$0** |
| **VPS ingress** | ≈ 30 GB/month | 20 TB included → **$0** |
| **VPS** | CX22-class, ≈ €4–5/month gross | ≤ €7 gate |
| **Total** | **≈ €4–5/month** | ceiling €10 → ≥ €5 margin |

With WX6's three adapters added (MRMS every 2 min is the expensive one) writes rise to roughly
150 k/month and storage to a projected ~1 GB — still $0 on R2, still inside the ceiling, but that
is when the rolling-window guard starts to matter. Record the **actual** metered numbers here after
the first full month (an epic closeout item):

```
first metered month: ____________  VPS €____  R2 $____  total €____
```

**If the budget guard fires**, in order of preference: (a) shorten the R2 lifecycle rule from 48 h
to 24 h — nothing usable is older than ~12 h anyway, and it halves storage; (b) drop the RV cadence
in `adapters.conf` from `*:0/5` to `*:0/10` (costs ≤ 5 min of radar freshness); (c) check that a
product did not accidentally grow — a full-domain wet RV frame is tens of kB, a *hundreds*-of-kB
frame means an adapter regression, not weather.

## 7. Routine operations

**Read the logs.**

```sh
systemctl list-timers 'obc-wx-bake@*'                 # when each adapter last ran and next runs
journalctl -u 'obc-wx-bake@*' -n 100 --no-pager       # everything, newest last
journalctl -u obc-wx-bake@dwd-rv.service -f           # follow one adapter
journalctl -u 'obc-wx-bake@*' -p err --since -24h     # only failures
systemctl show -p NRestarts -p ExecMainStatus obc-wx-bake@dwd-rv.service
```

Every tick prints its own report: what each product did (`baked` / `upstream unchanged` /
`not selected`), bytes fetched, objects published, elapsed ms, and any warnings.

**Upgrade the binary.** Rolling back is the same command with an older ref, and needs no R2 work:
the published objects a good release made stay valid, and the next tick of an older binary simply
republishes over them.

```sh
sudo systemctl stop 'obc-wx-bake@*.timer'
sudo cp /usr/local/bin/obc-wx-bake /usr/local/bin/obc-wx-bake.prev    # your rollback
sudo ops/weather/install.sh --from-source develop        # or --binary /tmp/obc-wx-bake
sudo systemctl start obc-wx-bake@dwd-rv.service          # one bake in the foreground of the log
sudo systemctl start 'obc-wx-bake@*.timer'
# rollback: sudo cp /usr/local/bin/obc-wx-bake.prev /usr/local/bin/obc-wx-bake && sudo systemctl start …
```

**Change a cadence / add an adapter.** Edit `adapters.conf`, re-run `install.sh`, and add the new
id to the `OBC_WX_EXPECT` variable (**T8**). Adding a WX6 adapter is *only* that — no firmware, no
app, no protocol release.

**Retire an adapter (deliberately).** Removing a product is an operator act, because an outage
never removes one — a stalled product stays listed and visibly expired, which is what keeps the
alarm honest. So retirement is three steps, in this order:

1. delete its row from `adapters.conf` and re-run `install.sh` (its timer is disabled and removed);
2. remove its id from the `OBC_WX_EXPECT` variable, or the probe will now alert that it is missing;
3. remove the entry itself: the surviving timers would otherwise carry it forward forever. Load the
   environment as in §4 step 5 and delete the manifest —
   `rclone deletefile obcwx:obc-wx/wx/v1/manifest.json` — then run one bake per remaining adapter.
   Each rebakes from upstream within its own next tick anyway; the retired product's frame objects
   age out on their own.

**Rotate the R2 token.** Zero-downtime, because credentials are only read at process start:

1. Cloudflare → R2 → *Manage API tokens* → create a **second** token, same `obc-wx`-only scope.
2. On the box: `sudo nano /etc/obc-wx/r2.env` (still 0600 root:root), paste the new pair.
3. `sudo systemctl start obc-wx-bake@dwd-rv.service` and read the journal — a publish must succeed.
4. Only then delete the old token in the dashboard.
5. If the old token was ever pasted anywhere but that file, treat it as leaked and rotate again.

**Pause the service** (e.g. an upstream is broken and you want quiet):
`sudo systemctl stop 'obc-wx-bake@*.timer'`. Products expire honestly; the freshness alarm will
fire, which is correct — silence it by closing the issue, not by disabling the workflow.

## 8. When something breaks

The baker fails **closed**: any error anywhere publishes *nothing* and leaves the previous manifest
and its frames byte-identical. So a failing tick is never a corruption risk; it is only a freshness
risk. Wait for the next tick before intervening.

| Symptom | What it means | Do |
| :-- | :-- | :-- |
| One adapter's unit fails, others fine | that upstream is broken/changed | nothing yet — its product stays listed and expires honestly while the others stay fresh. That decoupling is why the timers are per-adapter |
| `… carried past its staleness deadline … no longer verified` | a product has been stalled long enough to expire; it stays published so its expiry is visible, and its frames stop being proven fetchable | check that provider; nothing on the box to fix |
| `rclone: … 403 / AccessDenied` | token wrong, expired, or scoped to another bucket | §7 rotate; re-check **T5** |
| `… is not fetchable — refusing to swap the manifest in` | a frame of a **still-usable** product is gone — almost always a too-aggressive lifecycle rule (expired products' frames are exempt, so this is never a dead product's fault) | check **T4**'s rule is ≥ 48 h; the previous manifest is untouched meanwhile |
| The probe says a product is `MISSING` | not weather: its timer is gone (disabled, renamed, never installed) — an outage would have left it listed | `systemctl list-timers 'obc-wx-bake@*'`, then `adapters.conf` + re-run `install.sh` |
| Cycle killed, `MemoryMax` in the journal | an adapter exceeded 768 MB | that is a bug in the adapter, not a tuning problem — file it; raise the cap in the unit template only with a measurement |
| Every tick logs `flock`/timeout | a bake is wedged holding the lock | `systemctl stop 'obc-wx-bake@*.service'`, check `ps`, then start one by hand |
| `published manifest is unreadable (…); rebaking everything` | the manifest object got corrupted | self-healing — the next tick rebakes it from scratch |
| Timers "active" but nothing publishes | clock skew, or a paused system | `timedatectl` (NTP on?), then run one bake by hand |
| Everything green on the box, probe says stale | delivery, not baking: public access, custom domain, DNS or a cached 404 | re-do §4 steps 1–3 |
| A product is stuck at an old `reference_time` | upstream is serving a stale run; the baker refuses to move `reference_time` backwards or forwards without a complete run | check the journal's warning, then the provider's status page |

**Wedged state, last resort.** The published manifest is the service's *only* state, so deleting it
is a full reset with no data loss: load the environment as in §4 step 5, then
`rclone deletefile obcwx:obc-wx/wx/v1/manifest.json`, and run one bake per adapter by hand. Every product is rebaked from upstream; old frame objects age out on
their own. Do this only when the journal says the manifest itself is the problem.

**Total outage** (box gone, provider incident, invoice unpaid): do nothing urgent. R2 keeps serving
until the lifecycle rule expires the objects; riders see hourly-only weather from MET, which the
phone fetches directly and which does not depend on this service at all. Rebuild with §9 when
convenient.

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
   worth keeping in your password manager (5 min including finding them).
6. `systemctl start obc-wx-bake@dwd-rv.service`, read the journal (1 min).
7. Walk §4's six checks (5 min).
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
