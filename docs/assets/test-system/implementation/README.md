# Test system redesign — implementation documents

The design record behind epic
[#1816](https://github.com/timohueser/OpenBikeComputer/issues/1816). The epic fixes scope, order and
acceptance; these documents hold the design and the evidence it rests on.

| File | What it is |
| --- | --- |
| `epic.md` | The epic body, mirrored from #1816. The authoritative scope. |
| `plan.md` | The settled design (plan v3), corrected on 2026-09-16 against current `develop`. Children are derived from this. |
| `plan-v2-superseded.md` | The first full proposal. Kept because the adversarial reviews cite it. Do not implement from it. |
| `surveys/` | Nine parallel subsystem surveys of the test system as it stood at `d42e2e3a`, plus a completeness critic. Every claim carries a `file:line`. |
| `reviews/` | Six adversarial reviews of the first proposal, and one independent review by a second agent that produced plan v3. |

## How to read these

`plan.md` is the design. It was written against `d42e2e3a` and landed three days later, by which
time `develop` was 764 commits ahead. Its corrections are applied in place and marked
`[CORRECTED …]`, `[REMOVED …]` or `[ALREADY DONE …]`; the epic carries the same list with reasons.

The surveys and reviews are a snapshot, not a live inventory. They describe the repository at
`d42e2e3a`. Read them for mechanism and rationale, never as a current census. Anything counted
there must be re-counted before it is quoted again: `obc-app` lost 27% of its test targets, and the
golden manifest shrank from 317 frames to 263.

## The settled harness decision

`plan.md` decided against sharing the host-side application assembly. All three of its stated
reasons have since stopped holding, and the owner settled it on 2026-09-16: share only the frame
seam that is provably identical in every host, and leave everything host-specific alone. The exact
surface, and the explicit list of what must not be extracted, are in `epic.md` and in #1816. This
overrides settled decision 1 and the corresponding row in `plan.md`.
