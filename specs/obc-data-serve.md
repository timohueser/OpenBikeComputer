# obc data serve and deploy

`obc data serve`, which follows a target and runs the services of its planner releases, and
`obc data deploy`, which puts the follower of `prod` and the tile Worker in place.

- Writers: `host/obc-data` (`serve.rs`, `cli/deploy_cli.rs`).
- Readers: the web and iOS planners, the device, `obc data update` (`ready`).
- Code authority: `host/obc-data`.
- Related: [targets](obc-data-targets.md), [service runtimes](obc-data-products.md#service-runtimes),
  [commands](obc-data-cli.md).

## Serve

`obc data serve [TARGET]` follows the releases that a target names, by default `local`, and runs
the services of their planner releases. It answers on port 8710: for a `dir` target on every IPv4
address of the machine, and for an R2 target on `127.0.0.1` only, behind a proxy. It reads an R2
target through its public origin, without credentials. The current release is the serving
release once its services answer: a `dir` target gives clients its pointers. Every answer allows
each origin.

| Path | Answer |
| --- | --- |
| `/planner-api/version` | `{"commit", "formats", "manifest"}`: the commit that the build of serve names in `OBC_DATA_COMMIT`, or `null`, the format of each layer that its services read, and the format of the release manifest that it reads. Never cached |
| `/planner-api/releases/<id>/ready` | `{"state": "ready"}` while each service of the planner release `<id>` answers. 503 with `{"state": "starting"}` while its objects download or a service starts, `{"state": "installed"}` while its files are on disk and its services do not run, or `{"state": "failed", "reason": "not ready: <why>"}` with the reason of each failed service or of the failed fetch. 404 with `{"state": "unknown", "reason"}` while serve does not know `<id>` yet: a publisher that waits for `<id>` keeps waiting, as on 503. Never cached |
| `/planner-api/releases/<id>/<service>/<path>` | `<path>` of the `routing`, `search` or `downloads` service of the planner release `<id>`, while that service answers, also while another service of `<id>` failed. Search gets `/api/planner-search/<path>`. 503 with `{"code": "busy", "message": "The <service> service restarts. Try again shortly."}` while that service does not answer. 404 when serve does not know `<id>` |
| `/planner-offline/<path>` | `<path>` of the downloads service of the current release, for apps that name no planner release. 503 while it does not answer |
| `/cell-catalog/catalog.json` | `dir` only: the device pointer of the current release |
| `/planner/catalog.json` | `dir` only: the planner pointer of the current release. Each string that starts with one of its `origins`, followed by `/` or nothing, starts with the origin of the request instead: `http://` and the `Host` of the request |
| Another path | `dir` only: the file of the target at that key, with ranges; without a file, the tile handler of `planner/tiles` over the target |

Serve reads `release.json` and `planner/next.json` of a `dir` target each second, and of an R2
target every 30 seconds. It reads the manifest, the pointers and the list of the named files of a
release once: a rollback reads nothing else. It reads no other list. A `planner/next.json` that cannot be read, or whose release cannot be read,
names no release: serve writes why, reads it again at each look, and loads the other releases.
The planner release of a release is `active.id` of its planner pointer. Serve orders the releases
current, serving, release of `planner/next.json`, rollback, and counts each planner release once:

1. One fetch runs at a time, on a thread of its own. It is for the first planner release of the
   order that has no install and does not wait after a failure. While `planner/next.json` names a
   release, the rollback does not fetch; a fetch of the rollback that runs when the file appears
   stops between two files, and goes on after the file goes. A fetch first compares the formats of
   the layers of the release with the formats that the services read (see
   [Service runtimes](obc-data-products.md#service-runtimes)). A layer in another format fails the planner release
   before anything downloads: `needs a routing service that reads planner/routing format 10; it
   reads 9`. Then the fetch downloads the objects and installs the files. Serve keeps reading the
   two files and checking the services meanwhile.
2. The services of the first two planner releases of the order that have an install run. The
   services of each other installed planner release stop, and its files stay.
3. Clients get the serving release once each service of its planner release answers: routing
   `GET /health`, search `GET /api/planner-search/status` with `parser.ready` true, and downloads
   `GET /catalog`. Until then clients keep the current release. No client gets the release of
   `planner/next.json`.
4. Each service has its own state and wait. A service fails when its start fails, when it stops
   within 60 seconds after its first answer, or when it does not answer within 120 seconds after
   it starts. It starts again after 60 seconds, after each later failure in a row twice the wait
   before, up to 30 minutes. A service that stops 60 seconds or more after its first answer starts
   again at the next check, and its wait is 60 seconds again. The other services of its planner
   release keep answering, and clients keep the current release.
5. A fetch that fails waits as a failed service does. It fetches again at once only when the two
   files name other releases.
6. The services of a planner release that neither the serving release, the rollback, the release
   of `planner/next.json` nor the current release names stop.

When the serving release and the rollback fail one after the other, clients keep a release that
`release.json` no longer names, and the services of its planner release keep answering. Serve
then writes a warning: an update can delete the files of that release.

The services of a planner release read its files under `routing/`, `search/` and `offline/` in
its install, `<store>/serve/<target>/<planner id>/`. The document `releases/<planner id>/release.json`, which the list
of the named files names, lists them: format 1 in `files`; format 2 in the step indexes whose `folders` hold
one of those folders, and in `files`. Serve downloads those indexes as objects after the document,
checks their size and SHA-256, and keeps them with the files. Each file comes from an object of the store of serve, which downloads
an object that it lacks from `objects/` of the target, 8 at a time, and checks its SHA-256: a link
to an `identity` object, or the decoded bytes of a `gzip` object under a temporary name. No write
opens an existing file. Each file must have the SHA-256 and the size that the planner release
gives. After the files, the install gets the document as `release.json`, the data of the file
system goes to the disk, and the install writes the marker `installed` last, which holds the
planner release id. Only a folder whose marker names it is an install; a fetch deletes the content
of a folder without one first. The services read the region of the release from `release.json`.
When serve stops, it ends each download at its next read and each install between two files, and
waits for them within the 10 seconds of its grace; a wait before a retry, a connection or a hash
does not see the stop.

A planner release whose files hold `routing/directory.bin` has a routing package of format 10
([the route package contract](route-package.md)). Its files `routing/directory.bin`,
`routing/package.json` and `routing/objects/*` do not go into `routing/`. After the other files,
the routing program installs all cells and all profile groups of the package in the folder
`route-cells/` of the install:
`planner-service <install>/route-cells --install DIRECTORY PACKAGE OBJECTS 2`. `DIRECTORY` and
`PACKAGE` are the objects of `routing/directory.bin` and `routing/package.json` in the store of
serve, and `OBJECTS` is `objects/` of that store, which holds each object as
`<first two hex digits>/<SHA-256>`. The installer runs at nice 10 on 2 threads, checks each object
while it writes, and drops each file from the page cache after its sync, so that an install does
not evict the files of the serving release. A stop of the fetch kills it. A fetch keeps
`route-cells/` of a folder without the marker, and the next install goes on from its complete
files. A failed install fails the fetch with its exit status and, after an exit that is not
from a signal, the last line of the installer: `the routing cells (exit status: 1): <why>`. The
exit status 3 means that a published object is missing or damaged, and the last line is
`object <SHA-256>: <why>`: serve deletes that object from its store, so that the next fetch
downloads it again. The marker `installed` comes after the routing cells.

Before the objects of the files download, the fetch checks the disk. The install needs the bytes
of each object that the store lacks, the decoded bytes of each file that is not `identity`, and a
reserve of 5 % of the file system for the logs, the downloads cache and the file system. For a
routing package of format 10, the fetch first downloads `routing/directory.bin` and
`routing/package.json`, and the install also needs the installed bytes of the files that
`route-cells/` lacks, which `planner-service <install>/route-cells --missing DIRECTORY PACKAGE`
prints. When the
free bytes of the file system of the store are fewer, nothing downloads. Then, when the planner
release is not the one of the rollback, and no other release names the planner release of the
rollback, serve stops the services of the rollback, deletes its install, writes `WARNING: no
rollback during the install of <planner>: deleted the files of planner release <rollback>; the
disk has <free> GB free and the install needs <need> GB`, and fetches again at its next check.
Else the fetch fails with `the disk has <free> GB free; the install needs <need> GB`.

An install stays when serve starts and stops, and when a service stops: a restart installs
nothing again. Serve deletes an install only when neither a planner release that it follows nor
a release that the two files name has it, after its services stopped, and only after a look
read the two files and each release that they name: a start without a network deletes nothing.
`obc data clean` does not touch the installs. The objects are in a store of serve alone, `<store>/follower/<target>/`. After the
releases that the two files name change, and after fetches, serve deletes there each object and
each unfinished download that the services of none of them read, once no fetch runs. A `gzip`
object and a file of the routing package of format 10 of a planner release with an install go
too: the install holds the decoded file. A fetch
for a planner release that no release names any more stops between two files. For a
`dir` target, download bundles name the origin of the machine on its network when serve starts,
else `http://127.0.0.1:8710`. For an R2 target, they name `api_origin` and
`<objects_origin>/objects` of the planner pointer.

The routing service gets `<install>/route-cells` as its argument.
It maps each file there at its first use and keeps the map, so its RSS counts the mapped pages.
Two limits keep one service that grows from taking the memory of the unit from it:

- Search and downloads start with `oom_score_adj` 1000, so when the unit runs out of memory the
  kernel kills one of them first, not the routing service with its larger RSS.
- The routing service starts with `RLIMIT_DATA` of (workers + 1) GiB: 1 GiB for each worker and
  1 GiB for the shared part. `planner-service --workers` prints the worker count by the rule of
  the service: `ROUTE_WORKERS`, else one for each CPU of the process. The limit counts private
  writable memory, not the read-only maps of the files.

The kernel limits the maps of each process with `vm.max_map_count`. The VPS sets it to 1,048,576
(`tools/vps/obc-data.sysctl`). An install with more files than
the limit allows does not start its routing service. The reason of a failed service ends with the
last line of its standard error, unless a signal ended it, so the release fails with the message
of the routing service.

For a `dir` target, serve builds the routing service with cargo when it starts. For an R2 target,
the routing service is the `planner-service` program beside the program of serve, and serve
needs no tile handler. When the thread that follows the target ends without a stop, such as on a
panic, serve ends with a `failed` error, so that a supervisor starts it again.

## Deploy

An update of `prod` publishes data. Two programs serve it, and `obc data deploy` puts the commit
of the checkout that runs it in place: the follower, `obc data serve prod` as the systemd unit
`obc-data-serve` on the VPS, and the tile Worker of `planner/tiles` on Cloudflare.

| Part | Deploy | Answers `version` at | Credential |
| --- | --- | --- | --- |
| `services` | `tools/vps/deploy-services.sh "$OBC_VPS_SSH" COMMIT` | `<api_origin>/planner-api/version` | `OBC_VPS_SSH`: an SSH host that logs in as root |
| `tiles` | `npm ci --prefix planner/tiles`, then `npm run deploy --prefix planner/tiles -- --var COMMIT:COMMIT` | `<tiles_origin>/version`, `{"commit"}` | `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` |

The origins are the `[publication]` of `data/planner-runtime.toml`. Only 200 with `commit` is an
answer of this code; `commit` is `null` for a build that names none.

`obc data deploy` asks each part for its `version`, and checks what an action needs from this
machine. Each check has a name, its problem, its fix in words, the commands of the fix, and the
command of the page that applies it, if one does. The checks read and change nothing, except the
fetch of `origin/develop`, which never prompts. Each command that reaches another machine ends
within 15 seconds, and the checks run at once. The check of the follower runs up to three such
commands in a row: the login, the configuration of `ssh` and the host key.

| Action | Checks |
| --- | --- |
| Publish: an update of `prod` and `r2 legacy` | The `OBC_R2_*` variables; rclone on PATH |
| `deploy services` | `OBC_VPS_SSH`, which does not start with `-`; this machine logs in as root without a prompt, with a host key that it knows; the one-time setup on the VPS: the user `obc-data`, `/opt/obc-data`, Rust, uv and the uv Python of `.python-version` of that user, a C compiler, Node of the `[target]` of `data/planner-runtime.toml`, the enabled unit, and `vm.max_map_count` of at least 1,048,576. The deploy installs the unit and the limit of memory maps of its commit. The Caddy route of the API is a check that the deploy does not wait for. `tools/vps/`, `data/planner-runtime.toml` and `.python-version` have no change that the commit does not hold |
| `deploy tiles` | The two variables; the token may list the Workers of the account; `planner/tiles` has no change that the commit does not hold |
| Both | The commit of the checkout is on `origin/develop` after a fetch. An older commit of develop serves to go back |

A refused login names its causes: the VPS does not know the key of this machine, the host does
not log in as root, or the key has a passphrase and no agent holds it. It gives the line of the
first public key of `~/.ssh` to add to `/root/.ssh/authorized_keys` on the VPS, or without a key
the command that makes one. A host key that this machine does not know gives the fingerprint of
the ED25519 host key of the VPS to compare with the console of the hosting provider, and
`deploy trust --expect FINGERPRINT`. A host key that changed gives the command that removes the
old one. A missing piece of the setup gives its command.

`deploy trust --expect FINGERPRINT` reads the ED25519 host key of the VPS again and refuses
another fingerprint. It refuses when the known hosts already hold a key of the host: a key that
changed goes only with `ssh-keygen -R`. Then it asks, or needs `--yes` without a terminal, and
appends the key under the name and to the file that `ssh` reads for the host: its
`HostKeyAlias` and its first `UserKnownHostsFile`. It is the one change of a check.

`deploy services|tiles --dry-run` checks the credential, the commit and the files, and writes the commit, what the deploy changes, its commands and the arguments that start it. A
deploy starts only with `--expect COMMIT`, the commit of the checkout now, and asks first, or needs
`--yes` without a terminal. It is a detached [run](obc-data-layers.md#runs), and the output of each command goes to
its log. One run at a time deploys each part. The commands get only `PATH`, `HOME`, the user, the
locale, `TMPDIR`, the SSH agent and the credential of their part. `deploy credential
services|tiles` reads the values on standard input, as
[Credentials of sources](obc-data-store.md#credentials-of-sources) says, into `credentials.env`.
