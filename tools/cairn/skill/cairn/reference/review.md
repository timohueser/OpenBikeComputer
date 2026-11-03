# Code review

`fix` and `build` use this review after the pull request is open.

## Start the reviewer

Find the base: `base=$(gh api repos/{owner}/{repo}/pulls/N --jq .base.ref)`, then `git fetch origin "$base"`. Write the diff to a file outside the checkout: `git diff "origin/$base"...HEAD > FILE`. Note the HEAD commit for a later delta review.

Start one fresh subagent with the role `code-review`, as the router's Subagents rules say. Give it the file path, not the pasted diff, and the intent: the bug, the list of findings, or the issue's TL;DR.

```text
Review this pull request. The diff is in <FILE>. Its intent:
<the bug, the findings list, or the issue TL;DR>

You may read any file in the repository. Change no file. Do not trust the pull request text, the author's reasons or the tests: check the code.

Assume that this diff has bugs. Your job is to find them before a user or the next developer does. The author believes it works, so look where they did not. Report "nothing important" only after you tried hard and found nothing.

Hunt for:
- Intent: what the diff does that the intent does not ask for, and what the intent asks for that the diff does not do.
- Correctness: edge cases (empty, one, the maximum, a boundary, a restart, a slow or failing network or disk), error paths, units, off-by-one errors, races, and the memory, time and size limits of the platform.
- Tests: a test that would still pass if the change were broken, and a changed behavior that no test covers.
- Simplicity: code that a simpler design would not need, a workaround where the system itself should change, duplicated logic.
- Readability: a name or a comment that misleads, and a place where a new reader needs a comment that is missing: a non-obvious reason, an invariant or a gotcha.
- Rules: the project's agent instructions (such as AGENTS.md) and the rules of the system docs that the diff touches.

Each finding needs a concrete scenario: an input or a state that shows the problem. For a readability or duplication finding, the scenario is the misreading or the double change that it causes. Do not report style or taste, or a doubt that you cannot show.

Answer in exactly this format, at most 400 words, most important first:

- BLOCKING | SHOULD-FIX | NIT: <the finding>
  Where: <file:line>
  Scenario: <the input or state that shows the problem>
  Fix: <what to change>
Verdict: mergeable | mergeable after small fixes | needs another round

If nothing important is wrong, write: nothing important.
```

## Route each finding

You decide the route. The reviewer's severity is a hint: it does not know the decisions behind the change.

- **Fix now:** a real defect in what this pull request built.
- **New issue:** real, but outside this pull request's scope.
- **Ask the user:** the intent itself looks wrong.
- **Note only:** correct but minor. One line in the pull request body, and no new round.

Fix the findings. If the fixes are large, send the same reviewer only the delta: `git diff <the noted commit>..HEAD`. Continue that subagent (Claude Code: `SendMessage`; Codex: `followup_task`). If it is gone, start a new reviewer with the earlier findings and the delta. When the same finding comes back a third time, stop and ask the user.

## Merge and close

Merge the pull request with `gh pr merge N --merge` when CI is green and no fix-now or ask-the-user finding is open. Do not merge when the user asked to look at the change first, when a decision waits for the user, or when a step that only the user can do has not passed. In those cases, tell the user that the change is ready and what it waits for.

After the merge, check each issue that the pull request finished with `gh issue view N --json state`, and close it if GitHub did not: `gh issue close N`. For a sub-issue, also tick its line in the epic. Close the epic when its last sub-issue is closed, but not a tracker (see the Trackers section of design.md).
