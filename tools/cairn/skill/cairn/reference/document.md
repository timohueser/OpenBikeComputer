# document <system>

Writes or reworks `docs/design/<system>.md` bottom-up: peanuts, then hay. A subagent that misreads the code shows unclear code: a finding, not a failure.

**This command writes documentation only.** It writes the system doc, its header (`purpose:`, and `status:` at the end), the map and the one-sentence purpose comments. It never edits code, other comments, guides or specs. It does not critique: what it notices goes into the findings issue.

A leaf is a unit for reading, not a unit of purpose: it can hold several modules. Each module (file) gets its own purpose sentence.

## Findings

Keep one list of findings while you work. A finding never interrupts the user. Its kinds: drift, `UNRELIABLE` summaries, `CANNOT` and `SPLIT` purposes, confusion notes, the system's violations, leaves over the limit, rules with no check, and numbers in a gate with no stated origin. Each has its kind, a `file:line`, one line of description and a suggested action: `fix`, `refactor issue`, or `intended choice` (the Decisions section explains it).

A question that the doc needs is content, not a finding: for example a rejected alternative that the code does not show, or the origin of a number in the doc. Mark it in the draft where it belongs, as `(inferred: <your best guess>)` or `(open: <the question>)`.

## 1. Measure

Run `cairn check` and `cairn leaves <system>`. If `cairn leaves` says there is no doc, stop and tell the user. The leaves leave out tests and vendored code; the draft links tests as evidence for rules.

The system's violations are its entries in `.cairn/baseline.json` (the IDs that name it) and the `cycle between` lines of `cairn check` that name it.

If the system has more than 10 leaves, give the user the count and ask whether to start. Ask nothing else before step 9.

If the system has no leaves, skip steps 2 to 4. Write the doc from the files that the system owns, such as a registry, and the top-down view of step 5. Say why in the start message.

On a rework, a leaf needs no peanut when each of its modules has a purpose comment and no file changed since the last commit of the system doc, committed or not. Its purpose comments are its summary; its rules are already in the doc.

## 2. Peanuts

Start one `doc-peanut` subagent for each leaf, all in parallel. Give it only this prompt with the files of its leaf. Do not give it the `uses` lines or the system doc: step 3 needs a claim that does not copy them. Keep the `Purpose:` line only when the leaf is one module: one file, or one folder with its module file.

```text
Read only these files, and no others:
<the files of the leaf>

Write a summary in exactly this format. `Parts` has one short line for each file. The rest has at most 400 words.

Parts:
- <file>: <its purpose, one sentence without "and">
  or <file>: CANNOT: <why one sentence is not possible>
Purpose: <the purpose of the whole module, one sentence without "and">
Uses:
- <crate, package or module outside these files>: <what for>
Rules:
- <a rule that the code enforces or assumes>: <file:line>
Confusion:
- <misleading name | mixed role | dead code | duplicate | false comment | comment with a date or an issue number>: <file:line>, <one line>

Write "none" under a heading with nothing to report.
```

## 3. Mechanical check

Compare each peanut's `Uses` with its leaf's `uses` and `tests use` lines. Treat `_` and `-` as the same in a crate name. A name in the `repository crates` line is a wrong claim when the leaf's two lines do not list it and it is not the leaf's own crate. Other names are outside the check. A leaf with no crate passes.

Mark a summary with a wrong claim `UNRELIABLE`, with the claim. It goes up to the hay.

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
- <a duplicate or a mixed role across the parts>: <file:line>, <one line>
```

The top summary is the bottom-up view: what the code does.

## 5. Two views

Write the top-down view yourself: what we intended. Read the specs in `specs/` that the system implements, the guide pages in `docs/content/` about it, and the pull requests and issues that touched its files. If `git rev-parse --is-shallow-repository` prints `true`, run `git fetch --unshallow` first. `git log --first-parent --merges --format='%s | %b' -- <the owns patterns>` lists the merged pull requests with their titles. Read only the ones whose titles concern the system.

Compare the two views: each difference is drift between intent and code.

## 6. Draft

Write the body below the header, in these sections:

1. What it does
2. A picture
3. A walk-through of one concrete example
4. Its parts: the top level of the hay
5. Rules that must hold, each linked to the check or test that enforces it
6. Decisions, each with its reason and the alternative we rejected
7. Words it introduces

Aim for about 1,200 words; write no filler. Link a spec; do not copy its tables. Use the words of the glossary.

A figure is one of three kinds: a generated figure, Mermaid, or a hand-drawn SVG with one line that says why the other two are not enough.

If the hay found a better purpose sentence, correct `purpose:` in the header.

## 7. Purpose comments

Write each module's purpose sentence from `Parts` into its file: `//!` in Rust, the module docstring in Python, otherwise a one-line comment at the top. Do this only where the comment is missing or wrong, and only from a summary that passed step 3.

## 8. Start message

At most 5 lines, with no list. Name the system, and give the number of sections, the word count and the number of findings for the issue. Then show the first section.

## 9. Section by section

Send one message for each section, in order:

1. Show the section's text in the chat, not only a link. Show `purpose:` with section 1.
2. Ask the `(inferred: …)` and `(open: …)` questions of the section. Each stands alone: what it is, why it matters, your best guess. Here questions go by section, not all together.
3. End with this line: "Approve and continue, or tell me what to change, including any sentence you had to read twice."
4. Wait. Apply the changes and remove the answered markers. Show the section again only when the user asks.

## 10. Goldfish and approval

Run the comprehension test of [goldfish.md](goldfish.md) on the doc. Fix gaps in facts silently. Ask only what needs the user, as standalone questions.

When the user approves the doc, set `status: approved`, then run `cairn map` and `cairn check`. If it fails, show the failures, and keep `draft` until the user decides.

## 11. File and finish

Write the findings to a file, in Simplified Technical English, grouped by kind, each with its `file:line`, description and action. Open the issue with `gh api repos/{owner}/{repo}/issues -X POST -f title="Findings from documenting <system>" -F body=@FILE`.

Then open one pull request with the doc, the map and the purpose comments. Its body links the findings issue. Its `Mode:` line is `change #N` when an issue asked for the doc, otherwise `fix`.

End with two lines: the pull request link and the issue link.
