# obc data commands

The `obc data` commands, the overview, the operator page, the R2 client, the errors and the JSON
output.

- Writers: `host/obc-data`.
- Readers: people, scripts, the operator page of `builder/web`.
- Code authority: `host/obc-data`. Its Rust types generate [Error codes](#error-codes),
  the table of [JSON schemas](#json-schemas) and [`obc-data.schema.json`](obc-data.schema.json),
  and a test compares them with these files.
- Related: [registry](obc-data-registry.md), [targets](obc-data-targets.md),
  [serve and deploy](obc-data-serve.md), [prepared sources](obc-data-prepared.md).

## Overview

`obc data` gives the machine that runs it and, for each target, the release that it serves, the
machine that made it and when, its rollback, for a `dir` target how [serve](obc-data-serve.md#serve) gives it to
each app, the run that holds its lock or why `lock.json` cannot be read, the region and the parts
that are on, the state of each layer against the steps of now, the blocked steps, the notes of the
layers as in the [review](obc-data-targets.md#review), the pointers that drift, an error that stopped the judgement,
and the command to run next. While a run holds the lock of a `dir` target,
the next step is to wait for that run, unless its process ended on this machine: then the next
update takes the lock over. It fetches only small metadata, and checks upstream with the checks
of the last hour. A target that serves nothing has the next command
`obc data update TARGET --region ID`. For an R2 target, the next command is the review
(`--dry-run`): its update needs the id that the owner approved.

## Commands

| Command | Output |
| --- | --- |
| `obc data [--json]` | The [overview](#overview): the machine, and for each target what it serves and the command to run next |
| `obc data update TARGET [--region ID\|PATH[,PATH…] \| --box W,S,E,N] [--add LAYER]… [--remove LAYER]… [--refresh SOURCE=DAYS\|manual]… [--only LAYER,…] [--dry-run \| --plan \| --expect ID] [--accept-format-change] [--yes] [--json]` | The [update](obc-data-targets.md#update) of a target. The flags change its settings. `--dry-run` writes the [review](obc-data-targets.md#review) and changes nothing. `--plan` makes the review in a run and changes nothing; with `--json`, it writes the run and a `null` review id. Else it starts a detached run, as [Runs](obc-data-layers.md#runs) says, and writes its log until it ends: the steps that it baked or reused, the release that the target serves, its rollback, the number of kept outputs and what the collection did. With `--json`, it writes the run and the review id |
| `obc data rollback TARGET [--yes] [--json]` | The [rollback](obc-data-targets.md#rollback) of a target. Writes the release that the target serves and its rollback |
| `obc data router TARGET [--json]` | For an R2 target, asks the follower once, as [Router ready](obc-data-targets.md#router-ready) says, for the planner release of the serving release and of the rollback. Writes each answer, and the release that `planner/next.json` names. Changes nothing. Refuses a `dir` target |
| `obc data unlock (TARGET \| --source ID) --from MACHINE [--yes] [--json]` | Frees the [lock](obc-data-targets.md#lock) of a target, or with `--source` the lock of a [prepared source](obc-data-prepared.md#prepared-sources), that a run of MACHINE holds with a heartbeat 10 minutes old or older. Asks first. Writes the holder that it removed |
| `obc data sources [--check-now] [--json]` | Every source with licence, and for each target that serves a release its policy, its versions and its [state](obc-data-registry.md#state-of-a-source); the versions in the local store; for a prepared source the root of its home, its versions there and the run that holds its lock; and how this machine holds it, as the [source overview](obc-data-registry.md#source-overview) says. Rows are in kind order: data, then assets, then tools. An upstream check of the last hour serves, except with `--check-now` |
| `obc data sources --layer [LAYER] [--json]` | For each layer, or LAYER, the sources that it reads with their status, as the [source overview](obc-data-registry.md#source-overview) says |
| `obc data sources (--region ID\|PATH[,PATH…] \| --box W,S,E,N) [--json]` | The sources that a bake of the region reads, what each layer and each source still needs, and what a bake still downloads or prepares, as the [source overview](obc-data-registry.md#source-overview) says. Fetches nothing |
| `obc data source refresh ID [--json]` | Fetches upstream's newest version of a `digest` source without `start` or params, and writes `refreshed/<source>.json` of the store with the versions that the targets serve: the next update of a target that serves one of them, or none, reads it, as [Versions](obc-data-layers.md#versions) says. Refuses another source. The fetch is a detached [run](obc-data-layers.md#runs): a terminal follows it, and `--json` writes `{run}` and ends |
| `obc data source prepare ID [--region ID\|PATH[,PATH…] \| --box W,S,E,N] [--languages L,…] [--dry-run \| --expect ID] [--json]` | [Prepares](obc-data-prepared.md#prepare) a version of a prepared source in the archive. `--dry-run` writes the review. Else it starts a detached run and writes its log until it ends; with `--json`, it writes the run and the review id |
| `obc data source copy ID --from TARGET [--dry-run] [--json]` | [Copies](obc-data-prepared.md#copy) the versions of a prepared source from a target into the archive. `--dry-run` writes the plan. Else it starts a detached run and writes its log until it ends; with `--json`, it writes `{run}` |
| `obc data source credential ID [--json]` | Reads the values of the credential of a source on standard input and writes them, as [Credentials of sources](obc-data-store.md#credentials-of-sources) says. Writes the file and the names of the values |
| `obc data fetch SOURCE[@VERSION] [NAME=VALUE…] [--json]` | Fetches the version, or else the newest file upstream. Writes the store path of each file |
| `obc data archive status [--json]` | The [status](obc-data-archive.md#status) of the archive. Changes nothing |
| `obc data archive get DATASET [--root PATH] [--json]` | [Gets](obc-data-archive.md#get) the dataset. Writes the status of the dataset |
| `obc data archive verify DATASET [--json]` | [Verifies](obc-data-archive.md#verify) the dataset. Writes the status of the dataset |
| `obc data areas [QUERY] [--json]` | The regions of `data/regions/` and the Geofabrik areas of the cached index whose id or name holds `QUERY`. Fetches nothing |
| `obc data clean [--apply [--yes]] [--json]` | The plan of [Clean](obc-data-store.md#clean). `--apply` asks, then cleans. With `--json` and `--apply`, the plan goes to standard error, and the output is what it did |
| `obc data runs [--json]` | Every run in the store, newest first: id, arguments, state, time, and the size of its fetches and of the layers that it built |
| `obc data runs RUN [--json]` | One run: its status, with the users of each step and its time in the last run that built it, the last 50 lines of its log with its path, and the review of a run of `update --plan` |
| `obc data runs RUN --follow [--json]` | The log of the run as it comes, until it ends; with `--json`, its status each time it changes. Ctrl-C ends only the following. The exit status is the one of the run |
| `obc data runs RUN --stop [--json]` | Stops a run that runs, as [Runs](obc-data-layers.md#runs) says. With `--json`, writes the run as `runs RUN` does |
| `obc data serve [TARGET] [--detach] [--json]` | [Serves](obc-data-serve.md#serve) a target until Ctrl-C. Writes its origin on the machine, and for a `dir` target its origin on the network and how each app opens it. With `--detach`, it serves in a detached [run](obc-data-layers.md#runs) and writes `{run}`; `runs RUN --stop` stops it. That run does not hold the store for [Clean](obc-data-store.md#clean) |
| `obc data deploy [--json]` | The commit of this checkout; for each part of [Deploy](obc-data-serve.md#deploy), the commit that it runs or why it does not answer, and the checks of a deploy; and the checks of a publish to `prod`. Changes nothing |
| `obc data deploy services\|tiles [--dry-run \| --expect COMMIT [--yes]] [--json]` | `--dry-run` writes what the deploy does. Else it deploys the commit in a detached run and writes its log until it ends; with `--json`, it writes `{run}` |
| `obc data deploy credential services\|tiles [--json]` | Reads the values of the credential of the deploy on standard input and writes them to `credentials.env`. Writes the file and the names of the values |
| `obc data deploy trust --expect FINGERPRINT [--yes] [--json]` | Adds the host key of the VPS to `~/.ssh/known_hosts`, as [Deploy](obc-data-serve.md#deploy) says. Writes the fingerprint and the file |
| `obc data setup [--json]` | What this machine can do: each job, each tool with its version or its install command (Java needs 21 or newer, Node 22 or newer), each credential (only whether it is set, and how to set it), and the store with its size and free space. It exits 0 also when a tool or a credential is missing |
| `obc data ui` | Builds the page with `npm run build:data` in `builder/web`, then serves it as [The page](#the-page) says, until it is stopped |
| `obc data open MACHINE` | Runs `ssh -N -L 8701:localhost:8700 HOST`, where HOST is the `ssh` of MACHINE in [`data/machines.toml`](obc-data-registry.md#datamachinestoml) or else MACHINE, and opens `http://localhost:8701/` in the browser. It sends nothing to MACHINE |

`--json` writes one JSON document to standard output. [JSON schemas](#json-schemas) has the
schema of each output, and [Errors](#errors) has the error codes and the exit statuses. In
addition:

- `fetch` lists only the requested files.
- `runs` lists a run whose `status.json` cannot be read as `failed`, or as `running` while its
  lock is held.
- `runs RUN --follow --json` writes one status per line. When the run failed, the error is the
  last line.

### The page

`obc data ui` listens on `127.0.0.1:8700` and answers:

- `GET /` with `builder/web/dist/data/data.html`, and `GET /PATH` with the file PATH in that
  folder. A part of PATH that is empty, `.` or `..`, or that holds `\`, names no file.
- `POST /api/cli` with the body `{"args": [ARG, …]}`: it runs its own binary as
  `obc data --json ARG…` in the repository root, with no standard input, and answers
  `{"exit": STATUS, "output": JSON}`, the exit status and the JSON that the command wrote. When
  the command writes no JSON, `output` is a `failed` [error](#errors) and the status is not 0.
  A command runs to its end, and its output is read, also when the request ends first. The page
  runs only `obc data` without a command, `obc data setup`, `obc data areas`, `obc data sources`, `obc data sources --layer`, `obc data sources --region ID`, `obc data runs`, `obc data runs RUN`,
  `obc data runs RUN --stop`, `obc data router TARGET`, `obc data serve TARGET --detach`, `obc data update TARGET` with settings flags and then `--dry-run`,
  `--plan` or `--expect ID` (with `--accept-format-change` and `--yes`), `obc data rollback TARGET --expect ID`
  (with `--yes`), `obc data unlock TARGET --from MACHINE --yes`, `obc data source refresh ID`, and
  `obc data source prepare ID` with `--region`, `--box` and `--languages` and then `--dry-run` or
  `--expect ID`, `obc data clean`, `obc data clean --apply --yes`, `obc data deploy`,
  `obc data deploy services|tiles --dry-run` and `--expect COMMIT --yes`, `obc data deploy trust
  --expect FINGERPRINT --yes`, `obc data r2 legacy
  TARGET`, and `obc data r2 legacy TARGET --delete --expect DIGEST --reason TEXT --yes`.
- `POST /api/credential` with the body `{"args": ARGS, "values": [VALUE, …]}`, where ARGS is
  `["source", "credential", ID]` or `["deploy", "credential", "services"|"tiles"]`: it runs
  `obc data --json ARGS…` the same way, with each value on a line of its own on standard input,
  and answers in the same form. A body that is not of this form, an ID that starts with `-`, or a
  value with a line break gets HTTP 400 and the code `usage`. The page logs no body,
  and no answer holds a value.

Each request has a `Host` of `127.0.0.1`, `localhost` or `[::1]` at any port; an `Origin`, when
it has one, of `http://` and that `Host`; and, for a POST, the `Content-Type` `application/json`.
A request that does not, and a command that the page does not run, get HTTP 403 and
`{"exit": 2, "output": {"error": …}}` with the code `usage`.

## R2 client

The R2 client in `host/obc-data` reaches the bucket for `obc bake publish --target r2`,
`obc bake clean-r2`, `obc fixtures publish` and the firmware publish workflow. rclone moves the
bytes. `obc data r2` is plumbing for scripts: those commands call it, and a person deletes a key with
`obc data r2 delete`. It does not change the state of a release. Once the bucket has the
`release.json` of `obc data`, `obc bake publish --target r2` refuses to run, and `obc data r2 put`
and `delete` refuse `release.json`, `lock.json`, `planner/next.json`, a pointer key, a key
under `objects/` or `releases/`. `obc data r2 delete` refuses a key under `sources/<id>/` of a
prepared source while the [archive](obc-data-prepared.md#prepared-sources) lacks a version that
`sources/<id>/versions.json` of the bucket lists, with its manifest SHA-256 and that manifest.
`obc bake clean-r2` refuses to run when the pointer of its prefix has `release`, or is not JSON. `obc data` never writes under `reference/v1/`.

### Credentials

| Variable | Meaning |
| --- | --- |
| `OBC_R2_BUCKET`, `OBC_R2_ACCESS_KEY_ID`, `OBC_R2_SECRET_ACCESS_KEY` | The bucket and its key |
| `OBC_R2_ACCOUNT_ID` | The Cloudflare account; the endpoint is `https://<id>.r2.cloudflarestorage.com` |
| `OBC_R2_ENDPOINT` | Another endpoint; replaces the one from `OBC_R2_ACCOUNT_ID` |
| `OBC_R2_LOCAL_DIR` | A local directory that replaces the bucket. Tests use it. When it is set, it must not be empty, and `OBC_R2_BUCKET` must not be set |

`--fixtures` reads the same variables with the prefix `OBC_FIXTURE_R2_` instead. `legacy` refuses
`--fixtures`: it reads the bucket of its target. A credential goes to rclone only in its
environment, never in an argument or a file. The S3 client of an R2 target signs with the secret
key and never sends it.

### Commands

| Command | Does |
| --- | --- |
| `obc data r2 list PREFIX [--json]` | Lists every object under the folder `PREFIX`. A `PREFIX` that names an object is refused |
| `obc data r2 stat KEY... [--json]` | Lists the objects of the keys that the bucket holds; a missing key is not an error |
| `obc data r2 get KEY FILE [--json]` | Downloads one object |
| `obc data r2 put FILE KEY [--cache-control V] [--content-type V] [--immutable] [--json]` | Uploads with `--checksum` and the headers given, then verifies |
| `obc data r2 delete (KEY... \| --prefix P) --reason TEXT [--legacy] [--yes] [--json]` | Deletes, see below |
| `obc data r2 legacy TARGET [--prefix P] [--delete --expect DIGEST --reason TEXT [--yes]] [--json]` | Lists the legacy keys that the target does not use; with `--delete`, deletes them, see below |

Rules:

- The `r2` commands are plumbing below the rule of [Errors](#errors). Only `delete` and
  `legacy --delete` ask: a delete cannot be undone. `put` and `get` never ask.
- A key or a prefix is never empty, has no empty, `.` or `..` part, and has no control
  character. The bucket root is never a target.
- Verify passes when the object has the size of the file, and the same MD5 when both sides
  report one.
- `--immutable`: when the key holds an object, nothing is uploaded and the object is never
  replaced. The command passes only when the object has the size and the MD5 of the file. It
  fails when they differ, and when the bucket reports no MD5 for the object.
- `delete --prefix P` resolves `P` to its keys first. Then explicit keys and a prefix have the
  same rules: a key that the bucket does not hold is refused, `removed.jsonl` is refused, a key
  under `cell-catalog/`, `planner/` or `reference/` is refused without `--legacy`, and then nothing
  is deleted. Legacy keys go only after the cutover, from the list that the owner approved.
- `legacy` lists each key under `cell-catalog/` and `planner/`, or under the one `--prefix`, that
  `obc data` does not write. It refuses while the target serves no release. It refuses
  `reference/v1` while `reference/v1/index.json` is missing or does not parse, and while the
  `index.json` of the newest version of `terrain` in the archive lacks a key of its `sources`. The
  digest is the SHA-256 of one line `<bytes>  <key>\n` per key, sorted by key. With `--delete`, it lists again,
  refuses when the digest is not `--expect`, asks, and then deletes in a detached [run](obc-data-layers.md#runs): as
  `delete --legacy`, with the counts, the bytes and the digest as its plan. The run lists again
  and refuses another digest too. One run at a time deletes the keys of a target. With no key, the
  run deletes nothing. With `--json`, it writes `{run}`; else it follows the log of the run.
- `delete` prints the plan: the bucket, and each object with its size and
  upload time. With `--json`, the plan goes to standard error, and the output is the objects
  that it deleted. Then it asks as [Errors](#errors) says.
- `delete` appends one line per object to `removed.jsonl` at the bucket root before it deletes:
  `{"by": USER, "bytes": N, "key": KEY, "reason": TEXT, "removed": "YYYY-MM-DDTHH:MM:SSZ"}`,
  with the escapes of Python's `json.dumps`: `\uXXXX` for DEL and for each character outside
  ASCII. When the delete fails, the error names the keys that the bucket still holds.

## Errors

A command that fails writes its error to standard error. With `--json`, it writes
`{"error": {"code", "message", "fix"}}` on one line to standard output. The code is stable and
sets the exit status. The message tells what failed, and the fix tells what to do.

| Exit status | Meaning |
| --- | --- |
| 0 | The command succeeded |
| 1 | A check found problems, or the command failed: a file is not valid; a fetch, R2, a run or the store failed; or the person did not agree |
| 2 | Usage: an argument is not valid, or a command that deletes did not get consent |
| 4 | Blocked: a credential is missing, a layer cannot be planned, the target cannot take the command, or this machine cannot do the work |
| 5 | Verify failed: a release failed its check, or the bytes in a target are not the bytes that the command wrote |

The commands that delete, `clean --apply` and `r2 delete`, and `unlock` show their plan and ask
in a terminal. Without a terminal, they need `--yes`. Without it, they exit with 2 and change
nothing.

### Error codes

| Code | Exit | When | Fix |
| --- | --- | --- | --- |
| `usage` | 2 | An argument is not valid, an id names nothing, the command runs outside the repository, or another command must run first. | Correct the command. `obc data --help` lists the commands and their arguments. |
| `no_terminal` | 2 | A command that asks has no terminal to ask in, and no `--yes`. | Show the question to a person. When they agree, run the command again with `--yes`. |
| `not_confirmed` | 1 | The person did not answer yes. | Nothing changed. Run the command again when you want the change. |
| `invalid_data` | 1 | A file under `data/` is not valid. | Correct the file that the message names. `specs/obc-data-registry.md` gives its format. |
| `fetch_failed` | 1 | A fetch or an upstream check failed. | Run the command again. A download continues where it stopped. |
| `blocked` | 4 | A credential is missing: a fetch failed without the credential of its source, or the R2 variables are not set. Or a layer cannot be planned, the target cannot take the command, or this machine cannot do the work. | Set the credential that the message or `obc data sources` names, or correct what the message says a product needs, then run again. |
| `r2_failed` | 1 | R2 or rclone failed, or refused a key. | Check the key, the `OBC_R2_*` variables and that rclone is on PATH, then run again. |
| `verify_failed` | 5 | A release failed its check before a target serves it, or after an upload the object in the bucket is not the file. | Upload the file again. |
| `run_failed` | 1 | A run failed: the build, or the run that `runs RUN --follow` shows. | `obc data runs RUN` shows the step that failed and its error. |
| `failed` | 1 | The store or the file system failed. | Correct the file or the directory that the message names, then run again. |

## JSON schemas

`--json` writes one document of the schema in this table to standard output.
[`obc-data.schema.json`](obc-data.schema.json) holds the schemas, which come from the Rust
types in `host/obc-data`. A test fails when this section or that file is not what they give;
`OBC_UPDATE_DATA_SPEC=1 cargo test -p obc-data` writes them again.

| Command | Schema |
| --- | --- |
| `obc data` | `Overview` |
| `update` | `Started` |
| `update --dry-run` | `Review` |
| `rollback` | `RolledBack` |
| `router` | `Router` |
| `unlock` | `Unlocked` |
| `sources` | `Sources` |
| `sources --layer` | `SourceLayers` |
| `sources --region`, `sources --box` | `RegionSources` |
| `source refresh`, `source copy`, `serve --detach`, `deploy WHAT --expect`, `r2 legacy --delete` | `Detached` |
| `source prepare` | `Started` |
| `source prepare --dry-run` | `PrepareReview` |
| `source copy --dry-run` | `CopyPlan` |
| `fetch` | `Fetched` |
| `archive status` | `ArchiveStatus` |
| `archive get`, `archive verify` | `ArchiveDataset` |
| `areas` | `Areas` |
| `clean`, `clean --apply` | `CleanPlan` |
| `serve`, when it listens | `Listening` |
| `setup` | `Setup` |
| `source credential`, `deploy credential` | `CredentialSet` |
| `deploy` | `Deployed` |
| `deploy WHAT --dry-run` | `DeployPlan` |
| `deploy trust` | `Trusted` |
| `runs` | `RunList` |
| `runs RUN`, `runs RUN --stop` | `View` |
| `runs RUN --follow`, one per line | `Status` |
| `r2 list`, `stat`, `delete` | `Objects` |
| `r2 get` | `Downloaded` |
| `r2 put` | `Uploaded` |
| `r2 legacy` | `Legacy` |
| Every command that fails | `Failure` |
