use super::*;
use crate::{providers::github_pulls, repository::request};
use crewroom_core::ticket_source;
use std::collections::HashSet;
fn continues(s: &Store, run: &str, candidate: &str, published: &Value) -> Result<bool> {
    let coding = s.coding_record(run)?.unwrap_or(Value::Null);
    let value = s.get_run(run)?;
    let candidates = array(&coding["candidates"]);
    let continuations = array(&value["feedbackContinuations"]);
    let mut visited = HashSet::new();
    let mut current = candidates.iter().find(|c| c["id"] == candidate);
    while let Some(c) = current {
        if !visited.insert(c["id"].as_str().unwrap_or("")) {
            return Ok(false);
        }
        let feedback = continuations
            .iter()
            .find(|f| f["id"] == c["feedbackFrom"]["continuationId"]);
        if present(&c["feedbackFrom"])
            && feedback.is_none_or(|f| {
                f["state"] != "candidate"
                    || !f["candidateId"]
                        .as_str()
                        .is_some_and(|id| visited.contains(id))
                    || f["publicationId"] != published["id"]
                    || f["prId"] != published["pr"]["id"]
                    || f["repository"]["uuid"] != published["repository"]["uuid"]
                    || f["branch"] != published["branch"]
            })
        {
            return Ok(false);
        }
        let parent = c
            .get("refreshedFrom")
            .unwrap_or(&c["feedbackFrom"]["sourceCandidateId"]);
        if !present(parent) {
            return Ok(false);
        }
        let previous = candidates.iter().find(|c| c["id"] == *parent);
        if let Some(f) = feedback {
            let p = previous.unwrap_or(&Value::Null);
            if (parent == &f["sourceCandidateId"] && p["revision"]["head"] != f["sourceHead"])
                || (parent != &f["sourceCandidateId"]
                    && p["feedbackFrom"]["continuationId"] != f["id"])
            {
                return Ok(false);
            }
        }
        if parent == &published["candidateId"] {
            return Ok(true);
        }
        current = previous;
    }
    Ok(false)
}
fn authorized(s: &Store, attempt: &str, candidate: &str) -> Result<(Value, Option<Value>)> {
    let a = s.get_attempt(attempt)?;
    let run = s.get_run(text(&a, "runId")?)?;
    let id = text(&run, "id")?;
    if !array(&s.runtime(id)?["assignments"]).iter().any(|a| {
        a["coordinator"] == true && !present(&a["retiredAt"]) && a["currentAttemptId"] == attempt
    }) || a["ownership"] != "held"
        || run["status"] != "running"
        || run["desiredState"] != "running"
        || present(&run["sourceConflict"])
    {
        return Err(conflict(
            "Only the active coordinator may request publication.",
        ));
    }
    let own = s.publication(id)?;
    let inherited = if own.is_none() {
        s.continuation_publication(id)?
    } else {
        None
    };
    let inherited_present = inherited.is_some();
    let previous = own.or(inherited);
    if let Some(p) = &previous {
        if present(&p["merged"]) {
            return Err(conflict(
                "This PR is merged; delivery is complete. New work needs a new run.",
            ));
        }
        if p["candidateId"] != candidate {
            if !inherited_present && !continues(s, id, candidate, p)? {
                return Err(conflict(
                    "Publication already belongs to another candidate. Explicit review continuation is required.",
                ));
            }
            if p["pr"]["state"] == "confirmed" && p["pr"]["providerState"] != "OPEN" {
                return Err(conflict(
                    "The existing PR is not open. No successor can be published.",
                ));
            }
            if ["push", "pr"]
                .iter()
                .any(|key| !matches!(p[key]["state"].as_str(), Some("pending" | "confirmed")))
            {
                return Err(conflict(
                    "Reconcile the publication receipts before publishing the successor candidate.",
                ));
            }
        }
    }
    Ok((run, previous))
}
impl<S: StoreAccess> Publication<S> {
    pub async fn prepare(&self, attempt: &str, input: &Value) -> Result<Value> {
        request(input, &["candidateId", "summary"])?;
        let candidate = text(input, "candidateId")?.to_owned();
        let summary = crewroom_core::store::trim_text(text(input, "summary")?).to_owned();
        if !crewroom_core::store::valid_uuid(&candidate)
            || !(40..=20000).contains(&summary.chars().count())
        {
            return Err(Error::Invalid(
                "Publication requires a candidate UUID and a summary of 40 to 20000 characters."
                    .into(),
            ));
        }
        let run = self.flight.with_open(|| {
            self.remote
                .store(|s| Ok(text(&authorized(s, attempt, &candidate)?.0, "id")?.to_owned()))
        })?;
        let attempt = attempt.to_owned();
        let this = self.clone();
        self.remote.operation(&run.clone(),move |ticket|async move {
            let (first,proof)=ticket.with_current(||this.flight.with_open(||this.remote.store(|s|Ok((authorized(s,&attempt,&candidate)?,Proof::capture(s,&run)?)))))?;
            if first.1.as_ref().is_some_and(|p|p["candidateId"]==candidate) {
                let _lock=this.mutation.lock().await;
                let record=ticket.with_current(||this.flight.with_open(||this.remote.store(|s| {
                    let (_,previous)=authorized(s,&attempt,&candidate)?;
                    let mut record=previous.filter(|p|p["candidateId"]==candidate).ok_or_else(||conflict("Publication changed while revising its description. Inspect publication_status."))?;
                    if record["summary"]!=summary {
                        if matches!(record["pr"]["state"].as_str(),Some("sending"|"uncertain")) {return Err(conflict("Reconcile the PR receipt before revising its description."));}
                        record["summary"]=json!(summary); record["description"]=json!(describe(s,&record)?); record["ready"]=json!(false); record["blockers"]=json!(["The PR description update has not been confirmed."]); s.save_publication(&record)?;
                    }
                    Ok(record)
                })))?;
                drop(_lock);
                return Ok(this.read(&run).await?.unwrap_or(record));
            }
            let owned=this.work.status(&run).await?; let local=this.evidence.assess(&run).await?;
            let _lock=this.mutation.lock().await;
            ticket.with_current(||this.flight.with_open(||this.remote.store(|s| {
                let (run,previous)=authorized(s,&attempt,&candidate)?;
                if let Some(previous)=&previous && previous["candidateId"]==candidate && previous["summary"]==summary {return Ok(previous.clone());}
                proof.check(s)?;
                if local["ready"]!=true || local["candidate"]["id"]!=candidate || local["candidate"]["revision"]!=owned["revision"] || present(&owned["writer"]) || owned["revision"]["head"]==owned["revision"]["base"] {return Err(conflict("Publication requires a changed, clean and locally validated current candidate."));}
                if previous.as_ref().is_some_and(|p|owned["branch"]!=p["branch"] || owned["repository"]["path"]!=p["repositoryPath"] || owned["repository"]["targetBranch"]!=p["target"]) {return Err(conflict("The owned branch or target repository changed. The existing PR cannot be replaced."));}
                let path=text(&owned["repository"],"path")?;
                let connection=this.remote.connection(path).ok_or_else(||conflict(&this.remote.unavailable(path).map_or_else(||"Bitbucket publication is not configured for this repository.".into(),|u|u.reason)))?;
                if owned["branch"]==owned["repository"]["targetBranch"] {return Err(conflict("Publication cannot push the target branch."));}
                let mut record=if let Some(mut previous)=previous {
                    if this.remote.matched(&previous).is_none() {return Err(conflict(&crate::publication_remote::unmatched(&previous)));}
                    let pushed=if previous["push"]["state"]=="confirmed" {Some(previous["revision"]["head"].clone())} else {None};
                    let mut entry=json!({"candidateId":previous["candidateId"],"revision":previous["revision"],"supersededAt":time()});
                    if let Some(pushed)=&pushed {entry["pushedHead"]=pushed.clone();}
                    let mut history=array(&previous["history"]).to_vec();history.push(entry);previous["history"]=json!(history);
                    previous["runId"]=run["id"].clone();previous["attemptId"]=json!(attempt);previous["candidateId"]=json!(candidate);previous["summary"]=json!(summary);previous["revision"]=local["candidate"]["revision"].clone();
                    previous["push"]=json!({"state":"pending","expectedHead":pushed.unwrap_or_else(||previous["push"]["expectedHead"].clone())});
                    remove(&mut previous,"observation");remove(&mut previous,"error"); previous["ready"]=json!(false);previous["blockers"]=json!(["Publication has not been confirmed."]);previous
                } else {
                    json!({"id":uuid::Uuid::new_v4().to_string(),"runId":run["id"],"attemptId":attempt,"candidateId":candidate,"repositoryPath":owned["repository"]["path"],"repository":connection.repository,"branch":owned["branch"],"target":owned["repository"]["targetBranch"],"revision":local["candidate"]["revision"],"title":format!("{}: {}",text(&run["taskSnapshot"],"key")?,text(&run["taskSnapshot"],"title")?),"summary":summary,"description":"","policy":connection.policy,"createdAt":time(),"push":{"state":"pending","expectedHead":null},"pr":{"state":"pending"},"ready":false,"blockers":["Publication has not been confirmed."]})
                };
                record["description"]=json!(describe(s,&record)?);s.save_publication(&record)
            })))
        }).await
    }
}
fn describe(s: &Store, record: &Value) -> Result<String> {
    let id = text(record, "runId")?;
    let run = s.get_run(id)?;
    let coding = s
        .coding_record(id)?
        .ok_or_else(|| conflict("Coding evidence is unavailable."))?;
    let mut lineage = HashSet::new();
    let mut current = array(&coding["candidates"])
        .iter()
        .find(|c| c["id"] == record["candidateId"]);
    while let Some(c) = current {
        if !lineage.insert(text(c, "id")?) {
            return Err(conflict("Candidate refresh lineage contains a cycle."));
        }
        current = c.get("refreshedFrom").and_then(|parent| {
            array(&coding["candidates"])
                .iter()
                .find(|c| c["id"] == *parent)
        });
    }
    let github = github_pulls::is_github(&record["repository"]);
    let task = &run["taskSnapshot"];
    let mut lines = vec![
        record
            .get("summary")
            .unwrap_or(&task["brief"])
            .as_str()
            .unwrap_or("")
            .to_owned(),
        "".into(),
    ];
    if let Some(closes) = closing_reference(task, &record["repository"]) {
        lines.extend([closes, "".into()]);
    }
    lines.extend([
        "---".into(),
        "".into(),
        "### Crewroom evidence".into(),
        "".into(),
    ]);
    if let Some(url) = task["sourceUrl"].as_str().filter(|s| !s.is_empty()) {
        let label = if ticket_source::provider(task) == Some(ticket_source::GITHUB) {
            "GitHub issue"
        } else {
            "Jira"
        };
        lines.extend([format!("{label}: {url}"), "".into()]);
    }
    lines.push("Acceptance criteria:".into());
    lines.extend(
        array(&run["taskSnapshot"]["criteria"])
            .iter()
            .map(|c| format!("- {}", c.as_str().unwrap_or(""))),
    );
    lines.push("".into());
    lines.push(format!(
        "- Candidate commit `{}` on base `{}` (candidate {})",
        text(&record["revision"], "head")?,
        text(&record["revision"], "base")?,
        text(record, "candidateId")?
    ));
    if let Some(work) = s.repository_work(id)? {
        for refresh in array(&work["refreshes"]).iter().filter(|r| {
            r["state"] == "completed"
                && r["candidateId"]
                    .as_str()
                    .is_some_and(|c| lineage.contains(c))
        }) {
            lines.push(format!(
                "- Rebased onto the target: `{}` → `{}`",
                text(refresh, "fromBase")?
                    .chars()
                    .take(12)
                    .collect::<String>(),
                text(refresh, "toBase")?
                    .chars()
                    .take(12)
                    .collect::<String>()
            ));
        }
    }
    lines.push(
        "- Required local checks, run in isolated containers against this exact commit:".into(),
    );
    for check in array(&coding["requiredChecks"])
        .iter()
        .filter(|c| c["required"] == true)
    {
        let receipt = array(&coding["checks"])
            .iter()
            .rev()
            .find(|c| c["candidateId"] == record["candidateId"] && c["checkId"] == check["id"])
            .ok_or_else(|| conflict("The publication's required check receipt is unavailable."))?;
        lines.push(format!(
            "  - {}: passed (output SHA-256 `{}`)",
            text(check, "id")?,
            text(&receipt["output"], "sha256")?
        ));
    }
    let hosted = if github {
        "- Hosted checks are this PR's commit statuses and check runs. Crewroom reports the PR ready only when they pass for this head and base."
    } else {
        "- Hosted checks are this PR's build statuses and pipeline. Crewroom reports the PR ready only when they pass for this head and base."
    };
    lines.extend([
        "- Independent review passed for this commit.".into(),
        hosted.into(),
        "- Merge is the operator's decision.".into(),
        "".into(),
        marker(record),
    ]);
    Ok(lines.join("\n"))
}

/// `owner/name` and issue number of a github.com issue URL.
fn github_issue(url: &str) -> Option<(String, String, u64)> {
    let rest = url.strip_prefix("https://github.com/")?;
    let parts = rest.trim_end_matches('/').split('/').collect::<Vec<_>>();
    match parts.as_slice() {
        [owner, name, "issues", number]
            if !owner.is_empty()
                && !name.is_empty()
                && !number.is_empty()
                && number.bytes().all(|b| b.is_ascii_digit()) =>
        {
            Some(((*owner).into(), (*name).into(), number.parse().ok()?))
        }
        _ => None,
    }
}

/// The closing keyword for a GitHub-sourced ticket whose PR is published to
/// GitHub: `Closes #N` in the same repository, `Closes owner/name#N` across
/// repositories. Jira, local and Bitbucket publications get none.
pub(super) fn closing_reference(task: &Value, repository: &Value) -> Option<String> {
    if ticket_source::provider(task) != Some(ticket_source::GITHUB)
        || !github_pulls::is_github(repository)
    {
        return None;
    }
    let (owner, name, number) =
        task["sourceUrl"]
            .as_str()
            .and_then(github_issue)
            .or_else(|| {
                // The issue key is `#N`, scoped to the source repository site.
                let number = task["key"].as_str()?.strip_prefix('#')?.parse().ok()?;
                let site = task["sourceSite"].as_str()?.trim_end_matches('/');
                let (owner, name) = site.strip_prefix("https://github.com/")?.split_once('/')?;
                (!name.contains('/')).then(|| (owner.to_owned(), name.to_owned(), number))
            })?;
    let same = repository["owner"]
        .as_str()
        .is_some_and(|o| o.eq_ignore_ascii_case(&owner))
        && repository["name"]
            .as_str()
            .is_some_and(|n| n.eq_ignore_ascii_case(&name));
    Some(if same {
        format!("Closes #{number}")
    } else {
        format!("Closes {owner}/{name}#{number}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn github_repository() -> Value {
        json!({"provider":"github","owner":"octo-fixture","name":"sandbox","id":42,"uuid":"github:42"})
    }

    #[test]
    fn github_sourced_tickets_close_their_issue_on_github_publications_only() {
        let issue = json!({"source":"github","key":"#12","sourceSite":"https://github.com/octo-fixture/sandbox","sourceUrl":"https://github.com/octo-fixture/sandbox/issues/12"});
        assert_eq!(
            closing_reference(&issue, &github_repository()).as_deref(),
            Some("Closes #12")
        );
        // Another repository's issue is named in full.
        let other = json!({"source":"github","key":"#7","sourceUrl":"https://github.com/octo-fixture/tracker/issues/7"});
        assert_eq!(
            closing_reference(&other, &github_repository()).as_deref(),
            Some("Closes octo-fixture/tracker#7")
        );
        // Without a URL, the `#N` key is scoped to its source repository site.
        let keyed = json!({"source":"github","key":"#3","sourceSite":"https://github.com/Octo-Fixture/Sandbox"});
        assert_eq!(
            closing_reference(&keyed, &github_repository()).as_deref(),
            Some("Closes #3")
        );
        // A pull request URL is not an issue to close.
        let pull = json!({"source":"github","key":"#4","sourceUrl":"https://github.com/octo-fixture/sandbox/pull/4"});
        assert_eq!(closing_reference(&pull, &github_repository()), None);
        // Jira and local tickets keep the existing description.
        let jira = json!({"source":"jira","key":"FIX-1","sourceUrl":"https://fixture.atlassian.net/browse/FIX-1"});
        assert_eq!(closing_reference(&jira, &github_repository()), None);
        assert_eq!(
            closing_reference(&json!({"key":"LOCAL-1"}), &github_repository()),
            None
        );
        // A Bitbucket publication cannot close a GitHub issue.
        let bitbucket = json!({"workspace":"fixture","slug":"repository","uuid":"{00000000-0000-4000-8000-000000000011}"});
        assert_eq!(closing_reference(&issue, &bitbucket), None);
    }
}
