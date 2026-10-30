# fix

Fixes a bug, makes a small tweak, or fixes the small findings of a findings issue. It needs no design and no approval.

## 1. Understand

- **A bug or a tweak:** read the map and the docs of the systems it touches. For a bug, reproduce it first, with a failing test when that is practical.
- **The findings of issue #N:** read the issue. Take only the findings whose action is `fix`. Leave the refactor items and the intended choices.

If the change needs a design, say so once and offer `design`: for example, when it adds a dependency between systems, changes a doc header or a file in `specs/`, or grows past a small change. If the user wants to go ahead, go ahead, and the pull request records `Mode: skip: <reason>`.

## 2. Change

Work on its own branch. Make the smallest change that removes the bug or the finding. A bug fix gets a test that fails without the fix. A fix to a guide in `docs/content/` goes in its own `docs:` commit.

Run the checks that AGENTS.md asks for, for the code you changed.

## 3. Pull request

Open one pull request. Its `Mode:` line is `fix`. Its body says what was wrong, what changed, and the checks that ran. For findings, link the issue.

Then review it as [review.md](review.md) says.

## 4. Finish

For findings, when the pull request merges, edit the findings issue: mark each fixed finding with the pull request link. Keep the larger items open. When no item is left, close the issue.

End with the pull request link and one line on what is still open.
