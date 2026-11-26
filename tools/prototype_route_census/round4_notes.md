
# 5. Notes on the numbers of this run

These notes read the tables above. They are the reading of the prototype's author, who does not know the roads of
Engelberg or of Ireland.

## The pairs

- The share of drawn pairs with the same route is 13 % for `today` against `first` in Freiburg north, 58 % for `first`
  against `first+signed`, and 31 % for `gravel`. At Engelberg it is 54 % and 88 %: a valley often has one road.
- The 22 km pairs of `fr-touring-first` all differ. The 6 km pairs are the same most often.

## The roads of the Engelberg box

- The extracts of Lucerne, Obwalden, Nidwalden, Uri and Schwyz leave a hole in the south-west of the box. South of
  46.74° and west of 8.425°, 10 of the 11 cells have no junction; from 46.74° to 46.76°, the cells from 8.325° to
  8.425° have none and the cells west of them have fewer than 50. This is the canton of Bern: Hasliberg, the Gental
  and the Gadmertal. Few pairs of `eb-*` can have an end there, and no route can pass through it.
- The other thin cells are the lake of Lucerne and high ground. The map does not tell a missing extract from a
  mountain.

## Engelberg against Freiburg

- The census starts higher (77.9 % against 69.0 %) and gains less: `first` +3.4 points (Freiburg +5.3), `first+signed`
  +5.8 (Freiburg +7.4). The splits fall from 299 to 192 and 173.
- The panel changes in the same direction as in Freiburg: turns 1.78 → 1.44 per km, leave and rejoin 14.7 → 7.5 per
  100 km, total length −0.3 % and +0.8 %.
- Costs that Freiburg does not show: fast main roads are 3.3 km per 100 km today (Freiburg 0.55) and 3.6 with `first`;
  slow main roads rise from 2.7 to 4.1 km per 100 km. 28 % of the route length has no known surface (Freiburg 5 %).
- Half of the panel routes change with `first` (Freiburg 78 %).

## Ireland (Galway, the Burren, Ennis)

- The signed network is small: 31 relations with a way in the box, 436 km of line, 12 relations of 5 km or more, 84
  legs. One leg is 1.2 % of the census. The gains of +0.9 and +1.4 points come with 14 fewer splits of 43.
- No road of class 1 has a tagged limit of 80 km/h or more, so that column is 0. Of 5,430 km of class 1, 4,584 km
  (84 %) have no tagged limit and 429 km have 51 to 79 km/h. `traffic::stress` gives a road with no limit the
  default of its `highway` value (50 km/h for `tertiary` and `unclassified`), so the cost model cannot see a fast
  lane. The routes run 19 km per 100 km on class 1 with a tagged limit over 50 km/h, with each variant.
- Main roads carry far more of a route than in Freiburg: 5.8 km per 100 km fast and 6.1 slow today (Freiburg 0.55
  and 1.0). `first` does not lower them (6.0 and 7.0).
- 41 % of the route length has no known surface (Freiburg 5 %), so the factor 1.4 of a missing surface tag acts on
  two fifths of each route.
- The signal cost does nothing in Galway town: 226.7 signalled junctions per 100 km today, 226.4 with `first`
  (Freiburg town 149.7 → 89.8). Slow main roads rise from 9.9 to 12.1 km per 100 km. The count is high: 2.3 signalled
  junctions per km. The box has 568 signal nodes in 317 junctions, 1.8 nodes for each junction (Freiburg 1.4). The
  40 m rule joins two signal nodes only when one short road joins them; signal nodes on two arms of one junction,
  with the junction node between them, count as two. The count and the cost S depend on how a region maps its
  signals. The rows do not separate the turn rule from the signal cost.
- Leave and rejoin is 55.7 per 100 km in Galway town today (Freiburg town 21.4). The measure counts a roundabout or
  a link with no name as a leave, so it is not comparable between regions.
- In all three regions the roads with a cycle route rank are a small part of the network and carry a large part of
  the panel: Galway 360 of 8,939 km (4 %) carry 40 % of the route length today, Engelberg 4 % carry 43 %, Freiburg
  7 % carry 58 %.
