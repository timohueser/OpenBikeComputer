# explore

Explores a new feature or a change when the problem or the direction is still open. It writes no code and no plan. It ends with an issue at `Status: explored`.

## 1. Read

Read the issue if there is one, the map, and the docs of the systems that the problem touches. Read code only to check a fact. If a question needs a test in code, offer `prototype`.

## 2. Ask

Find what you do not know: the problem, who has it, the limits (memory, power, the formats on the card, released devices) and what is out of scope. Ask these questions together, as the router's rule 3 says.

## 3. Directions

The user runs `explore` to find a new feature, or a new way to solve a problem that earlier attempts did not solve. So be bold: search beyond the first idea, and give at least one direction that is not the obvious one. Do not reject an idea only because it is unusual.

Give 2 or 3 directions. For each, say in a few sentences how it works, what it costs and when it fails. Recommend one, with the reason. The user chooses. Keep each rejected direction with its reason, so that nobody proposes it again without new facts.

Repeat steps 2 and 3 while the user corrects you. Do not ask again what the user already answered.

## 4. Write the issue

Write this body. Open a new issue with `gh api repos/{owner}/{repo}/issues -X POST -f title="<the problem>" -F body=@FILE`, or replace the body of issue #N and keep its `## Prototype` sections.

```text
Status: explored

## Problem
<what is wrong, for whom, and how we know>

## Limits
<what the solution must keep>

## Out of scope
<optional: what this change must not include>

## Directions
- Chosen: <the direction>: <why>
- Rejected: <a direction>: <why>

## Open questions
<what `design` must answer, or none>
```

End with the issue link and one line: the next step is `design #N`.
