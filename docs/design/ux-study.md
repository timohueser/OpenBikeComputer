---
system: ux-study
kind: system
area: Tooling
purpose: "Lets an LLM try tasks of the web planner as a first-time user, and records where it fails or is surprised."
owns:
  - builder/web/tests/browser/ux-study/**
depends_on: []
test_depends_on:
  - library
  - repository-tooling
  - route-planning
  - search
status: draft
---

# UX study

## What it does

A study lets an LLM (the participant) try the core tasks of the web route planner as a rider who never saw it. It then writes one short report of the places where the participant failed or was surprised. The owner reads the report and accepts or rejects each finding. The [README](../../builder/web/tests/browser/ux-study/README.md) says how to run a study and how to triage.

`obc ux` builds the planner against the active planner release and runs [`run.js`](../../builder/web/tests/browser/ux-study/run.js). A study goes like this:

1. For each task of [`study.js`](../../builder/web/tests/browser/ux-study/study.js), three walkthroughs (tries of the task) run at the same time, each in a new browser context with an empty library.
2. In each step, the participant gets the goal, its own earlier answers, and the screenshots before and after its last action. It answers with one step record: the actions that it sees, what it expects, the action that it chooses, whether the last result matched its expectation, and how confusing the screen is.
3. The run loop performs the action, waits until the page is stable, and adds the screen and the control under the point to the record. The participant never gets these two fields back.
4. The walkthrough ends when the task's `success` function returns true, when the participant gives up or repeats one action three times, or when the step budget ends. The participant then names the step that was the hardest for it, or no step. The run loop adds the answer to the record of that step and never sends it back.
5. Code selects the steps with a strong signal (the result did not match, the screen confused the participant most), a mild signal (the result matched partly, the screen caused doubt, the participant named the step as the hardest), or the end of a walkthrough without success. It groups them by task, screen and control, and puts the groups with a strong signal before the groups with only mild ones. One model call writes the findings from the groups, the open accepted findings, the rejected findings and the design principles of [`PRODUCT.md`](../../PRODUCT.md).

The study writes `report.md`, one `steps.jsonl` and one JPEG for each screen to `.artifacts/ux-study/<UTC time>/`. The map, the routing and the search come from the public hosts of the release, and the model calls go to the Anthropic API with the key `OBC_UX_API_KEY`.

A task's `success` reads the route records of the library (the IndexedDB database `obc-library`, with the record shape of [`records.ts`](../../builder/web/src/lib/library/records.ts)). The task `lunch-stop` also reads the points of the plan in the record ([`editor.ts`](../../builder/web/src/lib/planner/editor.ts)), and uses the food kinds of the place search ([`place-kinds.json`](../../planner/search/place-kinds.json)). No test reads the first two files, so a change to the record shape shows only in a study: a walkthrough then never ends as `done`.

## Rules that must hold

| Rule | Check |
| --- | --- |
| The participant gets back only its own answers, never the fields that the loop reads from the page. | `the participant gets back only its own fields, never the fields that the loop adds from the page` in [`loop.test.js`](../../builder/web/tests/browser/ux-study/loop.test.js) |
| Only the same action on the same screen three times ends a walkthrough as stuck. | `the same action on the same screen three times ends as stuck` in [`loop.test.js`](../../builder/web/tests/browser/ux-study/loop.test.js) |
| Each task accepts exactly the route records that reach its goal. | `each task accepts exactly the route records that reach its goal` in [`study.test.js`](../../builder/web/tests/browser/ux-study/study.test.js) |
| A failed end question does not change the outcome of a walkthrough. | `a failed end question keeps the outcome of the walkthrough, and its calls count toward the cost` in [`loop.test.js`](../../builder/web/tests/browser/ux-study/loop.test.js) |
| A failed model call costs money too, and counts toward the spending limit. | `a call that fails three times ends as error, and the failed calls count toward the cost` in [`loop.test.js`](../../builder/web/tests/browser/ux-study/loop.test.js) |
| At the spending limit, the study starts no more participant calls. | `the spending limit stops the participant calls and ends the walkthrough as error` in [`loop.test.js`](../../builder/web/tests/browser/ux-study/loop.test.js) |
| A group of steps goes to the report call only when two walkthroughs show it or a walkthrough failed there. | `a group stays when two walkthroughs show it or a walkthrough failed there; failed groups first, then groups with a strong signal` in [`report.test.js`](../../builder/web/tests/browser/ux-study/report.test.js) |
| Code, not the model, counts the walkthroughs of a finding. | `the report counts the walkthroughs of a finding from its steps, links each step to its screenshot, and says when gh failed` in [`report.test.js`](../../builder/web/tests/browser/ux-study/report.test.js) |
| A report says that an open finding was not seen only when the study completed and the report call answered. | `a report compares the open findings only when the study completed and the report call answered` in [`report.test.js`](../../builder/web/tests/browser/ux-study/report.test.js) |
| The report call gets the design principles. | `the report call gets the design principles of PRODUCT.md` in [`report.test.js`](../../builder/web/tests/browser/ux-study/report.test.js) |
| An unknown model stops the study before it spends anything. | `an unknown model stops the study before it starts, and the error lists the known models` in [`run.test.js`](../../builder/web/tests/browser/ux-study/run.test.js) |

## Decisions

- **Code decides when a task is done.** `success` reads the state of the app in the page. The word of the participant never counts, because an LLM can say that it is done when it is not. A task whose success code cannot check is not used.
- **One participant, the newcomer.** It takes the first option that seems plausible. Several participant types and test modes come back only when a session with a human rider shows that the study misses something.
- **Findings, not statistics.** The report has two numbers: the walkthroughs that completed their task, and the open accepted findings. Agents finish tasks more often and faster than people, so success rates and scores say nothing about riders. The study keeps no baselines and no trends.
- **A rejected finding is a line in [`rejected.md`](../../builder/web/tests/browser/ux-study/rejected.md).** Most findings of an LLM are wrong, so closed issues would outnumber the real ones. The report call compares by meaning, not by a key: to reject a repeated finding again takes seconds, and a key would need upkeep.
- **The spending limit lets calls in flight finish.** The study adds the cost of each call after it answers. A study can pass the limit by one call for each walkthrough that runs at the same time; the limit has a margin for this. A reservation of cost before each call would be more code for less than 1 USD.
- **A person starts each study.** A study costs money and its result differs from one study to the next, so CI never runs it. CI runs only the tests of the run loop and the report, with stand-ins for the browser and the model. No agent reads the findings or starts a study on its own.
- **Only [`environment.js`](../../builder/web/tests/browser/ux-study/environment.js) knows the browser.** The run loop, the participant and the report use the environment through a few functions: start and end a walkthrough, take a screenshot, perform and describe an action. A study of the device or the iOS app then needs only a new environment.
- **The key is `OBC_UX_API_KEY`, not `ANTHROPIC_API_KEY`.** A Claude Code session would use a variable of that name for its own model calls.
- **No fallback model after a refusal.** A fallback model has other prices, and the budget would then be wrong.

## Words

- **Walkthrough:** one try of one task by the participant, step by step.
- **Step record:** what the participant answers for one step, with the screen and the control that the run loop adds, and the end answer when the participant named this step as the hardest.
- **Finding:** one fault of the planner that walkthroughs show, with the steps that are its evidence.
- **Triage:** the owner accepts each finding as a GitHub issue with the label `ux-finding`, or rejects it with a line in `rejected.md`.
