---
name: cairn
description: Keeps one design document for each system of this repository, and moves a designed change through the `Status:` line of its GitHub issue. Use it whenever the user asks to document a system or write a system doc; to review a document, a design or an issue design (not code and not a pull request); or to explore a problem or a new feature, plan or design a change, approve a design, implement or build an issue (#N), or fix a bug or the findings of an issue. Use it also when the request does not name cairn.
---

# cairn

## Rules for every command

1. **A change's issue holds the state.** Read it with `gh api repos/{owner}/{repo}/issues/N --jq .body` and find its `Status:` line. Write each result into the issue, not only into the conversation.
2. **The workflow never blocks.** When the user skips a step, say so once, in one line, and name the step you suggest. If the user wants to go ahead, go ahead. The pull request then records `Mode: skip: <reason>`.
3. **You propose, and the user corrects.** Each question stands alone: what you found, why it matters, the options and your recommendation. Ask all open questions together, not one at a time.
4. **Each subagent starts fresh and has a role.** See "Subagents" below.
5. **After compaction, read your command's reference again.** The reference files do not come back after compaction.
6. **Write plainly.** Before you write a doc, an issue, or a message that explains code, read [writing.md](reference/writing.md).

## The flow

Before the first edit, sort the task: a fix, a change, or a prototype.

- A change moves through its issue's `Status:` line: `explored`, `designed`, `approved`, `done`. A new session reads the line and knows the next step.
- Approval is a status, not a command. When the user approves a design in conversation, set `Status: approved` on the issue and say so. For a size S design, the user's clear intent to build ("build it") is the approval.
- A fix needs no issue and no design. If it grows past a small change, offer `design`.
- A prototype never merges. Its findings go into the issue.

To change the status line, write the edited body to a file, then run `gh api -X PATCH repos/{owner}/{repo}/issues/N -F body=@FILE`.

## Commands

Choose the command from the request. If two commands fit, ask once. Read the command's reference before you start.

| Command | Use it when | Reference |
| --- | --- | --- |
| `init` | once per project: the config, an empty map, a glossary draft | not written yet |
| `explore` | a new feature, or a problem whose direction is still open | [explore.md](reference/explore.md) |
| `design #N` | the direction is clear and the change needs a plan | [design.md](reference/design.md) |
| `build #N` | the design is approved | [build.md](reference/build.md) |
| `fix` | a bug, a small tweak, or the small findings of a findings issue | [fix.md](reference/fix.md) |
| `prototype` | an idea needs a quick test, with no quality bar | not written yet |
| `improve-architecture` | structural problems: cycles, mixed roles, code in the wrong place | not written yet |
| `document <system>` | a system doc needs to be written or reworked | [document.md](reference/document.md) |
| `review-doc <doc>` | a document needs a test by fresh readers | [review-doc.md](reference/review-doc.md) |

For a command with no reference, do the task with your own judgment.

## Subagents

- Get the model and effort of a role with `cairn model <role>`. It prints `model:` and `effort:`, or `inherit`, which means the session's own model. On Codex, add `--harness codex`.
- Give a subagent only what its role needs, never the conversation. On Codex, start it with `fork_turns: "none"`.
- Tell each subagent that its report has at most 400 words.
- Start independent subagents in parallel.
- If you cannot start a subagent, do its work yourself, from its inputs only. Start your output with `DEGRADED: <role> did not run independently.`

A Goldfish is a fresh subagent that tests one document. [goldfish.md](reference/goldfish.md) has its three tests. `design`, `document` and `review-doc` use it.

## Records

- Each pull request has one `Mode:` line: `change #N`, `fix`, or `skip: <reason>`. It is a record for the weekly report; nothing checks it.
- An issue's `Size:` line (S, M or L) sets how much design and review the change gets. Only the user makes a size smaller.

## Files and tools

| Path | Holds |
| --- | --- |
| `docs/design/README.md` | the map: every system, its purpose and its dependencies. Read it first to find the systems that a task touches. |
| `docs/design/<system>.md` | one system doc: a header (`owns`, `depends_on`, `status`) and the body |
| `docs/design/glossary.md` | the words to use and the words to avoid, when the file exists |
| `tools/cairn/README.md` | the tool's commands and rules |

`cairn <command>` means `python3 tools/cairn/cairn.py <command>`, or `obc cairn <command>` in this repository.

- `cairn check` finds files with no owner, undeclared dependencies, cycles and the R1 to R4 violations. CI runs it.
- `cairn map` writes the generated part of the map.
- `cairn leaves <system>` cuts a system's files into small leaves, for `document`.
- `cairn model <role>` prints the model and effort of a role.
