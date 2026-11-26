
# E. Notes on the numbers of this run

These notes read the tables above. They are the reading of the prototype's author, who looked at no cluster on a
map: the names of the roads are all that says where a cluster is.

## A. The signal rule

- **The rule as written leaves a door open, for each R.** A junction on a path between two signal nodes is in the
  cluster. Often it is the last junction before the stop line: a cycle path or a side road joins there, or the
  carriageway splits. A route that arrives there is in the cluster, so the signal node after it is free. In Freiburg
  town `first-c60` enters 55.2 clusters per 100 km through such a junction and then passes a signal node (`free`),
  and 22.5 at a signal node (`paid`). In Galway town: 63.5 and 64.4. The search finds the doors: `free` is 39.6 with
  `first` and 55.2 with `first-c60` in Freiburg town.
- So `first-c40` and `first-c60` do less than the 40 m rule in each panel. Freiburg town, clusters with a signal
  per 100 km: 90.0 today, 60.6 `first`, 75.7 and 77.7 with the cluster rule. Signal nodes: 180.9, 115.7, 152.7,
  154.2. Galway town, signal nodes: 293.2, 312.2, 338.6, 340.6.
- **With the door closed (`first-e40`, `first-e60`, not in the brief) the rule does in Freiburg town what the 40 m
  rule does.** Clusters with a signal: 59.9 and 62.8 (`first` 60.6). Length against today: +1.4 % and +1.1 %
  (`first` +1.1 %). Detour p95: 1.387 and 1.355 (`first` 1.352, today 1.232).
- **No signal rule lowers the signals of Galway town.** Clusters with a signal: 112.5 today, 120.5 `first`, 123.8
  with `first-e40` and with `first-e60`. Slow main roads carry 9.85 km per 100 km today and 12.1 to 12.4 with each
  variant. S = 60 m for each junction does not outweigh what the other switches of `first` gain on the main roads.
- What the cluster rule changes in Galway is the count of the payments. The 40 m rule pays 226.4 times per 100 km
  in Galway town and 89.8 times in Freiburg town (`round4.md`): 2.5 to 1. `first-e40` pays 137.9 and 72.9 times
  (column `R = 40`): 1.9 to 1. `first-e60` pays 141.2 and 77.0 times: 1.8 to 1. The clusters with a signal are 2.0
  to 1, so what stays is the count of the junctions, not the style of the map.
- **R = 40 and R = 60 give almost the same routes.** Galway town: each column agrees to 0.2. Engelberg: signal
  nodes 14.7 and 15.0. Freiburg town: 59.9 against 62.8 clusters with a signal.
- **R = 60 chains junctions along a street; R = 40 does it less.** Clusters over 100 m across, for R = 40 and
  R = 60: Freiburg 3 and 15, Galway 0 and 3, the Engelberg box 3 and 15, Alsace 14 and 49. Over 200 m: none for
  R = 40. For R = 60: two in the Engelberg box (275 m and 218 m; Lucerne, by the names of the roads) and four in
  Alsace (the widest are 242 m and 220 m in Strasbourg and 208 m in Mulhouse).
- R = 40 still joins more than one junction in places. The widest cluster of Freiburg is 170 m across with 16
  signal nodes; its ten road names include two bridges (Schwabentorbrücke, Greiffeneggbrücke).
- A cost of the closed door: the route also pays where it touches a cluster and passes no signal node. With R = 60
  that is 14.2 of 77.0 payments per 100 km in Freiburg town and 17.4 of 141.2 in Galway town (`clusters entered`
  less `with a signal`).
- `clusters entered` is not always smaller for R = 60 than for R = 40: a cluster of R = 60 has more junctions with
  no signal, so a route touches it more often.

## B. The through keys

- **The three key rules give the same routes.** Gain of the census for (i), (ii), (iii): Freiburg +5.3, +5.4, +5.3;
  Engelberg +3.4, +3.4, +3.4; Alsace +4.5, +4.6, +4.5. Main road km per 100 km, fast plus slow: Freiburg 1.79,
  1.77, 1.79 (today 1.57); Engelberg 7.74, 7.74, 7.74 (today 6.06); Alsace 2.16, 2.08, 2.16 (today 1.90).
- Rule (iii) gives the numbers of (i) to each printed digit but `settled`, which shows that its costs differ.
- Cause: the `ref` is the only key of few arcs, and they pay little. It is the only key of 4 % of the arcs with a
  key in Freiburg, 1 % in the Engelberg box and 9 % in Alsace. A road with a `ref` mostly runs straight on at a
  junction, and the turn cost goes by the angle: about 20 m for each such arc, and three in four pay 10 m or less.
- **The main road km come from the turn cost itself, not from a key.** Engelberg, slow main km per 100 km: 2.72
  today; 2.90 with V1 and S60 only (`first-noturn`); 3.86 with the turn cost and no key (`first-nokey`); 4.14 with
  the keys. Of the +1.42, the keys give 0.28. Freiburg: 1.02, 0.92, 1.12, 1.27. Alsace, fast main: 0.98, 0.93,
  1.24, 1.24.
- The keys `name` and route relation carry about half of the gain. Gain with no key: Freiburg +2.8 of +5.3,
  Engelberg +1.4 of +3.4, Alsace +2.2 of +4.5. With no turn cost: +0.4, +0.2, +1.1.
- So no key rule can take back more than 0.28 km per 100 km at Engelberg, and a rule that goes that far costs
  about 2 points of the gain.

## C. The other profiles (Freiburg)

- `gravel` gains more on the census than `touring`: 47.8 % to 59.1 % (+11.3 points; `touring` +5.3). The panel is
  0.9 % shorter; the detour p95 is 1.213 and 1.215; turns fall from 1.86 to 1.41 per km.
- **`first` moves `gravel` onto paved roads**: paved 44.2 % to 50.3 %, compacted 24.0 % to 21.7 %, gravel 14.9 %
  to 13.6 %, dirt 5.1 % to 4.1 %. The share on a signed cycle route rises from 31.2 % to 45.2 %. 79 % of the routes
  differ.
- `mtb` changes less: length +0.3 %, detour p95 1.165 to 1.178, turns 1.91 to 1.41 per km, paved 38.1 % to 39.0 %,
  dirt 19.4 % to 17.6 %, signal nodes 13.3 to 8.5 per 100 km. 74 % of the routes differ.
- `road-c60`, the rule as written, raises the signals of `road` in town: 143.2 to 163.4 signal nodes per 100 km.
  It pays 60 m at 30.4 clusters per 100 km (1.8 km); today `road` pays 20 m at 143.2 nodes (2.9 km).
- `road-e60` pays 60 m at 85.3 clusters (5.1 km) and changes little: signal nodes 143.2 to 140.4, clusters with a
  signal 79.5 to 74.4, length +0.2 %, detour p95 1.281 to 1.279. 19 % of the town routes and 6 % of the panel
  routes differ.

## Not measured

- No cluster on a map. A town panel of Lucerne or Strasbourg, where the clusters of R = 60 are largest. The
  cluster rules in Alsace.
- An R other than 40 and 60, an S other than 60, and the cluster rule for `gravel` and `mtb`.
- The census of the cluster rows: it ran, and the split lists are in `round5/`, but no table shows it.
- The `name` key apart from the route relation key, and a rule with no key at all on a main road.
- V1 apart from the turn cost for `gravel` and `mtb`. A second region for section C. The census of `mtb` and `road`.
- The production query and the bytes of a package with a cluster rule. Each rule here is a cost of an arc, as the
  40 m rule is.
