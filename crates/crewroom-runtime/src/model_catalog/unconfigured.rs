//! Metadata discovery while no runner is configured. Installed harnesses on the
//! service PATH list their models so the operator can record model intent.
//! Nothing here grants access, verifies an execution profile or admits work;
//! the configured runner re-checks every saved selection before admission.
use super::{
    billing, discovery,
    policy::{self, Route},
    store::{Catalog, Source},
    *,
};
use std::path::Path;

/// Executable names searched on the service PATH, in catalog order.
pub const EXECUTABLES: [(Harness, &str); 3] = [
    (Harness::Omp, "omp"),
    (Harness::Codex, "codex"),
    (Harness::ClaudeCode, "claude"),
];

pub const REASON: &str =
    "Agent runner is not configured, so this service cannot start agents or confirm model access.";
pub const REMEDY: &str = "Open Settings → Runner to see what the runner needs and save its configuration (headless services can set CREWROOM_RUNTIME_CONFIG instead). Until then you can save model choices; they are checked when the runner is configured.";

fn on_path(path: &str, name: &str) -> Option<std::path::PathBuf> {
    std::env::split_paths(path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|candidate| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(candidate)
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// Sources for every installed harness. OMP lists configured providers only,
/// so without runner provider definitions it reports no models.
pub fn sources(helper: &Path, path: &str) -> Vec<Source> {
    EXECUTABLES
        .iter()
        .filter_map(|(harness, name)| {
            let binary = on_path(path, name)?;
            Some(Source {
                configuration_revision: policy::digest(
                    &json!({"mode":"discovery-only","binary":binary}),
                ),
                config: discovery::Config {
                    harness: harness.clone(),
                    binary,
                    helper: helper.into(),
                    path: path.into(),
                    omp_models: json!({"providers":{}}),
                },
            })
        })
        .collect()
}

fn executable(harness: &Harness) -> &'static str {
    EXECUTABLES
        .iter()
        .find(|(h, _)| h == harness)
        .map(|(_, name)| *name)
        .unwrap_or("")
}

/// The workspace projection: the shared catalog view plus choices whose
/// eligibility is computed by the admission predicate. Every choice remains
/// ineligible because access and execution profiles are unverified; the
/// `selection` field tells clients that saving records intent only.
/// `billing` names the native logins found in the service environment; they
/// are what a runner configured from the defaults would launch with.
pub fn view(catalog: &Catalog, billing: &billing::Sources, now: u64) -> Value {
    let mut view = catalog.view(now);
    view["runner"] = json!({"configured":false,"reason":REASON,"remedy":REMEDY});
    view["selection"] = json!("intent");
    view["equivalence"] = json!([]);
    let fresh: std::collections::HashMap<String, bool> = view["harnesses"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|h| {
            (
                h["harness"].as_str().unwrap_or("").to_owned(),
                h["fresh"] == true,
            )
        })
        .collect();
    match catalog.snapshot() {
        Ok(snapshot) => {
            let mut routes: Vec<Route> = Vec::new();
            for entry in &snapshot.harnesses {
                let record = entry.identity.as_ref().and_then(|i| {
                    capabilities::HarnessCapabilities::for_identity(&entry.harness, i)
                });
                let is_fresh = fresh.get(entry.harness.as_str()).copied().unwrap_or(false);
                for discovered in &entry.offerings {
                    // Bundled facts fill capabilities discovery left unknown.
                    let discovered = bundled::merge(discovered);
                    for route in billing
                        .detect(&discovered.harness, &discovered.provider)
                        .routes
                    {
                        let mut item = Route::new(
                            policy::with_profile(
                                &discovered.with_billing(route),
                                None,
                                Access::Unknown,
                            ),
                            discovered.capabilities.clone(),
                            is_fresh,
                            entry.identity.clone(),
                            entry.configuration_revision.clone(),
                        );
                        item.harness_capabilities = record.clone();
                        routes.push(item);
                    }
                }
                // FOR-73: the operator's OMP login. Without a runner no
                // provider is gateway-configured, so each states its reason.
                if let Some(listing) = &entry.login {
                    for (discovered, blocker) in login::unlaunchable(listing, &json!({})) {
                        let discovered = bundled::merge(discovered);
                        let mut item = Route::new(
                            policy::with_profile(
                                &discovered.with_billing(BillingRoute::Unknown),
                                None,
                                Access::Unavailable,
                            ),
                            discovered.capabilities.clone(),
                            is_fresh,
                            entry.identity.clone(),
                            entry.configuration_revision.clone(),
                        );
                        item.harness_capabilities = record.clone();
                        item.launch_blocker = Some(blocker);
                        if !routes.iter().any(|r| r.offering.key == item.offering.key) {
                            routes.push(item);
                        }
                    }
                }
            }
            let requirements = json!({"inputs":["text"],"tools":true});
            match policy::choices(&routes, &[], &requirements) {
                Ok(choices) => view["choices"] = choices,
                Err(_) => {
                    view["choices"] = json!([]);
                    view["choiceError"] = json!(
                        "Discovered offerings are inconsistent. Refresh models to replace them."
                    );
                }
            }
            view["offerings"] = json!(routes.iter().map(|r| json!({"offering":r.offering,"fresh":r.fresh,"advertisedCapabilities":r.advertised,"identity":r.identity})).collect::<Vec<_>>());
            for entry in view["harnesses"].as_array_mut().into_iter().flatten() {
                let Some(saved) = snapshot
                    .harnesses
                    .iter()
                    .find(|h| json!(h.harness) == entry["harness"])
                else {
                    continue;
                };
                entry["executionNote"] = json!(saved.execution_note());
                entry["compatibility"] = json!(saved.compatibility());
                if catalog.source(&saved.harness).is_none() {
                    entry["remedy"] = json!(format!(
                        "{} (`{}`) was not found on the service PATH. Install it or add its directory to the service PATH, then restart the service.",
                        saved.harness.label(),
                        executable(&saved.harness)
                    ));
                } else {
                    entry["billing"] = billing.summary(&saved.harness);
                }
                if catalog.source(&saved.harness).is_some() && saved.harness == Harness::Omp {
                    entry["note"] = json!(if saved.login.is_some() {
                        "Runner-configured OMP providers are listed once the runner is configured. Models from your OMP login are listed with the reason each cannot launch."
                    } else {
                        "OMP lists models only from providers defined in the runner configuration. Configure the runner to list OMP models."
                    });
                }
                harness_facts(entry, saved, &json!({}));
            }
        }
        Err(error) => {
            view["choices"] = json!([]);
            view["offerings"] = json!([]);
            view["choiceError"] = json!(error.to_string());
        }
    }
    view
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn only_executable_harnesses_on_absolute_path_entries_become_sources() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        for (name, mode) in [("codex", 0o700), ("claude", 0o600)] {
            std::fs::write(bin.join(name), "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(bin.join(name), std::fs::Permissions::from_mode(mode))
                .unwrap();
        }
        let path = format!("relative:{}", bin.display());
        let found = sources(Path::new("/fixture/process"), &path);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].config.harness, Harness::Codex);
        assert_eq!(found[0].config.path, path);
        assert_eq!(found[0].config.omp_models, json!({"providers":{}}));
    }

    #[test]
    fn view_names_missing_harnesses_and_the_runner_remedy() {
        let root = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(&root.path().join("catalog"), vec![], || {}).unwrap();
        let view = view(&catalog, &billing::Sources::default(), 100);
        assert_eq!(view["runner"]["configured"], false);
        assert!(
            view["runner"]["remedy"]
                .as_str()
                .unwrap()
                .contains("CREWROOM_RUNTIME_CONFIG")
        );
        assert_eq!(view["selection"], "intent");
        assert_eq!(view["choices"], json!([]));
        let harnesses = view["harnesses"].as_array().unwrap();
        assert_eq!(harnesses.len(), 3);
        for h in harnesses {
            assert_eq!(h["configured"], false);
            assert!(
                h["remedy"]
                    .as_str()
                    .unwrap()
                    .contains("not found on the service PATH")
            );
        }
        assert!(
            harnesses[2]["remedy"]
                .as_str()
                .unwrap()
                .contains("`claude`")
        );
    }

    #[test]
    fn omp_login_models_are_listed_with_their_reason_and_no_credentials() {
        use std::os::unix::fs::DirBuilderExt;
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("catalog");
        std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        // Discovery keeps identity and capability fields only; a
        // credential-shaped field in the harness output never survives.
        let rows = json!([
            {"provider":"openai-codex","id":"gpt-6-astra","selector":"openai-codex/gpt-6-astra","name":"GPT-6 Astra",
             "thinking":["low","high"],"input":["text"],"apiKey":"login-secret-canary"},
            {"provider":"zai","id":"glm-5","selector":"zai/glm-5","name":"GLM-5","thinking":["high"],"input":["text"]}
        ]);
        let mut snapshot = super::super::store::Snapshot::default();
        snapshot.harnesses[0].login = Some(login::Listing {
            outcome: super::super::store::Outcome::Success,
            observed_at: 1,
            offerings: discovery::normalize(&Harness::Omp, &rows, 1).unwrap(),
            error: None,
        });
        crewroom_core::authority::write_private(
            &dir.join("catalog.json"),
            &serde_json::to_vec(&snapshot).unwrap(),
        )
        .unwrap();
        let catalog = Catalog::open(&dir, vec![], || {}).unwrap();
        let view = view(&catalog, &billing::Sources::default(), 100);
        assert!(!view.to_string().contains("login-secret-canary"));
        let choices = view["choices"].as_array().unwrap();
        assert_eq!(choices.len(), 2);
        for choice in choices {
            assert_eq!(choice["source"], "omp-login");
            let blockers = choice["eligibility"]["high"]["blockers"]
                .as_array()
                .unwrap();
            assert_eq!(blockers.len(), 1);
            assert!(
                blockers[0]
                    .as_str()
                    .unwrap()
                    .contains("only API-key providers")
            );
        }
        assert!(
            choices[0]["eligibility"]["high"]["blockers"][0]
                .as_str()
                .unwrap()
                .contains("subscription sign-in")
        );
        // Bundled facts fill the unreported tool capability.
        assert_eq!(choices[0]["facts"], "bundled");
        assert_eq!(choices[0]["unknownCapabilities"], json!([]));
        assert_eq!(choices[1]["facts"], "live-only");
        assert_eq!(choices[1]["unknownCapabilities"], json!(["tools"]));
        let omp = &view["harnesses"][0];
        assert_eq!(omp["login"]["models"], 2);
        assert_eq!(omp["login"]["launchable"], 0);
        assert!(
            omp["login"]["offerings"].is_null(),
            "raw listing stays server-side"
        );
    }

    #[test]
    fn a_login_listing_is_omp_only() {
        let mut snapshot = super::super::store::Snapshot::default();
        snapshot.harnesses[1].login = Some(login::Listing {
            outcome: super::super::store::Outcome::Success,
            observed_at: 1,
            offerings: vec![],
            error: None,
        });
        use std::os::unix::fs::DirBuilderExt;
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("catalog");
        std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        crewroom_core::authority::write_private(
            &dir.join("catalog.json"),
            &serde_json::to_vec(&snapshot).unwrap(),
        )
        .unwrap();
        assert!(Catalog::open(&dir, vec![], || {}).is_err());
    }
}
