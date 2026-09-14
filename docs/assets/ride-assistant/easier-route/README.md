# Easier route wireframe proposal

Status: three concepts for rider review. No simulator or routing implementation is added.
The wireframes use the 240 × 320 display, Terminus label font, brown header, and amber selection.
[wireframes.html](wireframes.html) is the self-contained interactive fragment used in the review.
It has no network dependencies. The host can optionally provide the time-row design control.

## Shared example

All values are fictional and describe the remaining journey to the same destination.
“Rough” is a placeholder for the future surface rule, not a defined routing classification.
The example assumes known surface data. It must not imply that unknown surface is smooth.

| Route | Distance | Ascent | Rough surface | Optional approximate ride time |
| --- | ---: | ---: | ---: | ---: |
| Current | 42 km | 860 m | 6 km | 2 h 55 min |
| Less climbing | 45 km | 440 m | 6 km | 3 h 10 min |
| Smoother surface | 47 km | 980 m | 2 km | 3 h 15 min |
| Shorter ride | 36 km | 1,110 m | 6 km | 2 h 30 min |

The optional time row explores a future estimate. It is off by default. The distance goal is
called **Shorter ride**. Do not call it fastest before the time model can support that claim.

## A: Goal first — recommended for discussion

The header says **Easier route**. The current remaining distance and ascent sit below it.
Three two-line rows show each goal and its main change:

- **Less climbing:** 420 m less ascent, 3 km more distance.
- **Smoother surface:** 4 km less rough surface, 5 km more distance.
- **Shorter ride:** 6 km less distance, 250 m more ascent.

Up and Down change the selected row. Select opens **Preview route**. This layout starts with
the rider's question and makes the three available benefits easy to compare. The compact
summary does not list every tradeoff: the smoother route's additional ascent is in the review.

## B: Compare numbers

Show one alternative at a time with current/new columns for distance, ascent, and rough
surface. Up and Down browse alternatives. Select opens the review. An optional time row can
show approximate durations when the time model is available.

This exposes more of the cost before preview. It needs more reading and makes comparison
between alternatives depend on memory. It may work better as the second screen of A.

## C: Map first

Show the current and alternate route above a selected card with its main benefit and cost.
Up and Down change the candidate. Select opens the review. The wireframe lines are a schematic,
not real geography or a routing result.

This helps explain where the route changes. It uses much of the small display before the rider
has chosen a goal. A geographical comparison could instead be a page inside A's preview.

## Shared review

The wireframes use a cost review with **Same destination**, the selected goal, and current/new
columns. **Use this route** is the explicit acceptance action. Back returns to the choice and
preserves selection. Exploring alternatives does not change the accepted route.

A map/profile preview, route computation, no-route state, unavailable alternatives, unknown
surface/elevation, and preservation of required stops need definition in the later spec. Do
not silently skip required stops or turn missing data into a claimed improvement. Keep “less
climbing” separate from guarantees about maximum slope or technical difficulty.

## Next design step

Choose and refine the layout, then build this fourth simulator shell. After that, draft the
implementation epic and sub-issues for Find a place, What's next, Landmarks, and Easier route,
including map creation and routing work. The requested adversarial review belongs to that
future epic stage. These wireframes do not settle the routing algorithm or time model.

## Verification

- Browser: inspected all three layouts; exercised selection, review, Back with selection
  preserved, and explicit acceptance. At a 320 px viewport, the layouts stack without
  horizontal overflow.
- Node VM check: exercised every candidate, review, and acceptance with the optional time row
  both off and on. The draw bounds stayed within 240 × 320.
- `python3 docs/assets/ride-assistant/landmark-selection/count.py`: output matches counts.json.
- `python3 docs/build_docs.py --check-links`: passed.
- `./tools/obc suites check`: passed.
- `git diff --check`: passed.

Rust tests, Clippy, firmware builds, resource checks, the simulator snapshot sweep, and board
flashing were omitted: this change contains design assets, selection data, and documentation.
