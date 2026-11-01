# fix

Fixes a bug, makes a small tweak, or fixes the findings with the action `fix` in a findings issue.

## 1. Understand

- **A bug or a tweak:** read the map and the docs of the systems it touches. For a bug, reproduce it first, with a failing test when that is practical.
- **The findings of issue #N:** read the issue. Take only the findings with the action `fix`. A `refactor issue` item waits for its own `design`, and an `intended choice` needs no action.

If the change needs a design, say so once and offer `design`: for example, when it adds a `depends_on` entry, changes a file in `specs/`, or grows past a small change. If the user wants to go ahead, go ahead, and the pull request records `Mode: skip: <reason>`.

## 2. Change

When the user already works on a branch or a pull request, fix it there. Otherwise, start a branch from the default branch, not from another open pull request.

Make the smallest change that removes the bug or the finding. A bug fix gets a test that fails without the fix, when that is practical. A fix to a guide in `docs/content/` goes in its own `docs:` commit.

Run the checks that AGENTS.md asks for, for the code you changed.

## 3. Pull request

Open one pull request, and fill the pull request template. Its `Mode:` line is `fix`, or the `skip` of step 1. Its body says what was wrong, what changed, and the checks that ran.

For findings, edit the issue now: mark each fixed finding with the pull request link. Write `Closes #N` in the pull request body only when the issue has no `fix` or `refactor issue` item left.

Then review it as [review.md](review.md) says.

End with the pull request link and one line on what is still open.
