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
secondary actions: Markdown import and export, group management, and history.

**Import Markdown** reads `## Group` headings and `- **ID — Title.** Statement` lines into the
draft. An entry whose ID already exists in the draft is updated; every other entry receives a fresh
`SYS` ID. Tests and labels are kept. **Export Markdown** writes the draft in the same format.
Neither changes a saved revision until you select **Save revision**.

**Revision history** shows each revision as a difference against the one before it: added,
removed, and changed requirements, with the previous and new statement side by side.

## Coverage workflow

Each requirement has a **Coverage** section: its acceptance criteria, the tests that are evidence
for each criterion, and the gaps that remain. Several tests can support one criterion; one test can
support several criteria. A requirement's tests are exactly the tests its plan cites. A criterion is
covered once it has evidence and no gap. The badge next to the heading shows one state per
requirement, with the count of covered criteria:

| State | Meaning |
| --- | --- |
| Not assessed | No plan has been saved. |
| Needs review | A plan saved under the earlier workflow and not yet approved. The next save approves it. |
| Partial | Approved, but gaps remain. |
| Covered | Approved, and every criterion has evidence. Only this state satisfies the release gate. |

Coverage and test results are separate. An active requirement needs covered, approved coverage and
passing results for all the tests its plan cites. Test names alone cannot establish coverage; an
owner judges whether the criteria capture the whole requirement and the evidence supports each claim.
A candidate exception remains a separate administrator decision.

The **Coverage** tab shows the same states for the whole revision: covered requirements, covered
criteria, the catalogue tests that plans cite, and a chart of covered requirements per saved
revision. It reads the saved revisions and needs no extra bookkeeping.

### Edit coverage yourself

Select **Define coverage** or **Edit coverage** on a requirement. The plan is part of the
requirement draft, like its statement and tests: write each criterion, add evidence, say what each
test proves, and note the gap for anything still missing. The checkmarks and the count update as
you edit. Below the gap, a criterion can name the next test to build: a level (unit, integration or
system) and one sentence. A criterion with no gap has nothing left to build, so it takes no next
test. Evidence comes from the CI catalogue or from a manual procedure; **+ New manual procedure**
in the picker creates one on the spot, **Refresh catalogue** gets tests from a new CI import, and
**Edit procedure** on manual evidence changes its steps, expected result, and input files. The plan
attaches the tests: **Remove** on a piece of evidence unlinks its test. A manual procedure that no
criterion cites is deleted with its steps, so removing its last citation asks you to confirm.

**Save revision** stores the plan with the requirement and approves it, exactly like saving the
statement: a revision is an owner's act. The review records the CI catalogue commit of that moment
for the report and does not create test results. A save also approves any plan that was saved
under the earlier workflow and still waits for approval, whichever requirement the save touched.

### Review an agent's proposal

An agent can propose an initial plan or revise accepted coverage. Proposals appear on their
requirement, above the current plan; the sidebar marks the requirement and the **proposals to
review** button in the heading jumps to the next one. A proposal shows new, changed, and removed
criteria, with new evidence marked and removed evidence struck through. A manual procedure the
proposal brings is marked **new procedure**, with its steps collapsed. If approval deletes a
manual procedure that the plan cites nowhere, a sentence above **Approve** names each one.
**Approve** saves the plan, applies the links, creates the new procedures, and records your
approval for the commit the agent assessed, in one action. **Reject** asks for feedback, which the agent can read before submitting a revision. Agents
cannot approve coverage.

Save or discard requirement drafts before approving. A proposal cannot be approved once its
requirement is deleted or its evidence has left the catalogue. If the statement, the tests, or the
plan changed after the agent assessed them, a warning names what changed; approval stays possible,
so a typo fix or a removed obligation does not force a new proposal. Changes to other requirements
do not affect a proposal. After a decision, the view stays on the requirement; the **proposals to
review** button opens the next one. While a proposal is on screen, Ctrl+Enter (or ⌘+Enter)
approves it and Escape closes the feedback box.

### When a requirement changes

A change to the statement, the tests, or the plan itself keeps the criteria, evidence, and gaps,
and the save renews the review with your name and the current catalogue commit. Renaming a
requirement, changing its group, or changing labels keeps the existing review, including one that
came from an agent's proposal. Labels still apply their own release gates. Changes to another
requirement do not touch this requirement's review. Pending proposals for a changed requirement
show a warning, as described above.

The review records one source commit for the report: the commit the agent assessed, or the CI
catalogue commit when you save the plan yourself. The release gate checks that every test the
plan cites is present and passes in the candidate. A new source commit thus does not clear an approval. Existing candidates keep their original requirement and coverage snapshots.
Published reports remain frozen.

### Local demonstration

Run with Node 24 from the repository root:

```sh
npm run demo:coverage --prefix apps/obc-verification
```

Open `http://127.0.0.1:4180` and sign in as `demo` with password `local-coverage-demo`.
The disposable database starts with accepted plans and proposals that revise them:

- **SYS-039**: an approved partial plan with a pending proposal above it. The proposal replaces an
  obsolete smoke test with selection evidence and brings a new ride-check procedure with a next
  test to build; the omitted test is unlinked, persistence keeps its gap, and approval keeps
  coverage partial. Or select **Edit coverage**, add a manual procedure as
  evidence, and save the revision.
- **SYS-030**: a demo addition to the requirement statement, saved by the owner. Its proposal
  records the added zoom obligation as a gap. Its plan also cites a manual procedure.
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

**Delete requirement**, in the requirement editor, removes the selected requirement and its tests
from the draft after confirmation. **Undo** restores it before saving. Select **Save revision** to
save the deletion.
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
private credential store. Rotate the CI and GitHub credentials independently. Create agent
tokens in **Account → Agent access**. The service does not accept `VERIFICATION_AGENT_TOKEN`;
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
change release evidence, uploaded files, accounts, the CI catalogue, or backup archives. Old
coverage proposals are removed. Revision numbers continue to increase so that an older open editor cannot
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

An administrator can create a separate token for each agent or machine. Sign in with your
approved GitHub admin account or the local administrator. No GitHub personal access token, SSH
connection, or service restart is required.

1. Open **Account → Agent access**.
2. Enter a token name that identifies the machine or task.
3. Choose the day it expires, at most one year ahead. The default is 30 days.
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
`DELETE /api/admin/agent-tokens/ID`. Creation takes `{ "name": "Coverage review", "expiresAt": "2027-01-31T23:59:59Z" }`, an ISO timestamp with an explicit time zone at most a year ahead,
and returns `{ "token": "...", "access": { ... } }`. Listing returns metadata only. These endpoints
require an administrator session; writes also require the exact configured browser origin.

Use `Authorization: Bearer TOKEN` with the dedicated agent token. The token can read
`GET /api/bootstrap`, `/api/revisions`, `/api/revisions/ID`, `/api/catalog`, `/api/candidates`,
`/api/candidates/ID`, and `/api/coverage-proposals`. These responses use the same definitions as the owner interface.
Download a retained attachment with `GET /api/files/ID`.

### Propose coverage

Prefer a whole coverage plan when assessing a requirement. Read the current requirement and its
existing coverage first. Audit the source at an exact commit. Identify every obligation, preserve
stable criterion IDs when revising a plan, map tests by their actual assertions, and describe
missing tests or implementation in `gap`. When you know the test to build for a gap, set `next`: one
sentence with a `level`. The level says how much of the product the test exercises: `unit` for one
module, `integration` for several together, and `system` for the assembled product. It does not say
who runs the test — automated evidence cites a catalogue case and manual evidence cites a procedure,
so the plan already carries that. It also does not say where the test runs: a rig test and a CI
test of the same scope have the same level, and the suite in the case ID identifies the rig.

Each evidence entry takes the same `level`, for the test it cites. It is optional, because plans
written before it existed do not have one. Set it when you know it. The requirement page shows it.

A manual test also takes `manualReason`:

- `human` — a person must always run this test. For example, a real phone with a real device, or a
  check that needs human judgement. A release runs these tests by hand.
- `until-automated` — a person runs this test because no automated test exists yet.

Set it. A release must schedule the `human` tests; the `until-automated` tests are a backlog.

Prefer an automated test. When no automated test can prove a criterion, propose a manual procedure
instead: put it in `procedures` with its steps and expected result, and cite its `id` as `testId`
evidence; approval creates it on the requirement. Do not change the requirement title or statement.

**A manual procedure is complete evidence.** A criterion it covers gets a checkmark, like a
criterion with an automated test. Leave `gap` empty. Do not use `gap` to record that the evidence is
manual, and do not repeat the procedure as the next test to build. Both make a covered criterion
look unfinished. The release gate asks separately for a manual pass on the candidate, which is when
the procedure is run.

Use `gap` only for something that is missing. A manual test that an automated test must replace is
missing something: record that gap, and let `next` name the automated test. A manual test that only
a person can run is not a gap.

Keep `rationale` to two or three sentences: the scope of the audit and the conclusion. The
criteria carry the detail. A plan is covered when every criterion has evidence and no gap;
otherwise it is partial.
Proposing a plan never approves it.

A case ID contains the test's own describe and it names. If you rename or move a test that a plan
cites as evidence, the old case ID no longer exists, and the pending proposals that cite it cannot
be approved. Find the test in the catalogue, and send the plan again with its current ID.

Read decisions and reviewer feedback with `GET /api/coverage-proposals`. Send
`POST /api/coverage-proposals` with:

```json
{
  "baseRevision": 7,
  "requirementId": "SYS-039",
  "sourceSha": "EXACT_40_CHARACTER_COMMIT_SHA",
  "plan": {
    "rationale": "Projection is tested and persistence is covered by a ride check. User selection does not exist yet.",
    "criteria": [
      {
        "id": "projection",
        "statement": "Both orientations render correctly.",
        "evidence": [
          { "caseId": "REAL_NORTH_UP_CATALOGUE_ID", "rationale": "Checks north-up projection.", "level": "unit" },
          { "caseId": "REAL_HEADING_UP_CATALOGUE_ID", "rationale": "Checks course rotation.", "level": "unit" }
        ],
        "gap": ""
      },
      {
        "id": "selection",
        "statement": "The user can select either orientation.",
        "evidence": [],
        "gap": "The setting does not exist yet, so nothing can select an orientation.",
        "next": { "level": "unit", "summary": "Set each orientation through the settings store and assert the stored value." }
      },
      {
        "id": "persistence",
        "statement": "The choice survives a restart.",
        "evidence": [{ "testId": "ride-restart", "rationale": "Confirms the choice on the device after a power cycle.", "level": "system" }],
        "gap": ""
      }
    ]
  },
  "procedures": [
    { "id": "ride-restart", "title": "Ride check after restart", "manualReason": "human", "steps": "1. Set heading-up.\n2. Power the device off and on.\n3. Start a ride.", "expected": "The map stays heading-up." }
  ]
}
```

Replace all placeholder IDs and the source SHA. For an existing manual check, use `testId` with
its ID; for a new one, add it to `procedures` and cite it the same way. Every proposed procedure
must be cited. Each evidence entry must identify exactly one test and explain its assertions. The server rejects unknown tests and duplicate criteria. A
criterion may have no evidence yet: an agent can propose the criteria first and evidence later,
with or without a `gap` note.
The requirement's tests become exactly the tests the plan cites. A plan that omits a test therefore
unlinks it, and a manual procedure that no criterion cites is deleted with its steps. Approval
applies the criteria, the test links, and the coverage review in one transaction. The top-level
source SHA identifies the commit the agent assessed; it is not a claim that tests have run, and
approval records it.

The response contains the pending proposal ID and agent attribution. A requirement has at most one
pending proposal. An identical pending request is reused; a different plan replaces the pending
proposal, which stays in history as superseded. After rejection, read `feedback` and submit a new
proposal against the latest requirement revision. An agent cannot accept or reject plans. Owners
edit the plan as part of the requirement draft: `PUT /api/requirements` accepts a `coverage` plan
on each requirement, validates it the same way, makes the requirement's tests the tests that plan
cites, and never accepts an approval from the client: the save itself records the owner's review
on each plan it changes. It requires an owner session and exact browser origin. Owner review of
an agent proposal uses `POST /api/coverage-proposals/ID` with `{ "accept": true, "feedback": "" }`.

Approval rechecks the requirement, its tests, the current coverage plan, and the catalogue.
Changes to that requirement require a refreshed proposal; other requirements can be approved
sequentially. After an approval, prepare a new candidate to use the saved coverage snapshot.

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
