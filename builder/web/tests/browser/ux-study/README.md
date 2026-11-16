# UX study

A study lets an LLM try the tasks of the web planner as a first-time rider, and writes a report of
the places where it failed or was surprised. The system doc is
[`docs/design/ux-study.md`](../../../../../docs/design/ux-study.md).

A person starts each study. CI never runs a study: it runs only the tests of this folder
(`npm run test:ux-study --prefix builder/web/tests/browser`). No agent reads the findings or starts
a study on its own. The owner gives out each fix.

## Run a study

```sh
./tools/obc ux                              # all tasks, the default model
./tools/obc ux --task plan-route            # one task
./tools/obc ux --model claude-haiku-5-5     # another model of study.js
```

`obc ux` builds the wasm bridges and the planner against the active planner release, then runs
each task three times at once. The study uses the network: the map, the routing and the search
of the release, and the Anthropic API.

### From a cloud session

Start the session in the cloud environment `OpenBikeComputer`. It holds `OBC_UX_API_KEY`, the
allowed hosts and the build tools. A session in another cloud environment does not get them.

### On the owner's machine

1. Put `OBC_UX_API_KEY=<key>` in `~/.config/openbikecomputer/credentials.env`, or set it in the
   environment.
2. Install the Chromium of Playwright once:

   ```sh
   npx --prefix builder/web/tests/browser playwright install chromium
   ```

## Cost and time

The command prints the cost and the time of the study, and the report header gives the model and
the cost. The build of the planner takes most of the time.

| Item | Where |
| --- | --- |
| Spending limit of one study | `maxCostUsd` in `study.js` |
| Models, their prices, and the default model | `models` and `defaultModel` in `study.js` |

At the spending limit, the study starts no more model calls. Calls that are in flight still
finish.

## The report

The command prints the path of the report: `.artifacts/ux-study/<UTC time>/report.md`. Each
walkthrough has a folder beside it with `steps.jsonl` and one JPEG for each screen. The report
links the screenshots. Nothing of a study is committed.

## Triage

The owner accepts or rejects each new finding.

| Decision | Record |
| --- | --- |
| Accepted | A GitHub issue with the label `ux-finding`, with the text and the screenshots of the finding. |
| Rejected | One line in [`rejected.md`](rejected.md): the place, then why the design is right as it is. Use the present tense. Write no date and no issue number. |

The next report compares its findings with the open `ux-finding` issues and with `rejected.md`.
It lists an open issue that it does not see again: close the issue when its fix is in.
