# Goldfish

A Goldfish is a fresh subagent with no memory of the conversation. It gets one document and the files that the document links to, and nothing else. If it cannot understand or use the document, a later reader cannot either.

There are three tests. The command that calls Goldfish says which ones run:

| Test | Role | Asks |
| --- | --- | --- |
| comprehension | `goldfish-comprehension` | Can you explain this from the document alone? |
| critic | `goldfish-critic` | What is missing or wrong? |
| readiness | `goldfish-readiness` | Which decisions would you still have to make? |

## Run the tests

Start one subagent for each test, in parallel, as the [Subagents](../SKILL.md#subagents) rules of the router say. Write its prompt as below. The full text of the document comes first, before any word of yours, so nothing you say can anchor the reader. Add no summary and no opinion. For an issue, the document is the issue body.

### Comprehension

```text
<the full text of the document>

---
Above is one document. You are new to this project. You may open the files that the document links to, and no others.

Answer from the document alone, in exactly this format, at most 400 words:

Explanation:
<what it describes and why, in three sentences or fewer>
Example:
<one concrete case, step by step>
Parts:
- <part>: <its job>
Guesses:
- <what you had to guess>: "<the sentence that made you guess>"
Read twice:
- "<a sentence that you had to read twice>"

Write "none" under a heading with nothing to report.
```

Compare the answer with what you know. Each wrong statement or guess points to a sentence in the document that is missing, unclear or wrong.

### Critic

```text
<the full text of the document>

---
Above is one document. Review it. You may open the files that it links to, and no others.

Find what is missing or wrong: a claim that a linked file contradicts, a step that does not follow, a case that the document does not handle, a word with two meanings, a section that says nothing. Do not report style.

Answer in exactly this format, most important first, at most 400 words:

- MISSING | WRONG | UNCLEAR: <the finding>
  Where: <the section, or a short quote>
  Why it matters: <one line>

If nothing important is wrong, write: nothing important.
```

### Readiness

```text
<the full text of the document>

---
Above is one document. You must build what it describes, and you cannot talk to its author. You may open the files that it links to, and no others.

List only the decisions that you would still have to make before you start: places where you would have to guess, and a wrong guess would change the result. Skip what the document leaves to the builder on purpose, and what you can decide yourself.

Answer in exactly this format, most important first, at most 400 words:

- Decision: <the question>
  Why it matters: <what changes with the answer>

If nothing important is open, write: nothing important.
```

## Sort the findings

Sort each finding of every test into one of three piles:

- **Already answered:** the conversation, the issue or the code answers it. Put the answer into the document.
- **Minor:** a careful builder can decide it, and the decision does not change the result. Decide it, write it into the document, and list it as your call for the user.
- **For the user:** only the user can decide it, and the decision changes the result.

Only the third pile goes to the user, as questions asked together, not one at a time. Give the user the list of your calls too, so they can check them while they read.
