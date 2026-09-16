# Verification and releases

This application stores system requirements, linked tests, immutable revisions, and release evidence.
The application database is the authority. Git contains the implementation, not an editable copy of
requirement prose. One inactive example is created in a new database. An example does not satisfy a
release gate until the owner reviews and activates it.

## Local development

Use Node 24.21 or a later Node 24 release:

```sh
npm ci
npm run dev
npm run check
npm test
npm run build
```

Set `VERIFICATION_DATA_DIR` to a private directory. The default is `./data`. Set `ORIGIN` to the
exact browser origin, including the port. Configure an owner login before opening the application.
There is no default password or anonymous editing mode.

## Server configuration

The production service runs on Debian with Node 24, Caddy, and systemd. Application builds live in
`/opt/obc-verification/releases/`; `current` points at the active build. Persistent data lives in
`/var/lib/obc-verification/`. Deployments do not replace this directory.

Copy the files in `ops/` to the server and run `sudo bash ops/install.sh`. The installer does not
start the application or write credentials. Create `/etc/obc-verification/service.env` with mode
0600, owned by root. Configure these values without committing them:

| Variable | Purpose |
| --- | --- |
| `ORIGIN` | `https://releases.openbikecomputer.com` |
| `VERIFICATION_OWNER_USERNAME` | Local owner account name |
| `VERIFICATION_OWNER_PASSWORD_HASH` | Run `python3 ops/password_hash.py` |
| `VERIFICATION_CI_TOKEN` | Random bearer token used only by CI |
| `VERIFICATION_AGENT_TOKEN` | Separate random token for agent reads and link proposals |
| `GITHUB_REPOSITORY` | `timohueser/OpenBikeComputer` |
| `GITHUB_TOKEN` | Repository-scoped token with Actions read/write and contents read access |
| `VERIFICATION_SOURCE_BRANCH` | Allowed candidate branch; default `develop` |
| `GITHUB_CLIENT_ID`, `GITHUB_CLIENT_SECRET`, `VERIFICATION_OWNERS` | Optional GitHub OAuth login and allowed owner logins |

The service sets its data directory, production mode, and localhost port 3100. Caddy terminates TLS
and forwards the release subdomain to `127.0.0.1:3100`. Point the subdomain's DNS A record at the VPS
before enabling public access. Never enter credentials through an HTTP origin.

The environment file and SSH private keys are not application assets. Back them up separately in a
private credential store. Rotate the CI, agent, and GitHub credentials independently.

## Deployment

`deploy-verification.yml` tests and builds the application on pushes to `develop` that change this
application. It also supports manual dispatch on `develop`. It packages only the build, runtime
dependencies, package metadata, and source identity. It does not package local data or credentials.

Configure the `verification` GitHub environment with secret `OBC_VERIFICATION_DEPLOY_KEY`. Configure
repository variables `OBC_VERIFICATION_SSH_HOST`, `OBC_VERIFICATION_KNOWN_HOSTS`, and
`OBC_VERIFICATION_URL`. Copy pinned SSH host keys from a trusted connection; do not accept host keys
dynamically in CI.

The deployment public key belongs to `obc-verification-deploy`. Its `authorized_keys` entry is:

```text
restrict,command="sudo -n /usr/local/sbin/obc-verification-deploy" ssh-ed25519 PUBLIC_KEY
```

This key accepts an application archive on standard input. It cannot open an interactive shell or
forward connections. The root-owned deployment command validates archive paths, installs a new
build, switches `current`, and checks `/health`. A failed health check restores the previous build.
The application process runs as the separate unprivileged `obc-verification` user.

`/opt/obc-verification/previous` retains the prior build. To roll back, switch `current` to that
build and restart `obc-verification`. Application rollback does not roll back data. If a future
change needs a database format change, take a verified backup before deployment and state the
restore requirement in that change. Remove older inactive build directories only after checking
the `current` and `previous` links.

## Candidate and publication workflows

Configure repository secret `OBC_VERIFICATION_CI_TOKEN` to match the service. Keep firmware signing
and R2 distribution credentials in the existing `release` GitHub environment:
`OBCU_SIGNING_SEED`, `OBC_R2_ACCOUNT_ID`, `OBC_R2_BUCKET`, `OBC_R2_ACCESS_KEY_ID`, and
`OBC_R2_SECRET_ACCESS_KEY`. The production key rotation gate remains mandatory. Test-key firmware
cannot become a release candidate.

The application dispatches `verification-candidate.yml` on `develop`. Before it calls the build or
test workflows, it verifies the candidate ID, version, source SHA, and source ancestry against the
service. The candidate snapshots the requirement revision. `ci.yml` runs the registry's release
selection, and `release.yml` builds and signs the shipping firmware once. The `verify` job requires
both to succeed. Native results and exact firmware files are copied into the application after
that gate completes. Missing or skipped linked tests cannot count as passes.

`verification-catalog.yml` imports observed native test identities after successful `develop` CI.
A run without a report cannot establish that an unobserved test passed. The catalogue retains the
source identity of the imported result set; the candidate's own evidence decides release readiness.

When an owner selects Publish, `verification-publish.yml` rechecks readiness and file hashes. It
creates the exact candidate's tag and a draft GitHub release, uploads firmware and verification
reports, publishes the release, and then updates the existing R2 distribution channel. A retry
accepts existing assets only if their bytes match. It never overwrites an existing version's
firmware. The confirmation job records publication only after GitHub and R2 work completes.

A tag push no longer builds or publishes firmware. `release.yml` is a reusable build workflow only.
Keep repository rules and `release` environment access limited to maintainers. GitHub administrators
can still publish outside these workflows; the application does not claim to prevent that.

## Backups and restoration

The installer enables a daily backup timer. It retains seven successful local snapshots.
Run `sudo /usr/local/sbin/obc-verification-backup` for an additional snapshot. It takes a SQLite snapshot and copies every
immutable attachment referenced by that snapshot into one archive under
`/var/backups/obc-verification/`. It does not need to stop the application. Copy these archives to a
separate private machine or backup service. A backup on the same VPS does not protect against loss
of that VPS. Do not use map or fixture bucket credentials for application backups.

Verify a backup into a new directory before restoring it:

```sh
sudo python3 ops/restore.py /var/backups/obc-verification/ARCHIVE.tar.gz /var/lib/obc-verification-restored
```

The restore command checks SQLite integrity and each attachment's size and SHA-256. It never
replaces live data. To restore service, stop `obc-verification`, preserve the current data directory,
move the verified directory into its place, set its owner to `obc-verification:obc-verification`, and
restart the service. Confirm owner login, a saved revision, and an attachment download.

## Operations

- Health: `curl --fail http://127.0.0.1:3100/health`.
- Logs: `journalctl -u obc-verification --since '-30 minutes'`.
- Proxy: `systemctl status caddy`.
- Application: `systemctl status obc-verification`.

Report failed workflow runs in the application. A missing callback stays pending or failed and must
not become a successful candidate. Retry a failed publication from the existing candidate; retain
its evidence and firmware. Prepare a new candidate when source code or required evidence definitions
change. Manual results do not carry to another candidate.

## Agent and CI API

Use `Authorization: Bearer TOKEN` with the dedicated agent token. The token can read
`GET /api/bootstrap`, `/api/revisions`, `/api/revisions/ID`, `/api/catalog`, `/api/candidates`,
and `/api/candidates/ID`. These responses use the same definitions as the owner interface.
Download a retained attachment with `GET /api/files/ID`.

After the owner explicitly asks for a proposal, send `POST /api/proposals` with JSON:

```json
{
  "baseRevision": 1,
  "requirementId": "REQ-001",
  "caseId": "python-repository-tools::RouteTests::test_upload",
  "action": "add",
  "reason": "This test checks the required route size."
}
```

Use a case ID returned by the catalogue, not the illustrative ID above. `action` can also be
`remove`. The owner accepts or rejects the proposal in the UI. A stale revision returns HTTP 409.
The agent token cannot write requirement prose, upload files, record manual outcomes, or publish.
Owner writes require a session and the exact configured browser origin.

CI uses its separate token for catalogue imports, retained firmware uploads, and candidate results.
The service verifies the GitHub workflow, source revision, run attempt, and required job outcome.
`ops/import_results.py` reads native JUnit and Swift result files. IDs include the report family,
class, and test name. Matrix platforms stay separate; workflow attempt numbers do not change IDs.
Skipped, missing, failed, or ambiguous results cannot satisfy a linked automated test.

Manual test inputs belong to the requirement revision. Evidence attachments belong to a specific
manual execution. Neither file changes after upload. Each candidate stores its own definition and
execution records. Future supervised hardware runners can submit executions against this boundary;
this application does not execute hardware scripts.
