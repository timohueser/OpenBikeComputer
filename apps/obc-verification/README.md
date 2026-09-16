# Verification and releases

This application stores system requirements, linked tests, immutable revisions, and release evidence.
The application database is the authority. Git contains the implementation, not an editable copy of
requirement prose. One excluded example is created in a new database. An example does not satisfy a
release gate until the owner reviews and includes it.

New requirements receive sequential IDs such as `SYS-028`. The server reserves each number when
a draft is created; `POST /api/requirements/next-id` with `{"count": n}` reserves a block. Deleted
or discarded draft numbers are not reused, including after history cleanup. Existing IDs stay
unchanged.

## Editing

**+ Requirement** inserts a new requirement after the selected one, in the same group. In the
editor, **Done, add next** (or Ctrl+Enter / ⌘+Enter) keeps the current requirement and opens the
next one. **Move up** and **Move down** change the order within a group. **More** holds the
secondary actions: Markdown import and export, group management, link proposals, and history.

**Import Markdown** reads `## Group` headings and `- **ID — Title.** Statement` lines into the
draft. An entry whose ID already exists in the draft is updated; every other entry receives a fresh
`SYS` ID. Tests and labels are kept. **Export Markdown** writes the draft in the same format.
Neither changes a saved revision until you select **Save revision**.

**Revision history** shows each revision as a difference against the one before it: added,
removed, and changed requirements, with the previous and new statement side by side.

## Requirement groups

Each requirement can have one optional group, such as Navigation or Bluetooth. Groups are flat.
In the requirement editor, choose an existing group or type a new name. Leave the field blank for
an ungrouped requirement. The sidebar lists groups in the order they first appear, folded until
you open them; search also matches group names. The release candidate view groups its evidence the
same way and can show only outstanding requirements. **Manage groups** renames a group or moves its requirements to Ungrouped. These changes remain in
the draft until you select **Save revision**. Empty groups are not stored.

Group names belong to the requirement revision. Existing candidates and reports keep the groups
from their saved revision. Groups organize the view; they do not change verification or release
rules. The API exposes the optional `group` field on each requirement.

## Requirement labels and deletion

New requirements are included in future release candidates. The editor has three fixed labels;
select a label to apply it, or select it again to remove it. More than one label can apply.

- **Definition incomplete**: the requirement definition needs work, such as a target battery life
  or load time. An included requirement with this label blocks publication, even if tests pass.
  An exception cannot override it.
- **Implementation needed**: the requirement is defined but has a known implementation gap. An
  included requirement with this label blocks publication until the owner clears the label in a
  new revision or an administrator accepts an exception for the candidate.
- **Excluded from releases**: future candidates do not require this requirement to pass verification.
  The candidate, final review, report, and release notes list exclusions separately from verified
  requirements. This label remains in effect until the owner removes it.

Save label changes in a new revision before preparing a new candidate. Existing candidates keep
all labels from their saved revision. The API retains `todo` for Definition incomplete and `active`
for inclusion, and adds the optional `implementationNeeded` boolean. Existing saved requirements
need no database conversion.

**Delete requirement** removes the selected requirement and its test links from the draft after
confirmation. **Undo** restores it before saving. Select **Save revision** to save the deletion.
Earlier revisions and existing release candidates retain the requirement, tests, and attachments.

## Release exceptions

An administrator can accept an exception for one defined, active requirement in a candidate. A
reason is required. Use this for a known limitation, missing test coverage, or a failed manual
check. The record includes the administrator and time. An exception does not mark tests as passed,
change the requirement, or carry into a new candidate. It also supports known limitations that
existing tests do not detect. To change a decision, remove it and record a new one. The candidate's
stored history retains both changes. Publication freezes these decisions with the other evidence.
If another administrator changes an exception during final review, publication stops until the
updated decisions are reviewed. The server retains the exact HTML report and JSON evidence bytes for downloads and publication retries.

The release review, HTML/JSON report, and GitHub release notes identify accepted exceptions.
Verified, excepted, and excluded requirements have separate counts. Incomplete included definitions, failed CI,
missing build files, signing failures, and provenance checks cannot be bypassed by an exception.
A failed automated test that also fails the required CI run therefore still blocks publication.

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
| `VERIFICATION_OWNER_USERNAME` | Local admin fallback account name |
| `VERIFICATION_OWNER_PASSWORD_HASH` | Initial local admin hash; run `python3 ops/password_hash.py` |
| `VERIFICATION_CI_TOKEN` | Random bearer token used only by CI |
| `VERIFICATION_AGENT_TOKEN` | Separate random token for agent reads and link proposals |
| `GITHUB_REPOSITORY` | `timohueser/OpenBikeComputer` |
| `GITHUB_TOKEN` | Repository-scoped token with Actions read/write and contents read access |
| `VERIFICATION_SOURCE_BRANCH` | Allowed candidate branch; default `develop` |
| `GITHUB_CLIENT_ID`, `GITHUB_CLIENT_SECRET` | GitHub OAuth login credentials |

The service sets its data directory, production mode, and localhost port 3100. Caddy terminates TLS
and forwards the release subdomain to `127.0.0.1:3100`. Point the subdomain's DNS A record at the VPS
before enabling public access. Use Cloudflare DNS-only mode. Caddy replaces incoming forwarded
address headers, and the localhost-only service trusts one proxy hop for login rate limits. A
second proxy requires an explicit trusted-address configuration. Never enter credentials through
an HTTP origin.

The environment file and SSH private keys are not application assets. Back them up separately in a
private credential store. Rotate the CI, agent, and GitHub credentials independently.

## Accounts

GitHub is the primary sign-in method. Register an OAuth app in the administrator's GitHub account:

- Homepage: `https://releases.openbikecomputer.com`
- Callback: `https://releases.openbikecomputer.com/auth/callback`
- Device flow: disabled. Keep the default token expiry setting.

Put the client ID and secret in the server environment file and restart the service. This login
requests only public identity, without repository access or private email permissions. The
application stores the GitHub account ID and username. It does not store profile email addresses or
GitHub access tokens. The workflow dispatch credential is separate from login credentials.

Sign in with the local admin fallback, then open **Account → Users**. Add your own GitHub username
with **Admin** selected. Add collaborators by their GitHub usernames. Approval is tied to GitHub's
stable account ID, so a renamed account retains access and a reused username does not inherit it.
All approved users can edit requirements, record test results, and manage releases. Admins can also
add and remove users. You cannot remove your current GitHub account. Removal ends that account's
sessions; it does not remove its historical test records or revisions.

The local fallback always has admin access. Its initial password hash comes from the environment
only when the database has no local admin hash. Change its password in **Account** while signed in
with that local account. A change ends its other sessions. Use a password manager to save the new
password. Subsequent deployments and restarts retain the changed password.

If you lose the local password, use SSH to recover it. Copy `ops/password_hash.py` to the server,
then run it as the application user, with the application's data directory:

```sh
sudo -u obc-verification python3 /path/to/password_hash.py --reset /var/lib/obc-verification
```

The command prompts for a new password without echoing it and ends local admin sessions. It does
not change requirements or evidence. There is no email reset service. Database backups contain
password hashes and session records; keep the backups private.

## Clear requirement history

An administrator can open **Account → Maintenance** to clear requirement history. The preview
shows how many revisions can be removed and how many are retained for release candidates.
The default keeps the current requirements and linked tests in a new revision. **Start fresh** also
clears the current requirements and tests. Enter the displayed confirmation phrase to proceed.
The server rejects the action if another user saved a revision after the preview was loaded.

Revisions referenced by any release candidate or publication remain available. The action does not
change release evidence, uploaded files, accounts, the CI catalogue, or backup archives. Old link
proposals are removed. Revision numbers continue to increase so that an older open editor cannot
save over the fresh state. An empty reset remains empty after a restart; the example is not seeded
again. Clearing history cannot be undone in the interface. This removes application history, not
all copies of data from backups or storage.

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
that gate completes. Missing or skipped linked tests cannot count as passes. An administrator can
accept a requirement exception, but the required CI and firmware gates must still pass.

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

On the maintainer Mac, the dedicated agent token is stored outside Git at
`~/.config/openbikecomputer/verification-agent.token`. Read it into the request without printing it.
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
Skipped, missing, failed, or ambiguous results cannot satisfy a linked automated test. An accepted
requirement exception is recorded separately and does not change these test results.

Manual test inputs belong to the requirement revision. Evidence attachments belong to a specific
manual execution. Neither file changes after upload. Each candidate stores its own definition and
execution records. Future supervised hardware runners can submit executions against this boundary;
this application does not execute hardware scripts.
