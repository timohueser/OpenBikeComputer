# Verification console

A SvelteKit application that stores system requirements, their coverage plans, linked tests,
immutable revisions and release evidence. The database is the authority; Git holds the
implementation, not an editable copy of requirement prose.

Server setup, deployment, backups and credential rotation are in
[`ops/verification-console.md`](../../ops/verification-console.md).

## Local development

Node 24.21 or a later Node 24 release:

```sh
npm ci
npm run dev
npm run check
npm test
npm run build
```

Set `VERIFICATION_DATA_DIR` to a private directory (default `./data`) and `ORIGIN` to the exact
browser origin, including the port. Configure an owner login before opening the application: there
is no default password and no anonymous editing mode.

A disposable demo with seeded plans, proposals and suggestions:

```sh
npm run demo:coverage --prefix apps/obc-verification
```

Open `http://127.0.0.1:4180` and sign in as `demo` with password `local-coverage-demo`. It uses a
separate temporary database, disables GitHub integration and makes no production change. Restart
the command to reset it. `VERIFICATION_DEMO_PORT` selects another port.

## Layout

| Path | What is in it |
| --- | --- |
| `src/lib/server/api.ts` | The whole HTTP surface; every route below is dispatched here |
| `src/lib/server/store.ts` | Revisions, candidates, proposals and suggestions |
| `src/lib/server/coverage-plan.ts` | Plan validation, shared by the agent and owner write paths |
| `src/lib/server/auth.ts`, `accounts.ts` | Sessions, GitHub OAuth, agent and CI tokens |
| `src/routes/api/[...path]` | The single catch-all endpoint |
| `ops/` | The server scripts and unit files the runbook installs |

## Coverage states

A plan is its requirement's acceptance criteria, the evidence for each one, and the gaps that
remain. Several tests can support one criterion and one test can support several. A requirement's
tests are exactly the tests its plan cites.

| State | Meaning |
| --- | --- |
| Not assessed | No plan has been saved. |
| Needs review | A plan saved under the earlier workflow. The next save approves it. |
| Not covered | Approved, but no criterion has evidence yet. |
| Partial | Approved, and at least one criterion has evidence. Gaps remain. |
| Covered | Approved, and every criterion has evidence. Only this state satisfies the release gate. |

Coverage and test results are separate. An active requirement needs approved, covered coverage
**and** passing results for every test its plan cites. Saving a revision is the owner's act of
approval; an agent can never approve.

New requirements get sequential IDs such as `SYS-028`. The server reserves each number when a
draft is created. Deleted and discarded numbers are not reused.

## Agent and CI API

Use `Authorization: Bearer TOKEN`. An administrator creates one token per agent or machine in
**Account → Agent access**; the secret is shown once. Save it at
`~/.config/openbikecomputer/verification-agent.token`, directory mode 0700 and file mode 0600:

```sh
install -d -m 700 ~/.config/openbikecomputer
install -m 600 ~/Downloads/verification-agent.token ~/.config/openbikecomputer/verification-agent.token
rm ~/Downloads/verification-agent.token
```

Give the agent the file path. **Never** paste the secret into chat, a URL, command history or a
log. The server stores only a SHA-256 hash of it and checks expiry, revocation and the issuing
administrator's current access on every request. **Revoke token** ends access at once.

| Route | Method | Purpose |
| --- | --- | --- |
| `/api/bootstrap` | GET | Actor, latest revision, catalogue and candidates in one response |
| `/api/revisions`, `/api/revisions/N` | GET | Saved revisions, and one revision in full |
| `/api/catalog` | GET | The CI test catalogue |
| `/api/candidates`, `/api/candidates/ID` | GET | Release candidates and their evidence |
| `/api/files/ID` | GET | One retained attachment |
| `/api/coverage-proposals` | GET, POST | Read decisions and feedback; submit a plan |
| `/api/coverage-proposals/ID` | POST | `{ "accept": false, "feedback": "…" }` — rejection only |
| `/api/requirement-suggestions` | GET, POST | Read decisions; suggest a requirement or a change |
| `/api/requirement-suggestions/ID` | POST | The owner's decision, or `{ "reopen": true }` to take it back |
| `/api/requirements` | PUT | Owner write: draft, plans and `accept` of proposal IDs |
| `/api/admin/agent-tokens`, `/api/admin/agent-tokens/ID` | GET, POST, DELETE | Token management |

The agent token cannot write requirement prose, upload files, record manual outcomes or publish.
Owner and admin writes need a session and the exact configured browser origin. CI uses its own
token for catalogue imports, firmware uploads and candidate results; skipped, missing, failed or
ambiguous results cannot satisfy a linked test.

### Propose coverage

1. Read the current requirement and its existing coverage.
2. Audit the source at an exact commit. Identify every obligation the statement carries.
3. Write one criterion per obligation. Keep stable criterion IDs when you revise a plan.
4. Map each piece of evidence to a test by its actual assertions, and say what it proves. Set
   `level`: `unit` for one module, `integration` for several together, `system` for the assembled
   product. The level is scope, not who runs the test and not where it runs.
5. Prefer an automated test. When no automated test can prove a criterion, put a manual procedure
   in `procedures` and cite its `id` as `testId`. Every proposed procedure must be cited. Give it
   a `manualReason`: `human` when a person must always run it, `until-automated` when no automated
   test exists yet. A release schedules the `human` ones; the others are a backlog.
6. Describe what is missing in `gap`, and name the test to build in `next`. **A manual procedure
   is complete evidence**: leave its `gap` empty. Use `gap` only for something that is missing.
7. Keep `rationale` to two or three sentences. The criteria carry the detail.
8. `POST /api/coverage-proposals`. Run `obc req propose --check` first.

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
          { "caseId": "REAL_NORTH_UP_CATALOGUE_ID", "rationale": "Checks north-up projection.", "level": "unit" }
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
        "evidence": [{ "testId": "ride-restart", "rationale": "Confirms the choice after a power cycle.", "level": "system" }],
        "gap": ""
      }
    ]
  },
  "procedures": [
    { "id": "ride-restart", "title": "Ride check after restart", "manualReason": "human", "steps": "1. Set heading-up.\n2. Power the device off and on.\n3. Start a ride.", "expected": "The map stays heading-up." }
  ]
}
```

The server rejects unknown tests and duplicate criteria. A requirement has at most one pending
proposal: an identical request is reused, and a different plan replaces the pending one. After a
rejection, read `feedback` and submit again against the latest revision.

**A plan that omits a test unlinks it**, and a manual procedure that no criterion cites is deleted
with its steps.

**A case ID contains the test's own describe and it names.** Renaming or moving a cited test
breaks every pending proposal that cites it. Find the test in the catalogue with `obc req tests`
and send the plan again with its current ID.

### Suggest a requirement

An agent never writes or edits a requirement. When a change needs a requirement that does not
exist, or a requirement no longer describes the product, suggest it and let the owner write it.
Acceptance is an acknowledgment only; it changes no draft.

1. Write the full replacement title and statement, not a patch.
2. Keep `reason` to two or three sentences: the observation behind the suggestion.
3. `POST /api/requirement-suggestions` with `baseRevision` at the current revision, `group` for a
   new requirement, and `requirementId` only for a change. `obc req suggest suggestion.json`
   validates, fills in the base revision and commit, and submits; use `--check` first.

```json
{
  "baseRevision": 7,
  "requirementId": "SYS-030",
  "sourceSha": "EXACT_40_CHARACTER_COMMIT_SHA",
  "title": "Say what happens without a fix",
  "statement": "The user shall be able to exit pan/zoom mode with one action. Without a position fix, the map shall follow the last known position and show that the fix is missing.",
  "group": "Map display on device",
  "reason": "The return action only recenters when a fix is present. The statement does not say what happens without one, so criterion 1 cannot be tested for that case."
}
```

An open suggestion for a changed requirement carries `stale`; one for a deleted requirement
carries `missing`. A new suggestion for the same requirement replaces the open one, which stays
in history as superseded. `obc req suggestions --decided` shows the owner's answer.

## Clear requirement history

**This cannot be undone in the interface.** An administrator opens **Account → Maintenance**,
reads the preview of how many revisions go and how many stay, and enters the displayed
confirmation phrase. The default keeps the current requirements and their linked tests in a new
revision; **Start fresh** clears those too.

Revisions that a release candidate or a publication references stay available. The action changes
no release evidence, uploaded file, account, catalogue entry or backup archive. Old coverage
proposals are removed. Revision numbers keep increasing, so an older open editor cannot save over
the fresh state.
