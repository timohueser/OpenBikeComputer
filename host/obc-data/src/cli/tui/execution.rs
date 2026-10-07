//! Detached operations use the shared start, observation, stop and reconciliation APIs.

use std::path::Path;
use std::sync::Arc;

use ratatui::text::Line;

use super::{App, Error};
use crate::cli::{build_cli::EnvPlan, operation_cli};
use crate::engine::runs::{Outcome, Publication};
use crate::operation::{Kind, Request, Status};
use crate::store::Store;

#[derive(Clone, Default)]
pub(super) struct Execution {
    pub selected: Option<String>,
    pub handle: Option<operation_cli::Handle>,
    pub view: Option<Arc<operation_cli::View>>,
    pub active: Option<Arc<operation_cli::View>>,
}

impl Execution {
    pub fn current(&self) -> Option<&operation_cli::View> {
        self.view.as_deref().filter(|view| Some(&view.run.summary.id) == self.selected.as_ref())
    }

    pub fn prepared(&self) -> Option<EnvPlan> {
        let view = self.current()?;
        if !matches!(view.operation, Some(Status::Finished { ok: true })) {
            return None;
        }
        serde_json::from_value(view.result.as_ref()?.get("plan")?.clone()).ok()
    }

    pub fn can_stop(&self) -> bool {
        self.current().is_some_and(|view| matches!(view.operation, Some(Status::Starting | Status::Running)))
    }

    pub fn can_reconcile(&self) -> bool {
        self.current().is_some_and(|view| {
            matches!(
                view.operation,
                Some(
                    Status::AwaitingOwner { .. }
                        | Status::UnknownOwner { .. }
                        | Status::Interrupted
                        | Status::Stopped
                        | Status::Finished { .. }
                )
            )
        })
    }
}

pub(super) fn start(root: &Path, store: &Store, kind: Kind, plan: &EnvPlan) -> Result<operation_cli::Handle, Error> {
    operation_cli::start(root, store, request(kind, plan), (kind != Kind::Prepare).then_some(plan))
}

pub(super) fn request(kind: Kind, plan: &EnvPlan) -> Request {
    let prepare = kind == Kind::Prepare;
    Request {
        kind,
        env: plan.env.clone(),
        only: if prepare { plan.only.clone() } else { Vec::new() },
        moves: if prepare {
            plan.moves
                .iter()
                .map(|(source, version)| {
                    version.as_ref().map_or_else(|| source.clone(), |version| format!("{source}@{version}"))
                })
                .collect()
        } else {
            Vec::new()
        },
        plan: None,
    }
}

pub(super) fn state(view: &operation_cli::View) -> &'static str {
    match view.operation {
        Some(Status::Starting) => "starting",
        Some(Status::Running) => "running",
        Some(Status::Stopping) => "stopping · admitted work drains",
        Some(Status::Stopped) => "stopped",
        Some(Status::Interrupted) => "interrupted",
        Some(Status::AwaitingOwner { .. }) => "awaiting publication owner",
        Some(Status::UnknownOwner { .. }) => "publication outcome unknown",
        Some(Status::Finished { ok: true }) => "finished",
        Some(Status::Finished { ok: false }) => "failed",
        None => match view.run.summary.outcome {
            Outcome::Running => "running",
            Outcome::Ok => "finished",
            Outcome::Failed => "failed",
        },
    }
}

impl App {
    pub(super) fn run_lines(&self) -> Vec<Line<'static>> {
        let Some(view) = self.execution.current() else {
            return vec![Line::from("Reading run progress…")];
        };
        let run = &view.run;
        let mut lines =
            vec![Line::from(format!("{} · {}", run.summary.id, run.summary.command)), Line::from(state(view))];
        let switched: Vec<_> = run
            .published
            .iter()
            .filter_map(|mutation| match mutation {
                Publication::Switched { product, release } => {
                    Some(format!("{product} release {}", &release[..release.len().min(8)]))
                }
                _ => None,
            })
            .collect();
        let activated = run.published.iter().any(|mutation| matches!(mutation, Publication::ServicesActivated { .. }));
        if !switched.is_empty() || activated {
            if run.summary.outcome == Outcome::Failed {
                lines.push(Line::from("Apply did not finish. Acknowledged publication changes remain live."));
            }
            if !switched.is_empty() {
                lines.push(Line::from(format!("Pointer switches acknowledged: {}", switched.join(", "))));
            }
            if activated {
                lines.push(Line::from("Service activation acknowledged."));
            }
        } else if matches!(view.operation, Some(Status::UnknownOwner { .. } | Status::AwaitingOwner { .. })) {
            lines.push(Line::from("Live outcome is not yet known. Observe or reconcile the owner result."));
        } else if matches!(view.operation, Some(Status::Finished { ok: true })) {
            lines.push(Line::from(if run.summary.command.starts_with("prepare ") {
                "Input preparation completed. No publication was requested."
            } else if run.summary.command.starts_with("build ") {
                "Build completed. No publication was requested."
            } else {
                "Apply completed without a pointer switch or service activation."
            }));
        } else {
            lines.push(Line::from("No Live pointer switch is acknowledged."));
        }
        if let Some(Status::UnknownOwner { reason, .. }) = &view.operation {
            lines.push(Line::from(format!("Owner: {reason}")));
        }
        let uploaded = run.published.iter().filter(|mutation| matches!(mutation, Publication::Uploaded { .. })).count();
        let removed = run.published.iter().filter(|mutation| matches!(mutation, Publication::Removed { .. })).count();
        if uploaded + removed > 0 {
            lines.push(Line::from(format!("Acknowledged writes: {uploaded} uploaded · {removed} removed")));
        }
        if let Some(error) = &view.observation_error {
            lines.push(Line::from(format!("Observation failed: {error}")));
        }
        if let Some(approval) = view.result.as_ref().and_then(|result| {
            result.get("approval").or_else(|| result.get("result").and_then(|result| result.get("approval")))
        }) {
            match serde_json::from_value::<crate::approval::Outcome>(approval.clone()) {
                Ok(approval) => lines.push(Line::from(approval.summary())),
                Err(_) => lines.push(Line::from("Automatic approval outcome could not be read.")),
            }
        }
        if let Some(error) = view.result.as_ref().and_then(|result| result.get("error")) {
            if let Ok(error) = serde_json::from_value::<Error>(error.clone()) {
                lines.push(Line::from(format!("Error: {}", error.message)));
                let fix = if error.fix == error.code.fix() {
                    match error.code {
                        crate::cli::Code::RunFailed => "Inspect the failed step and recent worker log in this view.",
                        crate::cli::Code::PlanOutdated => "Hide this run with Esc, then press p to review a new plan.",
                        _ => &error.fix,
                    }
                } else {
                    &error.fix
                };
                lines.push(Line::from(format!("Fix: {fix}")));
            }
        }
        for fetch in &run.fetches {
            let detail =
                fetch.error.as_deref().unwrap_or(if fetch.wall_ms.is_some() { "complete" } else { "fetching" });
            lines.push(Line::from(format!("{}@{} · {detail}", fetch.source, fetch.version)));
        }
        if let Some(phase) = run.phase {
            lines.push(Line::from(format!("Phase: {phase:?}")));
        }
        for step in &run.steps {
            let status = if step.error.is_some() {
                "failed"
            } else if step.reused {
                "reused"
            } else if step.receipt.is_some() {
                "built"
            } else {
                "running"
            };
            lines.push(Line::from(format!("{} · {status}", step.step)));
            let [duration, change, ram, output] = super::step_cells(step);
            lines.push(Line::from(format!("  {duration} · Δ {change} · peak {ram} · output {output}")));
            if let Some(error) = &step.error {
                lines.push(Line::from(error.clone()));
            }
        }
        if let Some(error) = &run.error {
            lines.push(Line::from(format!("Run error: {error}")));
        }
        if !view.logs.is_empty() {
            lines.push(Line::from("Recent worker log"));
            lines.extend(view.logs.iter().cloned().map(Line::from));
        }
        if self.execution.prepared().is_some() {
            lines.push(Line::from("Inputs are prepared. Review the new plan before build or apply."));
        }
        lines
    }
}
