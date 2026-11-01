# design #N

<!-- Changed from the Elephant-Goldfish Model `egm-draft` skill (Apache-2.0); see ../NOTICE. -->

Writes the plan of a change into the body of its issue, one section at a time with the user. It ends with `Status: designed`, and with `Status: approved` when the user approves.

## 1. Read

Read the issue, the map, the docs of the systems that the change touches, and the code that the plan changes.

`design` is a valid start: a change does not need `explore` first. If the problem is unclear, ask. If the direction is still open, suggest `explore` once. If the change has no issue, open one in step 3, when the user approves the TL;DR: `gh api repos/{owner}/{repo}/issues -X POST -f title="<the goal>" -F body=@FILE`.

## 2. Size

Set the size from its signals. One signal of a larger size is enough to move up.

| Size | Signals | Goldfish tests |
| --- | --- | --- |
| S | 1 system, no contract change, no new dependency between systems, up to 500 changed lines | none |
| M | 2 or 3 systems, a compatible contract change, a new dependency between systems, or up to 1,500 changed lines | critic |
| L | more than 3 systems, a breaking contract change, a new or split system, more than 1,500 changed lines, or a change that released devices cannot easily undo, such as the settings blob or the formats on the card | comprehension, critic and readiness |

When you are not sure between two sizes, take the larger one. Only the user makes a size smaller. An S design can be short: the TL;DR and a few plan steps.

## 3. Section by section

The body has this layout. Below `## Done when`, keep a section that `explore` wrote, such as Limits, when the design still needs it, and always keep a `## Prototype` section. Add `## Out of scope` only when the user or you want to say what this change must not include.

```text
Status: designed

## TL;DR

**Goal:** <what is true when the change is done, in one sentence>
**Problem:** <what is wrong now>
**Decision:** <the chosen direction>
**Rejected:** <each alternative that you or the user considered, and why, or none>
**Systems:** <the system slugs from the map>
**Contracts:** <the files in `specs/` or the system doc headers that change, or none>
**Size:** <S, M or L> (<its signals>)

## Plan

1. <a step that one review can check>

## Files

- `<path>`: <create, change or delete>: <what changes, and why>

## Done when

- <a check, with its command>
- <a step that only the user can do, such as a flash test on the board>
```

Rules for the content:

- A plan step names the file and the type or function that changes, not "update the reader". It gives no code, but it can give a signature or a format.
- Files lists every file that the plan changes, the tests and docs too.
- If the plan contradicts a decision of `explore`, show both and ask the user. The plan can be right, because `design` reads code that `explore` did not.

Send one message for each section, in order. For an S change, send all sections in one message.

1. Show the section's text in the chat.
2. Ask the questions of that section together. Each stands alone, as the router's rule 3 says.
3. End with: "Approve and continue, or tell me what to change."
4. Wait. Apply the changes, and write the section into the issue body. Keep the old `Status:` line until the Goldfish tests have run. A new issue has no `Status:` line until then.

## Epics

An L change is an epic. Its Plan lists its sub-issues, one line each: `- [ ] #N <title>`. When the user approves the Plan section, open each sub-issue: its own short TL;DR, its own `Status:` and `Size:` lines, a `Systems:` line that is a subset of the epic's, its plan steps, and `Depends on #x` when the order matters. Each sub-issue is one pull request.

## 4. Goldfish

Run the Goldfish tests of the size on the issue body, as [goldfish.md](goldfish.md) says. Fix the gaps in facts, and ask only the questions that need the user. Then set `Status: designed`.

## 5. Approval

Give the user the issue link, the size and your calls. When the user approves in the conversation, set `Status: approved`, on the sub-issues of an epic too, and say so. For an S change, the user's clear intent to build ("build it") is the approval. `build #N` can then start.
