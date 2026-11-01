# prototype

Answers one question with throwaway code. The code never merges. The answer goes into the issue.

## 1. Question

Write the question in one sentence, and say which decision the answer changes. For example: "Does a full refresh during a map scroll show on the panel? If it does, we need partial updates." If the request has no clear question, ask once.

Read the issue if there is one. If there is no issue, open one in step 4.

## 2. Branch

Start a branch `prototype/<short-name>` from the default branch. Do not open a pull request. Put the code next to the code it tests, and put `prototype` in its name, so that a reader knows what it is.

## 3. Build the smallest thing that answers

There is no quality bar: no tests, no docs, no system doc update, no error handling that the question does not need. `cairn check` and the rules of the system docs do not apply on this branch.

- Use a computer when it can answer the question. Use the board only when the question needs real hardware.
- Give one command that runs the prototype.
- Show the values that the answer depends on: print them, or draw them on the screen.
- If only the user can run it, say exactly what to run, what to look at, and what each result means.

Stop when the question has an answer. If the question changes, or the work grows past a small test, tell the user once and follow their choice.

## 4. Record

Commit the code and push the branch, so that the issue can link it. Add this section to the issue, or open a new issue with this section and `Status: explored`:

```text
## Prototype: <the question>

**Answer:** <the answer, in one or two sentences>
**Evidence:** <what you ran and what you measured or saw>
**Keep:** <what the design must use or avoid>
**Branch:** <the link>
```

An answer can be "no" or "we still do not know". Record it in the same way, so that nobody tries the same thing again without new facts.

End with the issue link, the answer in one line, and the next step: `explore #N` when the direction is still open, or `design #N` when it is clear. `build` writes the production code again from the design; it does not merge this branch.
