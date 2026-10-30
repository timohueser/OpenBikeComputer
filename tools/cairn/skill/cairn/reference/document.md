# document <system>

Writes or reworks `docs/design/<system>.md` with the user, bottom-up: peanuts, then hay. Each subagent reads a small part of the code, so each step stays accurate and cheap. When a subagent misreads the code, the code is unclear. That is a finding, not a failure.

A leaf is a unit for reading, not a unit of purpose: it can hold several modules. Each module (file) gets its own purpose sentence.

The system doc must have its header. If `cairn leaves <system>` says there is no doc, stop and tell the user.

## 1. Measure

Run `cairn check` and `cairn leaves <system>`. Keep both outputs. The leaves leave out tests and vendored code; the hay can name tests as evidence.

The system's violations are its entries in `.cairn/baseline.json` (the IDs that name it) and the `cycle between` lines of `cairn check` that name it. They go on the weakness list. So does each leaf over the leaf limit.

If the system has no leaves, skip steps 2 to 4. Write the doc from the top-down view of step 5, and tell the user why.

On a rework, a leaf needs no peanut when each of its modules has a purpose comment and no file changed since the last commit of the system doc. Its purpose comments are its summary.

## 2. Peanuts

Start one `doc-peanut` subagent for each leaf, all in parallel. Give it only this prompt with the files of its leaf. Do not give it the `uses` lines or the system doc: step 3 needs a claim that does not copy them. Keep the `Purpose:` line only when the leaf is one module: one file, or one folder with its module file.

```text
Read only these files, and no others:
<the files of the leaf>

Write a summary in exactly this format, at most 400 words.

Parts:
- <file>: <its purpose, one sentence without "and">
  or <file>: CANNOT: <why one sentence is not possible>
Purpose: <the purpose of the whole module, one sentence without "and">
Uses:
- <crate, package or module outside these files>: <what for>
Rules:
- <a rule that the code enforces or assumes>: <file:line>
Confusion:
- <misleading name | mixed role | dead code | duplicate | comment that the code contradicts>: <file:line>, <one line>

Write "none" under a heading with nothing to report.
```

## 3. Mechanical check

Compare each peanut's `Uses` with its leaf's `uses` and `tests use` lines. Treat `_` and `-` as the same in a crate name. A name in the `repository crates` line is a wrong claim when the leaf's two lines do not list it and it is not the leaf's own crate. Other names are outside the check. A leaf with no crate passes.

Mark a summary with a wrong claim `UNRELIABLE`, with the claim. It goes up to the hay and onto the weakness list.

## 4. Hay

Each distinct `level:` folder is a level. Its children are its leaves and the levels inside it. Above them come the crate, then the system. Skip a level with one child. Go bottom-up: start one `doc-hay` subagent for each level after its children are done. Give it each child summary, with the child's `leaf` line or level name before it, the `UNRELIABLE` marks, and the module file of its folder if it has one: `lib.rs`, `main.rs`, `mod.rs`, `x.rs` for a folder `x/`, `__init__.py` or `index.ts`.

```text
These are summaries of the parts of <level>. Read them and the files given, and no others.

<the child summaries>

Write a summary in exactly this format, at most 400 words.

Level: <level>
Purpose: <one sentence without "and">
  or SPLIT: <the purposes that this level mixes>
Parts:
- <child>: <its purpose>
Uses: <the systems or crates outside this level>
Rules:
- <a rule that holds for the whole level>: <where>
Confusion:
- <kind>: <count>; the worst: <file:line>, <one line>
```

The confusion notes go up with their counts. The top summary is the bottom-up view: what the code does.

## 5. Two views

Write the top-down view yourself: what we intended. Read the specs in `specs/` that the system implements, the guide pages in `docs/content/` about it, and the pull requests and issues that touched its files. If `git rev-parse --is-shallow-repository` prints `true`, run `git fetch --unshallow` first. `git log --first-parent --merges --format='%s | %b' -- <the owns patterns>` lists the merged pull requests with their titles. Read only the ones whose titles concern the system.

Compare the two views. Each difference is drift between intent and code, and goes on the weakness list.

## 6. Draft

Write the body below the header, with these sections in this order:

1. What it does
2. A picture
3. A walk-through of one concrete example
4. Its parts: the top level of the hay
5. Rules that must hold, each linked to the check that enforces it
6. Decisions, each with its reason and the alternative we rejected
7. Words it introduces

About 1,200 words is a guide, not a limit. A doc can be longer when its system needs it, and the content shows why. Write no filler. Link a spec; do not copy its tables. Use the words of the glossary.

A figure is one of three kinds: a generated figure, Mermaid, or a hand-drawn SVG with one line that says why the other two are not enough.

If the hay found a better purpose sentence, correct `purpose:` in the header. After each header edit, run `cairn map`, then `cairn check`.

Rank the weakness list, most important first: drift, `UNRELIABLE` summaries, `CANNOT` and `SPLIT` purposes, confusion notes by count, the system's violations, and rules with no check.

## 7. Session with the user

Give the user the draft and the ranked weakness list. For each item, propose one of these, with your reason:

- **Fix now:** a rename or a comment, in this pull request.
- **Refactor issue:** a larger change. Open the issue when the user agrees.
- **Intended choice:** the Decisions section explains it.

Ask the user to mark each sentence that they had to read twice. Rewrite those sentences.

## 8. Goldfish

Run the comprehension test of [goldfish.md](goldfish.md) on the doc.

## 9. Purpose comments

Write each module's purpose sentence from `Parts` into its file: `//!` in Rust, the module docstring in Python, otherwise a one-line comment at the top of the file. Do this only where the comment is missing or wrong, and only from a summary that passed step 3. The sentence states the module's intent.

## 10. Approval and pull request

When the user approves the doc, set `status: approved`, then run `cairn map` and `cairn check`. If the check fails, the quality gates now see the system's violations: show the failures to the user, and keep `draft` until the user decides.

One pull request holds the doc, the purpose comments and the fix-now items. Its body has the ranked weakness list, with what became of each item. Its `Mode:` line is `change #N` when an issue asked for the doc, otherwise `fix`.
