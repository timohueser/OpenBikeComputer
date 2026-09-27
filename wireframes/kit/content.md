# Content sheet for the route planner wireframes

Every mock uses this content. Do not invent other places or figures. All figures are mock values;
the page says so once at the top. Never use lorem ipsum.

## Copy rules (owner rules, non-negotiable)

- Plain, formal, simple copy. No taglines, no aphorisms, no wordplay, no helper paragraphs, no
  explainer lines under obvious controls. A control names its action ("Add as stop", "Use this
  route", "End Day 4 here").
- Riding time only, never a clock time. Never "arrive at 16:40", "before dark", "open when you
  arrive". Opening hours are shown as the place's hours ("Sun 8:30–12:30"), never as a prediction.
- Honest figures: say known or estimated. "23 km unknown surface", "Hours unknown", "No mapped water
  for 29 km" (never "no water").
- Ask, do not push: no pop-ups, no modal warnings. Warnings are quiet marks on the profile or one
  quiet line in a summary.
- No "scenic". No stars, ratings, reviews or popularity.
- The box never acts on a guess: it shows how it understood the sentence, and a route change needs
  an explicit tap.
- Numbers: thin grouping "1,020 m", units with a space ("20.8 km", "1 h 55"). Mono digits only in
  data columns and profiles.

## Vocabulary

- Bike types: Road, Gravel, MTB, Touring.
- Goals (preset = bike type + goal): Balanced, Shortest, Least climbing, Least unpaved, Most
  climbing. (The list is open.)
- Alternatives are named by what they win: "Shortest", "Least climbing", "Least unpaved". An
  option that wins nothing is not shown. Each option shows its difference from the selected one:
  "+13 km · −300 m climb".
- Point types (names are a proposal): **Pass here** (a via point, ring icon), **Shape** (bends the
  line, small dot, no stop), **Visit** (go there and come back to the line; flag icon), **Sleep**
  (a day end; tent icon). A day end is a point on the line.
- Segment modes, per segment: **Routed**, **Straight**, **Drawn**.
- Surface: Paved, Unpaved, Unknown. Unknown is drawn as an outlined (hollow) line or band.
- POI kinds (from the map cells): Water, Campsite, Lodging, Resupply (shops), Pharmacy, Bike shop,
  Train. The box accepts words such as "shops", "supermarket", "campsites", "hotel", "Zeltplatz".
- Toolbar actions: Undo, Redo, Versions, Import, Offline areas, Continue on phone (animated QR),
  Send to device, Export GPX.

## The Black Forest (quick jobs, phone)

Map "bf": `viewBox="0 0 1000 700"`, north up. x = (lon − 7.75) / 0.5 × 1000,
y = (48.15 − lat) / 0.3 × 700. About 36 km wide, 33 km tall. The Rhine plain is in the west (x <
230, flat), the Black Forest hills are east of Freiburg.

| Place | Kind | Height | x | y |
|---|---|---|---|---|
| Freiburg im Breisgau | city | 278 m | 200 | 362 |
| Denzlingen | town | 235 m | 256 | 191 |
| Waldkirch | town | 263 m | 424 | 131 |
| Kandel | mountain summit | 1,241 m | 536 | 203 |
| Glottertal | village | 310 m | 380 | 233 |
| St. Peter | village | 722 m | 566 | 310 |
| St. Märgen | village | 889 m | 686 | 369 |
| Kirchzarten | town | 392 m | 410 | 434 |
| Hinterzarten | village | 893 m | 712 | 576 |
| Titisee (lake and village) | village | 846 m | 820 | 576 |
| Schauinsland | mountain | 1,284 m | 296 | 558 |
| Feldberg | mountain | 1,493 m | 508 | 644 |
| Simonswald | village | 330 m | 620 | 117 |
| Hotel Krone, St. Peter | lodging | 720 m | 590 | 318 |

Rivers: Elz (from x 700 y 40 through Simonswald and Waldkirch to Denzlingen, then north-west off the
map), Glotter (Glottertal to Denzlingen), Dreisam (Kirchzarten through Freiburg to the north-west).
Lake Titisee: an ellipse about 70 × 26 around x 835 y 582, long axis south-west to north-east.
Main road B 31 (a trunk road, the planner avoids it): Freiburg → Kirchzarten → Höllental →
Hinterzarten → Titisee.

### Job 1 route: "Titisee" from here (here = Glottertal), Touring bike

| Option | Via | Distance | Climb | Riding time | Unpaved |
|---|---|---|---|---|---|
| Shortest (selected) | St. Peter, St. Märgen | 34 km | 1,020 m | 2 h 50 | 3.1 km, of which 1.4 km unknown |
| Least climbing | Freiburg, Kirchzarten, Höllental bike path, Hinterzarten | +13 km (47 km) | −300 m (720 m) | +0 h 20 (3 h 10) | 0.8 km |
| Least unpaved | St. Peter, St. Märgen, on roads | +2 km (36 km) | +40 m (1,060 m) | +0 h 10 (3 h 00) | 0 km |

### Query route: "road bike route up Kandel starting here" (here = Denzlingen), Road bike

One route: Denzlingen → Waldkirch → L 186 → Kandel summit. 20.8 km, 1,020 m, 1 h 55 riding, max
gradient 12 %, paved. No other way wins on distance, climbing or surface. The card says: "No other
way is shorter, flatter or more paved."

Kandel is ambiguous worldwide: "Kandel, mountain, 1,241 m, Black Forest" wins near Freiburg.
Other result: "Kandel, town, Rhineland-Palatinate, 138 km".

### Job 4: imported route, via my hotel

"Schwarzwald Gravel, 3 days" imported from a GPX file: 312 km, 6,480 m, Gravel. The line is kept
exactly as imported ("Imported line · not re-routed"). On the bf map it runs Waldkirch → Kandel →
St. Peter → St. Märgen → Hinterzarten → Titisee (the rest is off the map). Hotel Krone, St. Peter
lies 1.1 km off the line. As a Visit: +2.3 km, +60 m, only the new detour is routed, "312 km of
the line unchanged". The planner's data on the imported line: "41 km unknown surface",
"No mapped water for 38 km (Day 2)".

## The Alps (deep job on the website, tent job on the phone)

Trip "Alps: Genève to Nice", Touring bike, 10 days (9 riding days, 1 rest day). Dates are set in
most mocks: starts Thu 17 Jun 2027. One mock may show the trip without dates.

| Day | Date | From → To | km | Climb | Riding time | Passes |
|---|---|---|---|---|---|---|
| 1 | Thu 17 Jun | Genève → Le Grand-Bornand | 78 | 1,640 m | 5 h 40 | Col de la Colombière 1,613 m |
| 2 | Fri 18 Jun | Le Grand-Bornand → Beaufort | 64 | 1,720 m | 5 h 20 | Col des Aravis 1,486 m, Col des Saisies 1,650 m |
| 3 | Sat 19 Jun | Beaufort → Val d'Isère | 71 | 2,290 m | 6 h 30 | Cormet de Roselend 1,968 m |
| 4 | Sun 20 Jun | Val d'Isère → Valloire | 104 | 2,310 m | 7 h 10 | Col de l'Iseran 2,764 m, Col du Télégraphe 1,566 m |
| 5 | Mon 21 Jun | Valloire → Briançon | 53 | 1,480 m | 4 h 30 | Col du Galibier 2,642 m |
| 6 | Tue 22 Jun | Rest day, Briançon | – | – | – | – |
| 7 | Wed 23 Jun | Briançon → Guillestre | 56 | 1,420 m | 4 h 40 | Col d'Izoard 2,360 m |
| 8 | Thu 24 Jun | Guillestre → Barcelonnette | 51 | 1,470 m | 4 h 30 | Col de Vars 2,109 m |
| 9 | Fri 25 Jun | Barcelonnette → Saint-Étienne-de-Tinée | 63 | 1,640 m | 5 h 30 | Cime de la Bonette 2,802 m |
| 10 | Sat 26 Jun | Saint-Étienne-de-Tinée → Nice | 94 | 620 m | 4 h 50 | – |
| Trip | | | 634 km | 14,590 m | 49 h 00 riding | |

Trip facts for summaries: "634 km · 14,590 m · 49 h riding", "12 km unknown surface", "3 gaps"
(on request only), "1 seasonal closure on the route".

Day 4 is long (104 km, 7 h 10). This drives the tent job.

Trip-wide facts that the planner shows quietly:
- Seasonal closure: Col de l'Iseran, OSM `access:conditional`, closed about Nov–mid-Jun (mock
  wording: "Seasonal closure in OSM: Nov–15 Jun"). Day 4 is 20 Jun, so no conflict, but the pass
  is close to its opening date.
- Snow history (experiment), Col de l'Iseran top 6 km: "Free of snow on 20 Jun in 6 of 10 years.
  Early year: 28 May. Late year: 2 Jul." Cime de la Bonette top 4 km: "8 of 10 years".
- Gap on request: "No mapped water for 29 km" on Day 4, km 0–29 (Iseran). "Last shop for 63 km"
  on Day 9 (Barcelonnette → Saint-Étienne-de-Tinée).
- Unknown surface: 12 km in total, mostly on Day 2 (8 km, Col des Saisies side road) and Day 7.

### Alps overview map "alps": `viewBox="0 0 700 900"`, north up

x = (lon − 5.6) / 2.0 × 700, y = (46.3 − lat) / 2.7 × 900.

| Place | x | y | Place | x | y |
|---|---|---|---|---|---|
| Genève | 190 | 32 | Col du Galibier | 283 | 412 |
| Le Grand-Bornand | 290 | 119 | Briançon | 363 | 467 |
| Beaufort | 341 | 194 | Col d'Izoard | 397 | 493 |
| Val d'Isère | 483 | 284 | Guillestre | 367 | 547 |
| Col de l'Iseran | 501 | 294 | Col de Vars | 386 | 587 |
| Modane | 375 | 367 | Barcelonnette | 368 | 638 |
| Saint-Michel-de-Maurienne | 305 | 360 | Cime de la Bonette | 422 | 660 |
| Valloire | 290 | 378 | Saint-Étienne-de-Tinée | 464 | 681 |
| Lac d'Annecy | 200 | 150 | Nice | 583 | 866 |

Lake Geneva runs from Genève north-east off the top edge. The Mediterranean is below y ≈ 870 near
Nice. Italy lies east of about x 520 in the north and x 600 in the south.

### Day 4 map "d4": `viewBox="0 0 1000 600"`, north up

x = (lon − 6.35) / 0.75 × 1000, y = (45.50 − lat) / 0.40 × 600. About 59 km wide.

| Place | km on Day 4 | Height | x | y |
|---|---|---|---|---|
| Val d'Isère | 0 | 1,850 m | 840 | 78 |
| Col de l'Iseran | 15 | 2,764 m | 908 | 125 |
| Bonneval-sur-Arc | 30 | 1,780 m | 929 | 192 |
| Bessans | 38 | 1,710 m | 853 | 270 |
| Lanslevillard | 45 | 1,450 m | 747 | 310 |
| Lanslebourg-Mont-Cenis | 48 | 1,400 m | 704 | 321 |
| Termignon | 54 | 1,300 m | 620 | 334 |
| Bramans | 62 | 1,230 m | 567 | 412 |
| Modane | 71 | 1,060 m | 427 | 450 |
| Saint-Michel-de-Maurienne | 88 | 710 m | 160 | 420 |
| Col du Télégraphe | 99 | 1,566 m | 125 | 445 |
| Valloire | 104 | 1,430 m | 105 | 501 |

The river Arc runs from Bonneval down the valley through Lanslebourg, Modane and
Saint-Michel-de-Maurienne, then north-west off the map.

### Places on Day 4 (the tent job and the box results)

"End of Day 4" = Valloire, km 104. "Middle of Day 4" = around km 52 (Termignon).

| Name | Kind | km on Day 4 | Off the line | Extra climb | Hours |
|---|---|---|---|---|---|
| Camping Caravaneige, Valloire | Campsite | 104 | 0.6 km | +30 m | Reception 8:00–20:00 |
| Camping Les Verneys, Valloire | Campsite | 103 | 1.4 km | +70 m | Hours unknown |
| Hôtel Christiania, Valloire | Lodging | 104 | 0.2 km | 0 m | – |
| Camping Le Petit Nice, Saint-Michel-de-Maurienne | Campsite | 87 | 0.9 km | +10 m | Hours unknown |
| Camping de l'Arc, Modane | Campsite | 71 | 0.4 km | 0 m | Reception 9:00–19:00 |
| Camping Les Mélèzes, Lanslevillard | Campsite | 45 | 0.3 km | +10 m | Hours unknown |
| Sherpa, Valloire | Resupply | 104 | 0.3 km | 0 m | Sun 8:00–12:30, 16:00–19:30 |
| Carrefour Market, Modane | Resupply | 71 | 0.5 km | 0 m | Sun 8:30–12:30 |
| Intermarché, Saint-Michel-de-Maurienne | Resupply | 88 | 0.7 km | 0 m | Sun closed |
| Épicerie de Bonneval | Resupply | 30 | 0.1 km | 0 m | Hours unknown |
| Vival, Termignon | Resupply | 54 | 0.2 km | 0 m | Sun 8:00–12:00 |
| Fountain, Bonneval-sur-Arc | Water | 30 | 0.0 km | 0 m | – |
| Fountain, Lanslebourg | Water | 48 | 0.1 km | 0 m | – |

Day window (proposal): "Day 4 at 5–6 h riding ends between km 80 and km 92" (between Modane and
Saint-Michel-de-Maurienne). At the planned 7 h 10 it ends in Valloire.

A route change the box can propose: "end day 4 at saint-michel" → Day 4: 104 km → 88 km, 7 h 10 →
5 h 20. Day 5: 53 km → 69 km, 4 h 30 → 6 h 20, and Day 5 now starts with the Col du Télégraphe.
The line does not change.

## The query box

Character limit 80. Placeholder: "Search places or the route". Examples to show:

| Typed | Result type | Understood as (chips) |
|---|---|---|
| `campsites end of day 4` | List of places | Campsites · End of Day 4 (Valloire) · within 5 km |
| `shops middle of day 4` | List of places | Shops · Middle of Day 4 (km 40–64) |
| `pharmacies along route` | List of places | Pharmacies · Along the route · within 2 km |
| `road bike route up Kandel starting here` | Route | Route · From here (Denzlingen) · To Kandel summit · Road |
| `shops open Sunday day 4` | List of places (filtered) | Shops · Day 4 · Open on Sunday |
| `shops open day 4` | List of places, needs a date | Shops · Day 4 · Open on Day 4 (needs a date) |
| `Kandel` | One place | Kandel · mountain · 1,241 m |
| `end day 4 at saint-michel` | Change to the route | End Day 4 · at Saint-Michel-de-Maurienne |
| `via Hotel Krone St. Peter` | Change to the route | Add a visit · Hotel Krone, St. Peter |
| `water gaps` | List of stretches | Longest stretches without mapped water · whole trip |
| `campsites with a pool end of day 4` | Partly understood | Campsites · End of Day 4 · "with a pool" not understood, ignored |
| `is the galibier worth it` | Not understood | Falls back to place search: Col du Galibier, pass, 2,642 m |

Offline: "Offline · results from downloaded areas". Outside a downloaded area: "Day 9 is outside
your offline areas" with the action "Download Alpes du Sud · 212 MB".
Empty (typed `bike shop end of day 4`): "No bike shops within 5 km of the end of Day 4." Actions:
"Search within 25 km", or the nearest one on the route: "Nearest: Cycles Maurienne,
Saint-Michel-de-Maurienne, km 88, 16 km before the end".
Needs a date: "Day 4 has no date." Action: "Set trip dates". The list still shows each shop's
Sunday hours.

## Bands for the profiles (km ranges)

Alps trip, x = trip km (0–634). Day boundaries: D1 0–78, D2 78–142, D3 142–213, D4 213–317,
D5 317–370, rest day at km 370 (no length), D7 370–426, D8 426–477, D9 477–540, D10 540–634.

- Unknown surface: 104–112 (Day 2), 398–402 (Day 7). Unpaved (known): 118–119.5, 400–401.
- Gaps (only when asked): water 213–242 "No mapped water for 29 km"; shops 477–540 "Last shop for
  63 km".
- Snow history (experiment): 222–228 (Iseran, 6 of 10 years free on the day), 497–501 (Bonette,
  8 of 10 years), 331–335 (Galibier, 9 of 10 years).
- Seasonal closure mark: 220–229 (Col de l'Iseran).

Day 4 alone, x = Day 4 km (0–104): water gap 0–29; snow 9–15; day window 80–92.

Glottertal → Titisee, Shortest, x = km (0–34): unpaved 12.5–15.6, of which unknown 14.2–15.6.
