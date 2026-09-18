# OpenBikeComputer system requirements

Working draft updated on 18 September 2026 after owner review of product gaps. This folder is a review and editing handoff, not the published requirements database or evidence that the product passes these requirements.

| Document | Purpose |
| --- | --- |
| [Requirements](requirements-draft.md) | 383 requirements in 27 groups, written for review and manual entry. REQ-336–358 retain their 16 September review status. REQ-359–384 and revised REQ-239/262 are accepted additions from 18 September. |
| [Acceptance targets](acceptance-targets.md) | Capacity, accuracy, hardware, power, timing, platform, and integration criteria. Unset values are explicitly TODO. |
| [Decisions and source notes](review-notes.md) | Accepted intent, deferred details, code findings, and the reasons behind the requirements. |

## Editing

Keep requirement IDs stable when editing or moving entries. REQ-321 was retired when tunnel warnings were removed; do not reuse it. The last allocated ID is REQ-384. Update referenced acceptance criteria when a requirement changes. A TODO is unfinished acceptance work, not a waived requirement or a passing result.

GitHub renders these Markdown files with tables and a document outline. Use its file editor on branch `codex/system-requirements-draft`, or check out the branch on another computer:

```sh
git fetch origin
git switch --track origin/codex/system-requirements-draft
```

The branch contains only this documentation handoff. It is separate from `develop` and is not merged into the product branch.

## Scope decisions to preserve

- Personal moving-time estimates require a learned rider model, 300 km of prior usable history, and mean absolute percentage error below 10% on independent rides under the defined validation conditions.
- Unexpected power loss at 1 Hz may lose at most 30 seconds of accepted recording data on supported healthy storage. Successful orderly saving must lose none.
- Navigation recovery is an explicit choice, separate from recording recovery. Implementation is tracked in [issue #1860](https://github.com/timohueser/OpenBikeComputer/issues/1860).
- Route to start means the first point of the selected route. Trips do not advance automatically.
- Sharp-turn warnings use active-route geometry, with sound and a visual indication. Tunnel and other road-hazard warnings are excluded.
- Greek/Cyrillic and Romanian support must keep existing map record widths and text byte limits. Use a readable bounded fallback where needed.
- Route coverage notices appear only on the device, on receipt and route start. They are informational and must not block route use or add transfer-protocol requirements.
- Routing profiles must define suitability policies. Minimum functions after failures are specified in the acceptance tables.
- All device functions, including future additions, remain free of mandatory accounts and subscriptions. Open-source rehosting, local map production, and independent USB tools are required.

- While-moving interaction commitments cover only essential riding controls. Assistant use while moving is not promised.
- Pause/continue take one short press; direct destinations take one gesture; fixed feature/setting access takes at most four actions. Return to the live map takes at most two, with Map selected whenever the main menu opens.
- Active, unpaused recording limits full-screen interruptions to critical faults and deliberately uploaded route prompts. Ordinary riding information uses partial banners or indicators.

Remaining TODO values are intentional. They mainly need hardware trials, performance measurements, validation data, or detailed interaction and release-support definitions.
