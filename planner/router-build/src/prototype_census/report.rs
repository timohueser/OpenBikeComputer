//! PROTOTYPE. The tables of the census.
use super::census::{Census, Split, Stretch, Tally};
use super::terms::{self, GROUPS, N, NAMES};
use std::collections::BTreeMap;
use std::fmt::Write;

/// The 20 splits with the largest cost difference per km come from the splits whose signed
/// stretch is at least this long, so that a 30 m corner does not fill the list.
const TOP_MIN_M: f64 = 300.0;

pub fn km(metres: f64) -> String {
    format!("{:.1}", metres / 1000.0)
}

pub fn share(part: f64, whole: f64) -> String {
    if whole > 0.0 {
        format!("{:.1} %", 100.0 * part / whole)
    } else {
        "-".into()
    }
}

fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    sorted[((sorted.len() - 1) as f64 * q).round() as usize]
}

fn quantiles(mut values: Vec<f64>) -> String {
    values.sort_by(f64::total_cmp);
    let cells: Vec<String> =
        [0.05, 0.25, 0.5, 0.75, 0.95].iter().map(|&q| format!("{:.2}", quantile(&values, q))).collect();
    format!("| {} | {} |", values.len(), cells.join(" | "))
}

pub fn kind(split: &Split) -> &'static str {
    if split.router.roads.is_empty() {
        "loop"
    } else if split.signed_far_m < 1.0 && split.router_far_m < 1.0 {
        "parallel"
    } else {
        "apart"
    }
}

fn difference(split: &Split) -> f64 {
    split.signed.cost - split.router.cost
}

/// The fine term with the largest part of the cost difference.
fn top_fine(split: &Split) -> (&'static str, f64) {
    let mut best = ("none (equal cost)", 0.5);
    for (i, name) in NAMES.iter().enumerate() {
        if split.signed.terms[i] - split.router.terms[i] > best.1 {
            best = (name, split.signed.terms[i] - split.router.terms[i]);
        }
    }
    best
}

/// The group of terms with the largest part of the cost difference, with that part.
pub fn top_term(split: &Split) -> (&'static str, f64) {
    let (signed, router) = (terms::grouped(&split.signed.terms), terms::grouped(&split.router.terms));
    let mut best = ("none (equal cost)", 0.5);
    for (i, (name, _)) in GROUPS.iter().enumerate() {
        if signed[i] - router[i] > best.1 {
            best = (name, signed[i] - router[i]);
        }
    }
    best
}

fn signature(stretch: &Stretch, most: usize) -> String {
    let mut parts: Vec<_> = stretch.signature.iter().collect();
    parts.sort_by(|a, b| b.1.total_cmp(a.1));
    let parts =
        parts.iter().take(most).map(|(name, m)| format!("{name} {:.0}%", 100.0 * *m / stretch.length_m.max(1.0)));
    parts.collect::<Vec<_>>().join("; ")
}

fn degrees(udeg: i32) -> String {
    format!("{:.6}", f64::from(udeg) * 1e-6)
}

fn ways(stretch: &Stretch) -> String {
    stretch.ways.iter().map(i64::to_string).collect::<Vec<_>>().join(" ")
}

pub fn splits_csv(census: &Census) -> String {
    let mut out = String::from(
        "split,seen,relation,level,kind,from_lon,from_lat,to_lon,to_lat,signed_m,router_m,length_ratio,signed_cost,\
         router_cost,cost_difference,cost_difference_per_signed_km,top_term,signed_far_30m_m,router_far_30m_m,\
         signed_turns,router_turns,signed_signals,router_signals,at_point,signed_cost_full_bonus,\
         router_cost_full_bonus,signed_cost_unknown_paved,router_cost_unknown_paved",
    );
    for side in ["signed", "router"] {
        for name in NAMES {
            write!(out, ",{side}_{name}").unwrap();
        }
    }
    out.push_str(",signed_signature,router_signature,signed_ways,router_ways\n");
    for (i, split) in census.splits.iter().enumerate() {
        let (s, r) = (&split.signed, &split.router);
        write!(
            out,
            "{},{},{},{},{},{},{},{},{},{:.0},{:.0},{:.3},{:.0},{:.0},{:.0},{:.0},{},{:.0},{:.0},{},{},{},{},{},{:.0},{:.0},{:.0},{:.0}",
            i + 1,
            split.seen,
            split.relation,
            census.level_names[usize::from(split.level)],
            kind(split),
            degrees(split.from.lon),
            degrees(split.from.lat),
            degrees(split.to.lon),
            degrees(split.to.lat),
            s.length_m,
            r.length_m,
            s.length_m / r.length_m,
            s.cost,
            r.cost,
            difference(split),
            difference(split) / (s.length_m / 1000.0),
            top_term(split).0,
            split.signed_far_m,
            split.router_far_m,
            s.turns,
            r.turns,
            s.signals,
            r.signals,
            split.at_point,
            s.full_bonus,
            r.full_bonus,
            s.unknown_paved,
            r.unknown_paved,
        )
        .unwrap();
        for stretch in [s, r] {
            for term in stretch.terms {
                write!(out, ",{term:.0}").unwrap();
            }
        }
        writeln!(out, ",{},{},{},{}", signature(s, 4), signature(r, 4), ways(s), ways(r)).unwrap();
    }
    out
}

pub fn relations_csv(census: &Census) -> String {
    let mut out = String::from(
        "relation,name,network,level,line_km,no_touring_road_km,against_oneway_km,short_chain_km,measured_km,within_30m_km,\
         same_directed_road_km,legs,identical_legs,splits,split_signed_km,signed_cost,router_cost\n",
    );
    for route in &census.routes {
        let t = &route.tally;
        writeln!(
            out,
            "{},\"{}\",{},{},{},{},{},{},{},{},{},{},{},{},{},{:.0},{:.0}",
            route.id,
            route.name.replace('"', "'"),
            route.network,
            census.level_names[usize::from(route.level)],
            km(t.line_m),
            km(t.lost_m),
            km(t.oneway_m),
            km(t.short_m),
            km(t.measured_m),
            km(t.near_m),
            km(t.same_road_m),
            t.legs,
            t.identical_legs,
            t.splits,
            km(t.split_m),
            t.signed_cost,
            t.router_cost,
        )
        .unwrap();
    }
    out
}

fn follow_row(out: &mut String, name: &str, t: &Tally) {
    writeln!(
        out,
        "| {name} | {} | {} | {} | {} | {} | {} | {} | **{}** | {} | {} | {} | {} | {} | {} |",
        t.relations,
        km(t.line_m),
        km(t.lost_m),
        km(t.oneway_m),
        km(t.short_m),
        km(t.measured_m),
        km(t.near_m),
        share(t.near_m, t.measured_m),
        share(t.same_road_m, t.measured_m),
        t.legs,
        share(t.identical_legs as f64, t.legs as f64),
        t.splits,
        km(t.split_m),
        share(t.split_m, t.measured_m),
    )
    .unwrap();
}

pub fn ranking(out: &mut String, title: &str, splits: &[&Split], top: fn(&Split) -> (&'static str, f64)) {
    let mut tally = BTreeMap::<&str, (usize, f64, f64)>::new();
    for split in splits {
        let entry = tally.entry(top(split).0).or_default();
        *entry = (entry.0 + 1, entry.1 + split.signed.length_m, entry.2 + difference(split));
    }
    let count = splits.len() as f64;
    let length: f64 = splits.iter().map(|s| s.signed.length_m).sum();
    let mut rows: Vec<_> = tally.into_iter().collect();
    rows.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    writeln!(out, "\n{title}\n\n| largest term of the difference | splits | share of splits | signed km | share of signed km | sum of cost difference |\n|---|---|---|---|---|---|").unwrap();
    for (name, (n, metres, cost)) in rows {
        writeln!(
            out,
            "| {name} | {n} | {} | {} | {} | {cost:.0} |",
            share(n as f64, count),
            km(metres),
            share(metres, length)
        )
        .unwrap();
    }
}

fn term_sums(out: &mut String, splits: &[&Split]) {
    let (mut signed, mut router) = ([0.0; N], [0.0; N]);
    for split in splits {
        terms::add(&mut signed, &split.signed.terms);
        terms::add(&mut router, &split.router.terms);
    }
    let total: f64 = signed.iter().sum::<f64>() - router.iter().sum::<f64>();
    writeln!(out, "\n| term | signed stretches | router's stretches | signed − router | share of the whole difference |\n|---|---|---|---|---|").unwrap();
    for i in 0..N {
        let d = signed[i] - router[i];
        writeln!(out, "| {} | {:.0} | {:.0} | {d:.0} | {} |", NAMES[i], signed[i], router[i], share(d, total)).unwrap();
    }
    writeln!(
        out,
        "| **all** | {:.0} | {:.0} | {total:.0} | 100 % |",
        signed.iter().sum::<f64>(),
        router.iter().sum::<f64>()
    )
    .unwrap();
}

fn histogram(out: &mut String, values: &[f64], edges: &[f64]) {
    let total = values.len() as f64;
    let mut low = f64::NEG_INFINITY;
    for &high in edges.iter().chain([&f64::INFINITY]) {
        let n = values.iter().filter(|&&v| v > low && v <= high).count();
        let label = if low == f64::NEG_INFINITY {
            format!("≤ {high}")
        } else if high == f64::INFINITY {
            format!("> {low}")
        } else {
            format!("{low} – {high}")
        };
        writeln!(out, "| {label} | {n} | {} |", share(n as f64, total)).unwrap();
        low = high;
    }
}

fn rate(count: u32, metres: f64) -> String {
    format!("{:.2}", f64::from(count) / (metres / 1000.0).max(1e-9))
}

pub struct Facts<'a> {
    pub inputs: Vec<String>,
    pub stages: &'a [(String, f64)],
    pub counts: Vec<(String, String)>,
    pub step_m: f64,
    pub min_line_m: f64,
    pub terrain: bool,
}

pub fn census_md(census: &Census, facts: &Facts) -> String {
    let mut out = String::from("# Route census, profile `touring` (prototype for issue 3836)\n\n");
    for line in &facts.inputs {
        writeln!(out, "- {line}").unwrap();
    }
    write!(
        out,
        "\n## Method and limits\n\n\
         - A relation counts when it has `route=bicycle`, `type=route|superroute`, not `state=proposed`, and at least \
         {min} km of main line in the box. {seen} relations have the tags; {under} have less than {min} km. The main \
         line is `catalog::assemble::main_line` on the member ways inside the box (forward branch of a one-way split \
         only), each run in its own direction. No relation is routed against its direction.\n\
         - The signed line is matched to the directed roads of the graph exactly, by the node pairs of the member \
         ways. Line with no directed road that `touring` may use (no bike or push access in that direction, a \
         difficulty over the limit, a way that is no road) cuts the chain and is not measured: column `no road`. \
         Line that runs against a one-way for bicycles, where `touring` would push, cuts the chain too: column \
         `against one-way`. A chain under 1 km is not measured: column `short`.\n\
         - Points lie at junctions about every {step} km on each chain. Each leg is one exact bidirectional Dijkstra \
         search (`planner_router::search::Workspace::run`) on the real road and turn costs of the bake, from any \
         road that arrives at the first junction to any road that arrives at the second one. The signed stretch is \
         one candidate of that search, so the router's stretch never costs more.\n\
         - A split runs from the first road that differs to the next road that both lists take; a place where the \
         two lines only cross does not end a split. `parallel`: each stretch is within 30 m of the other line. \
         `apart`: some part is farther. `loop`: the signed line leaves a junction and comes back to it; the router's \
         stretch is empty.\n\
         - Relations overlap: a stretch in three relations is measured three times. The splits are listed once \
         (`seen` counts the repeats); the aggregates of section 3 use each split once.\n\
         - Level: the highest network of the relation or of a route above it, as `osm::cycle_ranks` gives it to the \
         ways. `other` is a route with no network tag of lcn, rcn, ncn or icn. A way gets the bonus of the highest \
         route on it, also in the census of a lower relation.\n\
         - Cost terms: the factors multiply, so a term is the cost that its factor adds on top of the factors before \
         it, in the order of the code: distance, surface, smoothness/tracktype, difficulty, pushing, traffic stress, \
         class preference, route bonus, uncertain access (way ×3, node +300), climb. The split of each road is \
         checked against the factor and the total of the bake.\n\
         - A turn is a change of heading over 45° (headings over 15 m) at a junction of three or more road ends. \
         Signals are signal nodes inside a stretch, not at its two ends.\n\
         - {terrain}\n",
        step = facts.step_m / 1000.0,
        min = facts.min_line_m / 1000.0,
        seen = census.relations_seen,
        under = census.relations_under_5km,
        terrain = if facts.terrain {
            "Terrain: Copernicus GLO-30 only, without the national reference models of the production bake. The climb term is 10 per metre of ascent."
        } else {
            "NO TERRAIN: every climb term is 0. `touring` charges 10 per metre of climb in production, so each number here lacks that term."
        },
    )
    .unwrap();
    writeln!(
        out,
        "- Checks: {} legs where the router's stretch costs more than the signed one (must be 0); {} member-way runs \
         with no road; {} node pairs with two roads.",
        census.optimality_violations, census.unmatched_runs, census.pair_conflicts
    )
    .unwrap();

    out.push_str(
        "\n## 1. Share of the signed length that the router follows\n\n\
         | level | relations | line km | no road km | against one-way km | short km | measured km | within 30 m km | \
         **within 30 m** | on the same directed road | legs | legs with no split | splits | split signed km | \
         split share |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for level in (1..6).rev() {
        follow_row(&mut out, census.level_names[level], &census.levels[level]);
    }
    follow_row(&mut out, "**all**", &census.total);
    writeln!(
        out,
        "\nEach signed directed road once: {} km measured, {} km in a split ({}).\n\
         Cost of all measured legs: signed {:.0}, router {:.0} ({}).",
        km(census.unique_measured_m),
        km(census.unique_split_m),
        share(census.unique_split_m, census.unique_measured_m),
        census.total.signed_cost,
        census.total.router_cost,
        share(census.total.router_cost, census.total.signed_cost),
    )
    .unwrap();

    out.push_str(
        "\nA level with no relation has its relations under a route of a higher level, or below the length limit.\n",
    );
    out.push_str("\nThe relations with less main line than the limit, which the census leaves out:\n\n| level | relations | with a relation as a member | line km |\n|---|---|---|---|\n");
    for level in (1..6).rev() {
        let (count, metres, parents) = census.excluded[level];
        writeln!(out, "| {} | {count} | {parents} | {} |", census.level_names[level], km(metres)).unwrap();
    }

    let all: Vec<&Split> = census.splits.iter().collect();
    let apart: Vec<&Split> = all.iter().copied().filter(|s| kind(s) == "apart").collect();
    let count = |name: &str| all.iter().filter(|s| kind(s) == name).count();
    let signed_m: f64 = all.iter().map(|s| s.signed.length_m).sum();
    let router_m: f64 = all.iter().map(|s| s.router.length_m).sum();
    writeln!(
        out,
        "\n## 3. Aggregates of the splits\n\n\
         {} splits ({} with repeats in other relations and legs): {} apart, {} parallel, {} loop. They cover {} of the \
         measured signed length with repeats, and {} with each road once. Signed stretches: {} km; the router's \
         stretches: {} km.",
        all.len(),
        census.total.splits,
        count("apart"),
        count("parallel"),
        count("loop"),
        share(census.total.split_m, census.total.measured_m),
        share(census.unique_split_m, census.unique_measured_m),
        km(signed_m),
        km(router_m),
    )
    .unwrap();

    let with_router: Vec<&Split> = all.iter().copied().filter(|s| s.router.length_m > 0.0).collect();
    let ratios: Vec<f64> = with_router.iter().map(|s| s.signed.length_m / s.router.length_m).collect();
    out.push_str("\n### Length ratio signed / router (splits with a router stretch)\n\n| | splits | p5 | p25 | p50 | p75 | p95 |\n|---|---|---|---|---|---|---|\n");
    writeln!(out, "| all {}", quantiles(ratios.clone())).unwrap();
    let apart_ratios = apart.iter().map(|s| s.signed.length_m / s.router.length_m).collect();
    writeln!(out, "| apart {}", quantiles(apart_ratios)).unwrap();
    out.push_str("\n| length ratio | splits | share |\n|---|---|---|\n");
    histogram(&mut out, &ratios, &[0.8, 0.95, 1.0, 1.05, 1.2, 1.5, 2.0]);
    let within = ratios.iter().filter(|&&r| r <= 1.05).count();
    let shorter = ratios.iter().filter(|&&r| r <= 1.0).count();
    writeln!(
        out,
        "\nSigned stretch at most 5 % longer than the router's stretch: **{within} of {}** ({}); of them {shorter} \
         are not longer at all.",
        ratios.len(),
        share(within as f64, ratios.len() as f64),
    )
    .unwrap();
    out.push_str("\n### Cost ratio signed / router\n\n| | splits | p5 | p25 | p50 | p75 | p95 |\n|---|---|---|---|---|---|---|\n");
    writeln!(out, "| all {}", quantiles(with_router.iter().map(|s| s.signed.cost / s.router.cost).collect())).unwrap();
    writeln!(out, "| apart {}", quantiles(apart.iter().map(|s| s.signed.cost / s.router.cost).collect())).unwrap();

    out.push_str("\n### Which term pays for the difference\n\nFor each split, the term with the largest part of (cost of the signed stretch − cost of the router's stretch).\n");
    ranking(&mut out, "All splits:", &all, top_term);
    ranking(&mut out, "Splits `apart` only:", &apart, top_term);
    ranking(
        &mut out,
        "All splits, with the fine terms (the class preference by road class, the surface by tag):",
        &all,
        top_fine,
    );
    out.push_str(
        "\nThe sums of each term over all splits (a negative difference is a term that favours the signed stretch):\n",
    );
    term_sums(&mut out, &all);

    let stays = |cost: fn(&Stretch) -> f64| {
        let left: Vec<&&Split> = all.iter().filter(|s| cost(&s.signed) > cost(&s.router) + 0.5).collect();
        let metres: f64 = left.iter().map(|s| s.signed.length_m).sum();
        format!(
            "{} of {} ({}) | {} of {} ({})",
            left.len(),
            all.len(),
            share(left.len() as f64, all.len() as f64),
            km(metres),
            km(signed_m),
            share(metres, signed_m)
        )
    };
    write!(
        out,
        "\n### The reach of two listed changes (upper bounds, not proposals)\n\n\
         Each row prices both stretches of each split again and counts the splits where the signed stretch still \
         costs more than the router's stretch of today. A split that leaves the count is not yet a followed route: \
         the router can have a third way.\n\n\
         | pricing | splits where the signed stretch costs more | their signed km |\n|---|---|---|\n\
         | today | {} |\n\
         | a road in a signed route loses its whole class preference (a route bonus of 100 %; today 10 % per level, \
         and `Profile::validate` allows less than 20 %) | {} |\n\
         | a road with no known surface gets the surface factor of a paved road | {} |\n",
        stays(|s| s.cost),
        stays(|s| s.full_bonus),
        stays(|s| s.unknown_paved),
    )
    .unwrap();
    let cut = all.iter().filter(|s| s.at_point).count();
    writeln!(
        out,
        "\n{cut} of the {} splits start or end at a point of the census; the points may have cut them.",
        all.len()
    )
    .unwrap();

    // The tag signatures of the signed stretches.
    let mut signatures = BTreeMap::<&str, (f64, usize, usize, f64, f64)>::new();
    for split in &all {
        let largest = split.signed.signature.iter().max_by(|a, b| a.1.total_cmp(b.1)).map(|(name, _)| name.as_str());
        for (name, metres) in &split.signed.signature {
            let entry = signatures.entry(name).or_default();
            let repeats = entry.4 + metres * split.seen as f64;
            *entry = (entry.0 + metres, entry.1 + 1, entry.2 + usize::from(largest == Some(name)), entry.3, repeats);
        }
        for (name, metres) in &split.router.signature {
            signatures.entry(name).or_default().3 += metres;
        }
    }
    let mut rows: Vec<_> = signatures.into_iter().collect();
    rows.sort_by(|a, b| b.1 .0.total_cmp(&a.1 .0));
    out.push_str(
        "\n### The 15 tag signatures with the most signed length that the router avoided\n\n\
         Signature: `highway|surface|bicycle|tracktype|maxspeed`. `measured` is the signed length with this \
         signature in all measured legs, with repeats; `avoided share` is its length in splits, with repeats, as a \
         share of that.\n\n\
         | signature | signed km in splits | splits with it | splits where it is the largest | km on the router's \
         stretches | measured signed km | avoided share |\n|---|---|---|---|---|---|---|\n",
    );
    for (name, (metres, with, largest, router, repeats)) in rows.iter().take(15) {
        let measured = census.measured_signature.get(*name).copied().unwrap_or(0.0);
        let avoided = share(*repeats, measured);
        writeln!(
            out,
            "| `{name}` | {} | {with} | {largest} | {} | {} | {avoided} |",
            km(*metres),
            km(*router),
            km(measured)
        )
        .unwrap();
    }
    out.push_str("\nThe 15 largest signatures of the measured signed length, with repeats, for comparison:\n\n| signature | measured signed km | share of measured | in splits |\n|---|---|---|---|\n");
    let mut measured: Vec<_> = census.measured_signature.iter().collect();
    measured.sort_by(|a, b| b.1.total_cmp(a.1));
    for (name, metres) in measured.iter().take(15) {
        let repeats = rows.iter().find(|row| row.0 == name.as_str()).map_or(0.0, |row| row.1 .4);
        writeln!(
            out,
            "| `{name}` | {} | {} | {} |",
            km(**metres),
            share(**metres, census.total.measured_m),
            share(repeats, **metres)
        )
        .unwrap();
    }

    out.push_str("\n### Turns and signals\n\n| | signed stretches | router's stretches |\n|---|---|---|\n");
    for (name, splits) in [("all splits", &all), ("apart", &apart)] {
        let sum = |pick: fn(&Split) -> (u32, u32, f64)| {
            splits.iter().map(|s| pick(s)).fold((0, 0, 0.0), |a, b| (a.0 + b.0, a.1 + b.1, a.2 + b.2))
        };
        let s = sum(|s| (s.signed.turns, s.signed.signals, s.signed.length_m));
        let r = sum(|s| (s.router.turns, s.router.signals, s.router.length_m));
        writeln!(
            out,
            "| turns per km, {name} | {} ({} on {} km) | {} ({} on {} km) |",
            rate(s.0, s.2),
            s.0,
            km(s.2),
            rate(r.0, r.2),
            r.0,
            km(r.2)
        )
        .unwrap();
        writeln!(out, "| signals per km, {name} | {} ({}) | {} ({}) |", rate(s.1, s.2), s.1, rate(r.1, r.2), r.1)
            .unwrap();
    }
    let more = all.iter().filter(|s| s.router.turns > s.signed.turns).count();
    let fewer = all.iter().filter(|s| s.router.turns < s.signed.turns).count();
    writeln!(
        out,
        "\nThe router's stretch has more turns than the signed one in {more} splits, fewer in {fewer}, the same count \
         in {}. Where it has more turns, the cost difference for each extra turn is:\n\n| | splits | p5 | p25 | p50 | \
         p75 | p95 |\n|---|---|---|---|---|---|---|",
        all.len() - more - fewer
    )
    .unwrap();
    let extra: Vec<f64> = all
        .iter()
        .filter(|s| s.router.turns > s.signed.turns)
        .map(|s| difference(s) / f64::from(s.router.turns - s.signed.turns))
        .collect();
    writeln!(out, "| cost difference per extra turn {}", quantiles(extra)).unwrap();

    let mut top: Vec<&Split> = all.iter().copied().filter(|s| s.signed.length_m >= TOP_MIN_M).collect();
    top.sort_by(|a, b| (difference(b) / b.signed.length_m).total_cmp(&(difference(a) / a.signed.length_m)));
    writeln!(
        out,
        "\n## 4. The 20 splits with the largest cost difference per km\n\nOf the {} splits with a signed stretch of at \
         least {TOP_MIN_M} m. The cost difference is per km of the signed stretch.",
        top.len()
    )
    .unwrap();
    for (i, split) in top.iter().take(20).enumerate() {
        let (s, r) = (&split.signed, &split.router);
        let (term, part) = top_term(split);
        let place = |p: planner_router::model::Point| {
            let (lat, lon) = (degrees(p.lat), degrees(p.lon));
            format!("{lat}, {lon} (https://www.openstreetmap.org/?mlat={lat}&mlon={lon}#map=17/{lat}/{lon})")
        };
        write!(
            out,
            "\n### {}. {:.0} per km: relation {} {} ({}), {}\n\n\
             - from {}\n- to {}\n\
             - cost: signed {:.0} on {:.0} m, router {:.0} on {:.0} m; largest term: {term} ({part:+.0})\n\
             - signed: {}; {} turns, {} signals; ways {}\n\
             - router: {}; {} turns, {} signals; ways {}\n",
            i + 1,
            difference(split) / (s.length_m / 1000.0),
            split.relation,
            census.routes.iter().find(|route| route.id == split.relation).map_or("", |route| route.name.as_str()),
            census.level_names[usize::from(split.level)],
            kind(split),
            place(split.from),
            place(split.to),
            s.cost,
            s.length_m,
            r.cost,
            r.length_m,
            signature(s, 3),
            s.turns,
            s.signals,
            ways(s),
            signature(r, 3),
            r.turns,
            r.signals,
            ways(r),
        )
        .unwrap();
    }

    out.push_str("\n## By-products\n\n| stage of the bake | seconds |\n|---|---|\n");
    for (name, seconds) in facts.stages {
        writeln!(out, "| {name} | {seconds:.1} |").unwrap();
    }
    out.push_str("\n| count | value |\n|---|---|\n");
    for (name, value) in &facts.counts {
        writeln!(out, "| {name} | {value} |").unwrap();
    }
    writeln!(
        out,
        "| queries of the census | {} |\n| mean time of a query | {:.1} ms |\n\nA query here is one leg of about {} km \
         with plain bidirectional Dijkstra on the graph in memory, without the landmarks, the snap and the answer of \
         the production router.",
        census.queries,
        1000.0 * census.query_seconds / census.queries.max(1) as f64,
        facts.step_m / 1000.0,
    )
    .unwrap();
    out
}
