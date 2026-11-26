---
name: plan-review
description: Use before you build a large change in this repository (a new system, a changed file format or protocol, a change across several top-level directories, or a change the owner calls large). A fresh subagent reviews the plan in its GitHub issue before the build. Not for a small fix.
---

# Plan review

A plan that only its author read carries the author's blind spots into the build. For a large
change, the plan is in the GitHub issue of the change, and a fresh subagent reviews it there
before the build starts.

The reviewer sees only the issue, with none of your context, so it reads the plan as the next
person will: what is missing, what is wrong, what can be simpler. It runs on Opus unless the
owner asks for another model. The plan in the issue changes until it stands.
