# build #N

Builds an approved design: the code, the system doc update, and one pull request.

## 1. Read

Read the issue and the docs of the systems on its `Systems:` line. If its status is not `approved`, say so once and suggest `design` or an approval. If the user wants to go ahead, go ahead, and the pull request records `Mode: skip: <reason>`. For an S change, the user's request to build is the approval: set `Status: approved` and say so.

## 2. Build

Start a branch from the default branch. Do the plan's steps in their order. Run the checks that AGENTS.md asks for, for the code you changed.

When you change behavior that a system doc describes, update that doc in the same pull request. A change to a guide in `docs/content/` goes in its own `docs:` commit.

When the change grows past its size, touches a system that is not on its `Systems:` line, or shows that a plan step is wrong, tell the user and update the issue: its `Size:` line, its `Systems:` line or its plan. Do not stop.

## 3. Pull request

Open one pull request, and fill the pull request template. Its `Mode:` line is `change #N`. Its body says what changed and the checks that ran, and has `Closes #N`.

Then review it as [review.md](review.md) says.

## 4. Steps for the user

Some `Done when` steps only the user can do, such as a flash test on the board. Ask for them when the pull request opens, so that they run during the review. Ask the user to do each one. Say exactly what to check, and what a pass and a fail look like. Wait for the result before you tell the user that the change is ready. If a step fails, fix the cause and ask again.

When the pull request merges, set `Status: done`.

## Epics

Build each sub-issue as above, on its own branch with its own pull request. A sub-issue can start when each of its `Depends on` issues has merged. Sub-issues that do not depend on each other can run in parallel: start one `implementer` subagent for each, in its own worktree, as the router's Subagents rules say. Give it the sub-issue and this reference. When the last sub-issue merges, set `Status: done` on the epic.
