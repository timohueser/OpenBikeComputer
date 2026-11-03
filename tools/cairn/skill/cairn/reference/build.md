# build #N

Builds an approved design: the code, the system doc update, and one pull request.

## 1. Read

Read the issue and the docs of the systems on its `Systems:` line. For an S change, the user's request to build is the approval: set `Status: approved` and say so. Otherwise, if its status is not `approved`, say so once and suggest `design` or an approval. If the user wants to go ahead, go ahead, and the pull request records `Mode: skip: <reason>`.

## 2. Build

Start a branch from the default branch, in a new worktree of your own (`git worktree add`, or the worktree tool of the harness). Other sessions can share this checkout, and a branch switch there changes their files. Do the plan's steps in their order. Run the checks that the project's agent instructions (such as AGENTS.md) ask for, for the code you changed.

When you change behavior that a system doc describes, update that doc in the same pull request.

When the change grows past its size, touches a system that is not on its `Systems:` line, or shows that a plan step is wrong, tell the user and update the issue: its `Size:` line, its `Systems:` line or its plan. Do not stop.

## 3. Pull request

Open one pull request, and fill the pull request template. Its `Mode:` line is `change #N`, or the skip of step 1. Its body says what changed and the checks that ran, and has `Closes #N`.

Some `Done when` steps only the user can do, such as a test on real hardware or a manual check in the app. Ask the user for each one now, so that it runs during the review. Say exactly what to check, and what a pass and a fail look like. If a step fails, fix the cause and ask again.

Then review it as [review.md](review.md) says. It merges only after those steps pass. When the pull request merges, set `Status: done`.

## Epics

Build each sub-issue as above, on its own branch with its own pull request. A sub-issue can start when each of its `Depends on` issues has merged. Sub-issues that do not depend on each other can run in parallel: start one `implementer` subagent for each, in its own worktree, as the router's Subagents rules say. Give it the sub-issue and this reference. When the last sub-issue merges, set `Status: done` on the epic, but not on a tracker, which has no `Status:` line.
