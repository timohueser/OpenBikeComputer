# review-doc <doc>

Tests one document with Goldfish: a system doc, an issue design, a guide page or any other document. It reviews documents only. Code review is part of `build`.

## 1. Find the document

The document is a path, or an issue `#N`. For an issue, the document is its body: `gh api repos/{owner}/{repo}/issues/N --jq .body`.

## 2. Choose the tests

| Document | Tests |
| --- | --- |
| a system doc in `docs/design/` | comprehension |
| an issue design, by its `Size:` line | S: none; M: comprehension; L: comprehension, critic and readiness |
| any other document | comprehension |

The user can ask for any test. When a design has size S, say in one line that no test runs, and offer comprehension. When a design has no `Size:` line, run comprehension and say that the line is missing.

## 3. Run the tests

Run the chosen tests as [goldfish.md](goldfish.md) says, and sort the findings into its three piles.

## 4. Fix and report

Fix the gaps in facts in the document yourself. For an issue, write the edited body back with `gh api -X PATCH repos/{owner}/{repo}/issues/N -F body=@FILE`.

Then give the user a short findings list, in this format:

```text
review-doc <doc>: <the tests that ran>
Fixed:
- <the gap>: <what you changed>
My calls:
- <the decision>: <your reason>
Questions:
- <each one standalone: what was found, why it matters, the options, your recommendation>
```

Leave out an empty section. If no test found anything important, say so in one line.
