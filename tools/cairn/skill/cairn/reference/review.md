# Code review

`fix` and `build` use this review after the pull request is open.

## Start the reviewer

Find the base: `base=$(gh api repos/{owner}/{repo}/pulls/N --jq .base.ref)`, then `git fetch origin "$base"`. Write the diff to a file outside the checkout: `git diff "origin/$base"...HEAD > FILE`. Note the HEAD commit for a later delta review.

Start one fresh subagent with the role `code-review`, as the router's Subagents rules say. Give it the file path, not the pasted diff, and the intent: the bug, the list of findings, or the issue's TL;DR.

```text
Review this pull request. The diff is in <FILE>. Its intent:
<the bug, the findings list, or the issue TL;DR>

You may read any file in the repository. Change no file. Do not trust the pull request text or the author's reasons: check the code.

Check two things, and keep them apart:
- Intent: does the diff do what the intent says, and nothing more?
- Quality: is it correct? Does it keep AGENTS.md and the system doc's rules? Do the tests prove the change?

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

The change is ready when CI is green and no fix-now or ask-the-user finding is open. Merge it only if the user asked you to. Otherwise tell the user that it is ready.
