# Repository agent instructions

Read and follow [`CONTRIBUTING.md`](CONTRIBUTING.md), especially its proportional-verification
policy. Run focused tests for the packages and surfaces changed. Do not run the complete workspace
or `obc check full` as a default handoff ritual; reserve full gates for the cross-cutting cases
listed there. Report the exact checks run and any checks intentionally omitted.

Use `obc clean` for stale worktree/build/test state. It is dry-run by default; never bypass its
dirty, unmerged, current-worktree, or main-worktree protections with ad-hoc deletion.

