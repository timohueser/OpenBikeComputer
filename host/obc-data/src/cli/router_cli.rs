//! `obc data router TARGET`: whether the follower of an R2 target serves the planner releases of
//! its serving release and of its rollback, which a switch to either needs. It changes nothing.

use std::path::Path;

use schemars::JsonSchema;
use serde::Serialize;

use super::{print_json, print_table, Code, Error};
use crate::fetch::http::Http;
use crate::release::{self, Role};
use crate::store::Store;
use crate::target::Target;

/// What `router` writes.
#[derive(Serialize, JsonSchema)]
pub struct Router {
    pub target: String,
    /// `null` while the target serves nothing.
    pub serving: Option<Answer>,
    /// `null` without a rollback.
    pub rollback: Option<Answer>,
    /// The release that `planner/next.json` names while a run waits for it; `null` without one.
    pub next: Option<Role>,
}

/// What the follower says about the planner release of a release.
#[derive(Serialize, JsonSchema)]
pub struct Answer {
    pub release: String,
    /// Why the follower does not serve it; `null` when it does.
    pub reason: Option<String>,
}

pub fn command(root: &Path, name: &str, json: bool) -> Result<(), Error> {
    let target = crate::target::find(root, name, &Store::open()?).map_err(|e| Code::Usage.error(e))?;
    if let Target::Dir(_) = target {
        return Err(Code::Usage
            .error(format!("target `{name}` is a folder: it has no router to ask"))
            .fix(format!("`obc data serve {name}` follows a folder within a second.")));
    }
    let router = router(name, &target, &Http::new())?;
    if json {
        return print_json(&router);
    }
    let answer =
        |answer: &Answer| format!("{} {}", &answer.release[..12], answer.reason.as_deref().unwrap_or("served"));
    let mut rows = vec![vec![format!("{name} router")]];
    rows.extend(router.serving.iter().map(|serving| vec!["  serving".into(), answer(serving)]));
    rows.extend(router.rollback.iter().map(|rollback| vec!["  rollback".into(), answer(rollback)]));
    rows.extend(
        router.next.iter().map(|next| vec!["  next".into(), format!("{} since {}", &next.id[..12], next.since)]),
    );
    print_table(&rows);
    Ok(())
}

/// Ask the follower of `target` once for each release of `release.json`.
fn router(name: &str, target: &Target, http: &Http) -> Result<Router, Error> {
    let ask = |role: &Role| Answer {
        release: role.id.clone(),
        reason: crate::ready::url(target, &role.id).map_or_else(Some, |url| crate::ready::ask(http, &url)),
    };
    let serving = release::serving(target)?.map(|(serving, _)| serving);
    let next = target.get(release::NEXT)?;
    let next =
        next.map(|bytes| serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", release::NEXT))).transpose()?;
    Ok(Router {
        target: name.into(),
        serving: serving.as_ref().map(|serving| ask(&serving.serving)),
        rollback: serving.as_ref().and_then(|serving| serving.rollback.as_ref().map(ask)),
        next,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_router_gives_the_answer_of_the_follower_for_the_serving_release_and_the_rollback() {
        let fixture = crate::engine::tests::fixture("router");
        let dir = fixture.scratch.0.join("prod");
        let target = Target::Dir(dir.clone());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let starting = r#"{"state":"starting"}"#;
        let (api, asked) =
            crate::cli::update_cli::tests::follower(&runtime, &dir, &[(200, r#"{"state":"ready"}"#), (503, starting)]);
        let (a, _) = crate::serve::tests::release_at(&target, "device a", b"routing a", 1, &api);
        let (b, _) = crate::serve::tests::release_at(&target, "device b", b"routing b", 1, &api);
        for id in [&a, &b] {
            assert!(release::switch(&target, release::serving(&target).unwrap().as_ref(), id).unwrap());
        }

        let router = router("prod", &target, &Http::new()).unwrap();
        let answers = [router.serving.unwrap(), router.rollback.unwrap()].map(|answer| (answer.release, answer.reason));
        assert_eq!(answers, [(b, None), (a, Some("the planner release is starting".into()))]);
        assert_eq!((asked.lock().unwrap().len(), router.next.is_none()), (2, true));
    }
}
