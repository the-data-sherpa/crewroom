//! Live GitHub PR publication probe (FOR-46). Drives the production
//! `PullRequestHost::GitHub` client against a disposable github.com repository
//! and prints a JSON receipt. The token is read from `CREWROOM_GITHUB_TOKEN`
//! for each request and never printed.
//!
//! Usage (after pushing `<branch>` to the repository):
//! `cargo run -p crewroom-runtime --example github-publication-live -- \
//!     <owner> <name> <repository-id> <branch> <target> <head> <issue> <stage>`
//!
//! `stage` is `create` (first publication), `continue <pr-id>` (keyed
//! continuation after a new push to the branch) or `observe <pr-id>` (provider
//! state, head/base, hosted evidence and feedback). `reviews <pr-id>` (FOR-55)
//! reads feedback twice, including submitted reviews, and reports the standing
//! review decisions and the readiness policy's blockers at the PR's head.
use crewroom_runtime::{
    providers::{Credential, Credentials, Error, github_pulls::GitHubPullClient},
    publication_remote::PullRequestHost,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;

fn credentials(token: Option<String>) -> Credentials {
    Arc::new(move || {
        let token = token
            .clone()
            .or_else(|| std::env::var("CREWROOM_GITHUB_TOKEN").ok());
        Box::pin(async move {
            token
                .map(|t| Credential::Bearer(t.trim().to_owned()))
                .ok_or_else(|| Error::Invalid("CREWROOM_GITHUB_TOKEN is not set.".into()))
        })
    })
}
fn host(repository: &Value, token: Option<String>) -> PullRequestHost {
    GitHubPullClient::new(repository, credentials(token), None)
        .unwrap()
        .into()
}
fn outcome<T>(result: Result<T, Error>) -> Value {
    match result {
        Ok(_) => json!({"ok":true}),
        Err(Error::Unavailable(u)) => {
            json!({"ok":false,"status":u.status,"requestSent":u.request_sent,"message":u.message})
        }
        Err(Error::Invalid(m)) => json!({"ok":false,"invalid":m}),
    }
}
fn pr_summary(pr: &Value) -> Value {
    json!({"id":pr["id"],"state":pr["state"],"draft":pr["draft"],"head":pr["source"]["commit"]["hash"],"base":pr["destination"]["commit"]["hash"],"branch":pr["source"]["branch"]["name"],"target":pr["destination"]["branch"]["name"],"repository":pr["source"]["repository"]["uuid"],"url":pr["links"]["html"]["href"],"descriptionSha256":pr["description"].as_str().map(|d| format!("{:x}", Sha256::digest(d.as_bytes())))})
}

#[tokio::main]
async fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let [
        owner,
        name,
        id,
        branch,
        target,
        head,
        issue,
        stage,
        rest @ ..,
    ] = args.as_slice()
    else {
        eprintln!(
            "usage: <owner> <name> <id> <branch> <target> <head> <issue> <create|continue|observe> [pr-id]"
        );
        std::process::exit(2);
    };
    let repository = json!({"owner":owner,"name":name,"id":id.parse::<i64>().unwrap()});
    let github = host(&repository, None);
    let mut receipt =
        json!({"stage":stage,"observedAt":crewroom_core::timestamp(crewroom_core::now_ms())});
    receipt["verifyRepository"] = github.verify_repository().await.unwrap();
    let marker = "Crewroom publication 00000000-0000-4000-8000-000000000046";
    let description = |summary: &str| {
        format!(
            "{summary}\n\nCloses #{issue}\n\n---\n\n### Crewroom evidence\n\n- Live FOR-46 provider probe; synthetic content.\n\n{marker}"
        )
    };
    let title = format!("#{issue}: FOR-46 live publication probe");
    match stage.as_str() {
        "create" => {
            let before = github.pull_requests(branch, target).await.unwrap();
            receipt["searchBefore"] = json!(before.len());
            let pr = github
                .create_pull_request(
                    &json!({"title":title,"description":description("First publication of the probe branch."),"branch":branch,"target":target,"head":head}),
                    None,
                )
                .await
                .unwrap();
            receipt["created"] = pr_summary(&pr);
            let found = github.pull_requests(branch, target).await.unwrap();
            receipt["searchAfter"] = json!(found.iter().map(|p| json!({"id":p["id"],"hasMarker":p["description"].as_str().unwrap_or("").contains(marker)})).collect::<Vec<_>>());
            // Truthful refusals against the real API.
            receipt["rejectedToken"] = outcome(
                host(&repository, Some("ghp_synthetic0invalid".into()))
                    .verify_repository()
                    .await,
            );
            receipt["invisibleRepository"] = outcome(
                host(
                    &json!({"owner":owner,"name":"crewroom-for46-does-not-exist","id":1}),
                    None,
                )
                .verify_repository()
                .await,
            );
            receipt["changedIdentity"] = outcome(
                host(&json!({"owner":owner,"name":name,"id":1}), None)
                    .verify_repository()
                    .await,
            );
            receipt["duplicateCreate"] = outcome(
                github
                    .create_pull_request(
                        &json!({"title":title,"description":description("Duplicate attempt."),"branch":branch,"target":target,"head":head}),
                        None,
                    )
                    .await,
            );
        }
        "continue" => {
            let id = rest[0].parse::<i64>().unwrap();
            let found = github.pull_requests(branch, target).await.unwrap();
            let keyed = found
                .iter()
                .filter(|p| p["description"].as_str().unwrap_or("").contains(marker))
                .collect::<Vec<_>>();
            receipt["keyedMatches"] =
                json!(keyed.iter().map(|p| p["id"].clone()).collect::<Vec<_>>());
            let updated = github
                .update_pull_request(
                    id,
                    &json!({"title":title,"description":description("Continuation: the branch now carries a reviewed follow-up commit.")}),
                    None,
                )
                .await
                .unwrap();
            receipt["updated"] = pr_summary(&updated);
            receipt["expectedHead"] = json!(head);
        }
        "observe" => {
            let id = rest[0].parse::<i64>().unwrap();
            let pr = github.pull_request(id).await.unwrap();
            receipt["pr"] = pr_summary(&pr);
            let observed = github.observe(id).await.unwrap();
            receipt["statuses"] = observed["statuses"].clone();
            receipt["pipelines"] = observed["pipelines"].clone();
            let comments = github.comments(id).await.unwrap();
            receipt["comments"] = json!({"state":comments["state"],"count":comments["comments"].as_array().map(Vec::len)});
        }
        "reviews" => {
            // FOR-55: submitted reviews as feedback and standing decisions in
            // the hosted observation. Attribution is reported as present, not
            // by account name.
            let id = rest[0].parse::<i64>().unwrap();
            let summary = |items: &Value| {
                json!(items.as_array().unwrap().iter().map(|c| json!({"kind":c["kind"],"id":c["id"],"state":c["review"]["state"],"commit":c["review"]["commit"],"submitted":c["created_on"],"attributed":c["user"]["uuid"].is_string(),"inline":c["inline"].is_object(),"bodyChars":c["content"]["raw"].as_str().map(|s| s.chars().count())})).collect::<Vec<_>>())
            };
            let first = github.comments(id).await.unwrap();
            let second = github.comments(id).await.unwrap();
            receipt["feedback"] =
                json!({"state":first["state"],"items":summary(&first["comments"])});
            receipt["reobservedIdentical"] =
                json!(summary(&first["comments"]) == summary(&second["comments"]));
            let observed = github.observe(id).await.unwrap();
            receipt["pr"] = pr_summary(&observed["pr"]);
            receipt["decisions"] = json!(observed["reviews"].as_array().unwrap().iter().map(|r| json!({"id":r["id"],"state":r["state"],"commit":r["commit"],"attributed":r["user"]["uuid"].is_string()})).collect::<Vec<_>>());
            let head = observed["pr"]["source"]["commit"]["hash"].clone();
            let record = json!({"candidateId":"live","revision":{"head":head,"base":observed["pr"]["destination"]["commit"]["hash"]},"branch":branch,"target":target,"description":observed["pr"]["description"],
                "repository":crewroom_runtime::providers::github_pulls::repository(&repository).unwrap(),"policy":{"statusKeys":[],"pipelineSteps":[],"optionalSteps":[]},
                "push":{"state":"confirmed"},"pr":{"state":"confirmed","id":id,"providerState":"OPEN","describedSha256":observed["pr"]["description"].as_str().map(|d| format!("{:x}", Sha256::digest(d.as_bytes())))},
                "remoteSourceHead":head,"remoteTargetHead":observed["pr"]["destination"]["commit"]["hash"],"observation":observed});
            let local =
                json!({"ready":true,"candidate":{"id":"live","revision":record["revision"]}});
            receipt["readiness"] =
                crewroom_runtime::publication::assess(&record, Some(&local), false);
        }
        other => panic!("unknown stage {other}"),
    }
    println!("{}", serde_json::to_string_pretty(&receipt).unwrap());
}
