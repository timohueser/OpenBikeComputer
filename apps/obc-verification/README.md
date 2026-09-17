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

## Coverage workflow

Each requirement has a **Coverage** section: its acceptance criteria, the tests that are evidence
for each criterion, and the gaps that remain. Several tests can support one criterion; one test can
support several criteria. A criterion is covered once it has evidence and no gap. The badge next to
the heading shows one state per requirement, with the count of covered criteria:

| State | Meaning |
| --- | --- |
| Not assessed | No plan has been saved. |
| Needs review | A plan exists, but the statement or tests changed after approval, or a candidate uses another source commit. |
| Partial | Approved, but gaps remain. |
| Covered | Approved, and every criterion has evidence. Only this state satisfies the release gate. |

Coverage and test results are separate. An active requirement needs covered, approved coverage for
the candidate's exact source commit and passing results for all linked tests. Test names alone
cannot establish coverage; an owner judges whether the criteria capture the whole requirement and
the evidence supports each claim. A candidate exception remains a separate administrator decision.

### Edit coverage yourself

Select **Define coverage** or **Edit coverage** on a requirement. The plan is part of the
requirement draft, like its statement and tests: write each criterion, add evidence, say what each
test proves, and note the gap for anything still missing. The checkmarks and the count update as
you edit. Evidence comes from the CI catalogue or from a manual procedure; **+ New manual procedure**
in the picker creates one on the spot. Adding catalogue evidence links the test to the requirement.
A test that is used as evidence cannot be unlinked until it is removed from its criterion.

**Save revision** stores the plan with the requirement. Saving never approves: the badge shows
**Needs review** until you select **Approve coverage**. Approval attests the saved statement, tests,
and plan for one commit, by default the latest CI catalogue commit. It does not create test results.
Approving again after a change is the same single action, with nothing to re-enter.

### Review an agent's proposal

An agent can propose an initial plan or revise accepted coverage. Proposals appear on their
requirement, above the current plan; the sidebar marks the requirement and the **proposals to
review** button in the heading jumps to the next one. A proposal shows new, changed, and removed
criteria, with new evidence marked and removed evidence struck through, plus the test links it adds
or removes. **Approve** saves the plan, applies the links, and records your approval for the commit
the agent assessed, in one action. **Reject** asks for feedback, which the agent can read before
submitting a revision. Agents cannot approve coverage.

Individual link suggestions from older agents appear in the same place. A suggestion that a
pending plan already contains is folded into that plan and resolved when the plan is approved. Save or discard requirement drafts before
approving. Approval rechecks the target requirement, coverage, test definitions, and catalogue;
stale proposals must be refreshed. Changes to other requirements do not prevent approval.

### When a requirement changes

A change to the statement, the tests, or the plan itself keeps the criteria, evidence, and gaps
but clears the approval; the badge shows **Needs review**. Check the plan, or ask an agent to
reassess it, then approve again. Renaming a requirement, changing its group, or changing labels
does not invalidate coverage. Labels still apply their own release gates. Changes to another requirement do not clear
this requirement's approval.

A new source commit requires a fresh assessment and owner approval. This conservative rule covers
changes to test assertions as well as implementation. The requirement view shows the state for its
saved assessment; the release view also checks the candidate's source commit. Existing candidates
keep their original requirement and coverage snapshots. Published reports remain frozen.

### Local demonstration

Run with Node 24 from the repository root:

```sh
npm run demo:coverage --prefix apps/obc-verification
```

Open `http://127.0.0.1:4180` and sign in as `demo` with password `local-coverage-demo`.
The disposable database starts with accepted plans and proposals that revise them:

- **SYS-039**: an approved partial plan with a pending proposal above it. The proposal adds
  selection evidence and unlinks an obsolete test; persistence remains a gap, so approval keeps
  coverage partial. Or select **Edit coverage**, add a manual procedure as evidence, save the
  revision, and approve.
- **SYS-030**: a demo addition to the requirement cleared the approval while keeping the criteria.
  Its proposal records the added zoom obligation as a gap.
- **Releases → v0.0.0-coverage-demo**: the candidate retains the earlier snapshot and simulated
  passing results. Later edits do not change its evidence.

The additional catalogue tests are explicitly illustrative, not implemented product tests.
The demo disables GitHub integration and uses a separate temporary database. It makes no production
changes. Stop it with Ctrl+C; restart the command to reset the example. Set `VERIFICATION_DEMO_PORT`
to use another local port.

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
private credential store. Rotate the CI and GitHub credentials independently. Create temporary
agent tokens in **Account → Agent access**. The service does not accept `VERIFICATION_AGENT_TOKEN`;
remove this unused variable from an existing service configuration.

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
add and remove users and create or revoke agent tokens. You cannot remove your current GitHub
account. Removal ends that account's sessions and revokes the agent tokens it issued. It does not
remove its historical test records, proposals, or revisions.

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

An administrator can create a separate, temporary token for each agent task. Sign in with your
approved GitHub admin account or the local administrator. No GitHub personal access token, SSH
connection, or service restart is required.

1. Open **Account → Agent access**.
2. Enter a token name that identifies the machine or task.
3. Set its lifetime in minutes. The default is 60; the allowed range is 1 to 240.
4. Select **Create token**, then **Download token file** or **Copy token**. The secret is available
   only in this view after creation. Save it before leaving the tab or selecting **Done — hide token**.
5. Save the file outside Git at `~/.config/openbikecomputer/verification-agent.token`.
   Give the directory mode 0700 and the file mode 0600. For a downloaded file, run:

   ```sh
   install -d -m 700 ~/.config/openbikecomputer
   install -m 600 ~/Downloads/verification-agent.token ~/.config/openbikecomputer/verification-agent.token
   rm ~/Downloads/verification-agent.token
   ```

6. Give the agent the file path. Do not paste the secret into chat or put it in a URL, command
   history, or log. Read it into the request without printing it.

The panel lists token names, issuers, expiry times, and last use. Select **Revoke token** to end
access immediately. Select **Show expired and revoked tokens** to see old entries. Revocation and
expiry keep submitted proposals available for review. Create a new token when more time is needed.

The server stores a SHA-256 hash of each random 256-bit token. It checks expiry, revocation, and the
issuing administrator's current access on every authenticated request. Each proposal records the
token ID, name, and issuing account. A token grants agent permissions only, regardless of its
issuer's permissions. The secret cannot be retrieved after creation.

Administrators manage tokens through `GET` and `POST /api/admin/agent-tokens`, and
`DELETE /api/admin/agent-tokens/ID`. Creation takes `{ "name": "Coverage review", "lifetimeMinutes": 60 }`
and returns `{ "token": "...", "access": { ... } }`. Listing returns metadata only. These endpoints
require an administrator session; writes also require the exact configured browser origin.

Use `Authorization: Bearer TOKEN` with the dedicated agent token. The token can read
`GET /api/bootstrap`, `/api/revisions`, `/api/revisions/ID`, `/api/catalog`, `/api/candidates`,
`/api/candidates/ID`, and `/api/proposals`. These responses use the same definitions as the owner interface.
Download a retained attachment with `GET /api/files/ID`.

### Propose coverage

Prefer a whole coverage plan when assessing a requirement. Read the current requirement and its
existing coverage first. Audit the source at an exact commit. Identify every obligation, preserve
stable criterion IDs when revising a plan, map tests by their actual assertions, and describe
missing tests or implementation in `gap`. Do not change the requirement title or statement.
A plan is covered when every criterion has evidence and no gap; otherwise it is partial.
Proposing a plan never approves it.

Read decisions and reviewer feedback with `GET /api/coverage-proposals`. Send
`POST /api/coverage-proposals` with:

```json
{
  "baseRevision": 7,
  "requirementId": "SYS-039",
  "sourceSha": "EXACT_40_CHARACTER_COMMIT_SHA",
  "plan": {
    "rationale": "Projection is tested. User selection and persistence are not.",
    "criteria": [
      {
        "id": "projection",
        "statement": "Both orientations render correctly.",
        "evidence": [
          { "caseId": "REAL_NORTH_UP_CATALOGUE_ID", "rationale": "Checks north-up projection." },
          { "caseId": "REAL_HEADING_UP_CATALOGUE_ID", "rationale": "Checks course rotation." }
        ],
        "gap": ""
      },
      {
        "id": "selection",
        "statement": "The user can select either orientation.",
        "evidence": [],
        "gap": "Add the user setting and an interaction test."
      },
      {
        "id": "persistence",
        "statement": "The choice survives a restart.",
        "evidence": [],
        "gap": "Add a save/reload test that also starts a ride."
      }
    ]
  }
}
```

Replace all placeholder IDs and the source SHA. For an existing manual check, use `testId` instead
of `caseId`. Each evidence entry must identify exactly one test and explain its assertions.
The server rejects unknown tests and duplicate criteria. A criterion may have no evidence yet:
an agent can propose the criteria first and evidence later, with or without a `gap` note.
To explicitly unlink existing tests, include `removeTestIds` in the plan. These
are requirement test IDs, not catalogue case IDs. A removed test must not remain mapped to any
criterion. Omitted removals retain the existing links. Approval applies additions, removals, and
coverage review in one transaction. The proposal retains the requested removals for audit; the
saved plan contains only its criteria and summary. The top-level source SHA identifies the commit
the agent assessed; it is not a claim that tests have run, and approval records it.

The response contains the pending proposal ID and agent attribution. An identical pending request
is reused. To revise your account's pending proposal, submit the new plan with its ID in the
optional top-level `supersedes` field. The old proposal remains in history as superseded.
After rejection, read `feedback` and submit a new proposal against the latest requirement revision.
An agent cannot accept or reject plans. Owners edit the plan as part of the requirement draft:
`PUT /api/requirements` accepts a `coverage` plan on each requirement, validates it the same way,
links cited catalogue cases, and never accepts an approval. Owners approve a saved requirement with
`POST /api/requirements/ID/coverage/approve` and `{ "baseRevision": N, "sourceSha": SHA }`. Both
require an owner session and exact browser origin. Owner review of an agent proposal uses
`POST /api/coverage-proposals/ID` with `{ "accept": true, "feedback": "" }`.

Approval rechecks the requirement, existing test links, current coverage plan, and catalogue.
Changes to that requirement require a refreshed proposal; other requirements can be approved
sequentially. After an approval, prepare a new candidate to use the saved coverage snapshot.

### Individual link API

Prefer a coverage proposal for new assessments. The individual link API remains available for
existing clients. After the owner explicitly asks for a link proposal, send `POST /api/proposals`
with JSON:

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
`remove`. The owner accepts or rejects the proposal in the UI. Submitting against a stale revision
returns HTTP 409. Repeating a pending proposal for the same revision, requirement, test, and action
returns the existing proposal. `GET /api/proposals` includes the current `requirement`, catalogue
`test`, and a `conflict` message when a pending proposal cannot be approved. An older proposal can
still be approved if its target requirement and link remain compatible; its original `baseRevision`
is kept.
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
