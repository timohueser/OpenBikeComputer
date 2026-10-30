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
Above is one document. You are new to this project: you know software, but not this project. You may open the files that the document links to, and no others. Change no file.

First, paraphrase the document from it alone. Then find what stops a new reader:
- a term of this project that the document does not define or link; a general engineering term is not a gap;
- a step or a prerequisite that the document assumes but does not state;
- a vague explanation, such as "the system handles this", that does not say how;
- a section that you cannot paraphrase: this is a Blocker.

Answer in exactly this format, at most 400 words:

What it describes:
<what it does and why, in two to five sentences>
How it works, per this document:
<one concrete case, step by step>
Gaps:
- <Blocker | Major | Nit>: <the gap>: "<the term or sentence>"
Read twice:
- "<a sentence that you had to read twice>"

Write "none" under a heading with nothing to report.
```

Compare the paraphrase with what you know. A wrong paraphrase is a finding: find the sentence that caused it.

### Critic

```text
<the full text of the document>

---
Above is one document. You are a skeptical senior engineer. You accept no claim without proof, and you look past the text for hidden risks. You may open the files that the document links to, and no others. Change no file.

Find what is missing or wrong:
- a claim that a linked file contradicts, or that nothing supports: about another system, the data, or performance;
- a failure mode, a limit or a risk that the design adds but does not handle;
- a step that does not follow, or a case that the document does not handle;
- a word with two meanings, or a section that says nothing.
Do not report style.

Answer in exactly this format, most important first, at most 400 words:

- <Blocker | Major | Nit> <MISSING | WRONG | UNCLEAR>: <the finding>
  Where: <the section, or a short quote>
  Change: <what the document must say or do instead>

If nothing important is wrong, write: nothing important.
```

### Readiness

```text
<the full text of the document>

---
Above is one document. You are the engineer who must build what it describes, and you cannot talk to its author. Ask yourself: do I know exactly what to change, where, and how? You may open the files that the document links to, and no others. Change no file.

List only the decisions that you would still have to make before you start:
- a design choice that the document leaves to you, such as a data format, an interface, or how to handle an error;
- a vague step, such as "update the reader", where you need "add field X to type Y in file Z".
Skip what the document leaves to the builder on purpose, and what you can decide yourself without a change to the result.

Answer in exactly this format, most important first, at most 400 words:

- <Blocker | Major | Nit> Decision: <the question>
  Where: <the section, or a short quote>
  Why it matters: <what changes with the answer>

If nothing important is open, write: nothing important.
```

## Sort the findings

Sort each finding of every test into one of three piles:

- **Already answered:** the conversation, the issue or the code answers it. Put the answer into the document.
- **Minor:** a careful builder can decide it, and the decision does not change the result. Decide it, write it into the document, and list it as your call for the user.
- **For the user:** only the user can decide it, and the decision changes the result.

Only the third pile goes to the user, as questions asked together, not one at a time. Give the user the list of your calls too, so they can check them while they read.
