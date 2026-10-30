# explore

Explores a change when the problem or the direction is still open. It writes no code and no plan. It ends with an issue at `Status: explored`.

## 1. Read

Read the issue if there is one, the map, and the docs of the systems that the problem touches. Read code only to check a fact. If a question needs a test in code, offer `prototype`.

## 2. Ask

Find what you do not know: the problem, who has it, the limits (memory, power, the formats on the card, released devices) and what is out of scope. Ask these questions together, as the router's rule 3 says.

## 3. Directions

Give 2 or 3 directions. For each, say in a few sentences how it works, what it costs and when it fails. Recommend one, with the reason. The user chooses. Keep the reason for each rejected direction: `design` needs at least one.

Repeat steps 2 and 3 while the user corrects you. Do not ask again what the user already answered.

## 4. Write the issue

Write this body. Open a new issue with `gh api repos/{owner}/{repo}/issues -X POST -f title="<the problem>" -F body=@FILE`, or replace the body of issue #N.

```text
Status: explored

## Problem
<what is wrong, for whom, and how we know>

## Limits
<what the solution must keep, and what is out of scope>

## Directions
- Chosen: <the direction>: <why>
- Rejected: <a direction>: <why>

## Open questions
<what `design` must answer, or none>
```

End with the issue link and one line: the next step is `design #N`.
