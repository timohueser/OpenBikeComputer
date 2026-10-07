//! Plan: the groups of `plan live`, one per cause. Only a move can stay out (`--only`): the edits,
//! the code and the repair always go together.

use std::collections::BTreeSet;

use ratatui::style::{Color, Stylize};
use ratatui::text::Line;

use super::{bytes, duration, row_text, widths, App};
use crate::cli::build_cli::{change, Cost, EnvPlan, NONE};
use crate::cli::status_cli::keys;
use crate::engine::plan::{Cause, Group, Plan};

#[derive(Clone)]
pub(super) struct PlanView {
    /// The plan of every group, which Plan lists.
    pub all: EnvPlan,
    /// The plan of the moves that Plan takes: what a build of live does.
    pub taken: EnvPlan,
    /// The moves that Plan leaves out.
    pub skipped: BTreeSet<String>,
    /// Plan shows the fetches and builds instead of the groups.
    pub steps: bool,
    pub group: usize,
}

pub(super) fn is_move(group: &Group) -> bool {
    matches!(group.cause, Some(Cause::Move { .. }))
}

impl PlanView {
    pub fn new(plan: EnvPlan) -> Self {
        PlanView { taken: plan.clone(), all: plan, skipped: BTreeSet::new(), steps: false, group: 0 }
    }

    /// The `--only` of the moves that Plan takes: empty when it takes every move, `none` when it
    /// takes none.
    pub fn only(&self) -> Vec<String> {
        if self.skipped.is_empty() {
            return Vec::new();
        }
        let taken = self.all.groups.iter().filter(|group| is_move(group) && !self.skipped.contains(&group.id));
        let only: Vec<String> = taken.map(|group| group.id.clone()).collect();
        if only.is_empty() {
            return vec![NONE.into()];
        }
        only
    }

    /// Whether `space` takes or leaves the selected group: a move.
    pub fn toggles(&self) -> bool {
        self.all.groups.get(self.group).is_some_and(|group| !self.steps && is_move(group))
    }

    /// Take or leave the selected move, and give the `--only` of the moves that Plan takes then.
    pub fn toggle(&mut self) -> Vec<String> {
        let id = self.all.groups[self.group].id.clone();
        if !self.skipped.remove(&id) {
            self.skipped.insert(id);
        }
        self.only()
    }

    /// A refused selection keeps the last resolved plan and its checkboxes together.
    pub fn restore_selection(&mut self) {
        self.skipped = self
            .all
            .groups
            .iter()
            .filter(|group| is_move(group))
            .filter(|group| !self.taken.only.is_empty() && !self.taken.only.contains(&group.id))
            .map(|group| group.id.clone())
            .collect();
    }
}

impl App {
    /// The lines of Plan, the lines under them that always show, and the line of the selected
    /// group.
    pub(super) fn plan_lines(&self) -> (Vec<Line<'static>>, Vec<Line<'static>>, Option<usize>) {
        let Some(view) = &self.plan else { return (Vec::new(), Vec::new(), None) };
        let (all, taken) = (&view.all, &view.taken);
        let estimate = |value: Option<u64>, text: fn(u64) -> String| value.map_or("—".into(), text);
        let mut lines = Vec::new();
        if let Some(approval) = &taken.approval {
            lines.push(Line::from(approval.summary()));
        }
        let mut focus = None;
        if all.groups.is_empty() {
            lines.push(Line::from("Live has every change."));
        } else if !view.steps {
            let mut table = vec![["", "CHANGE", "FETCH", "TIME", "OUTPUT"].map(String::from).to_vec()];
            for group in &all.groups {
                let cost = Cost::of(std::slice::from_ref(group));
                let fetch = if group.fetches.is_empty() { String::new() } else { estimate(cost.fetch, bytes) };
                let mark = if !is_move(group) {
                    ""
                } else if view.skipped.contains(&group.id) {
                    "[ ]"
                } else {
                    "[x]"
                };
                table.push(vec![
                    mark.into(),
                    change(group, &all.edits),
                    fetch,
                    estimate(cost.wall_ms, duration),
                    estimate(cost.bytes_out, bytes),
                ]);
            }
            let widths = widths(&table);
            lines.push(Line::from(row_text(&table[0], &widths)).dim());
            for (i, cells) in table[1..].iter().enumerate() {
                let line = Line::from(row_text(cells, &widths));
                lines.push(if i == view.group { line.reversed() } else { line });
            }
            focus = Some(lines.len() - table.len() + view.group + 1);
        } else {
            let plan = Plan { groups: taken.groups.clone() };
            let fetches: Vec<Vec<String>> = plan
                .fetches()
                .into_iter()
                .map(|fetch| vec![fetch.source, fetch.version, estimate(fetch.bytes, bytes)])
                .collect();
            let mut steps = BTreeSet::new();
            let builds = plan.builds().filter(|build| steps.insert(build.step.clone()));
            let builds: Vec<Vec<String>> = builds
                .map(|build| vec![build.step.clone(), estimate(build.estimate.map(|e| e.wall_ms), duration)])
                .collect();
            for (title, rows) in
                [("FETCH", fetches), ("BUILD", builds)].into_iter().filter(|(_, rows)| !rows.is_empty())
            {
                lines.push(Line::from(title).dim());
                let widths = widths(&rows);
                lines.extend(rows.iter().map(|cells| Line::from(format!("  {}", row_text(cells, &widths)))));
            }
        }

        let removed = taken.remove.iter().map(|removal| removal.bytes).sum::<Option<u64>>();
        let mut footer = vec![
            Line::default(),
            Line::from(format!("REMOVE FROM R2  {}, {}", keys(taken.remove.len()), estimate(removed, bytes))).bold(),
        ];
        if taken.needs_prepare {
            footer
                .push(Line::styled("Inputs are unresolved. Prepare inputs, then review the new plan.", Color::Yellow));
        }
        let blocked = taken.blocked.iter().map(|blocked| format!("blocked {}: {}", blocked.product, blocked.reason));
        let unlisted = (!taken.listed).then(|| "R2 was not listed, so leftovers are unknown".to_string());
        footer.extend(blocked.chain(unlisted).map(|warning| Line::styled(format!("⚠ {warning}"), Color::Yellow)));
        if !taken.groups.is_empty() {
            let cost = Cost::of(&taken.groups);
            let builds = taken.groups.iter().flat_map(|group| &group.builds).map(|build| &build.step);
            let total = format!(
                "TOTAL  fetch {} · build {} layers, {} · output {}",
                estimate(cost.fetch, bytes),
                builds.collect::<BTreeSet<_>>().len(),
                estimate(cost.wall_ms, duration),
                estimate(cost.bytes_out, bytes),
            );
            footer.push(Line::from(total));
        }
        (lines, footer, focus)
    }
}
