//! Metadata discovery while no runner is configured. Installed harnesses on the
//! service PATH list their models so the operator can record model intent.
//! Nothing here grants access, verifies an execution profile or admits work;
//! the configured runner re-checks every saved selection before admission.
use super::{
    discovery,
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
pub const REMEDY: &str = "Create the private runtime configuration, set CREWROOM_RUNTIME_CONFIG for the service, then stop and restart the service. Until then you can save model choices; they are checked when the runner is configured.";

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
pub fn view(catalog: &Catalog, now: u64) -> Value {
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
            let routes: Vec<Route> = snapshot
                .harnesses
                .iter()
                .flat_map(|entry| {
                    entry.offerings.iter().map(|discovered| Route {
                        offering: policy::with_profile(discovered, None, Access::Unknown),
                        advertised: discovered.capabilities.clone(),
                        fresh: fresh.get(entry.harness.as_str()).copied().unwrap_or(false),
                        identity: entry.identity.clone(),
                        configuration_revision: entry.configuration_revision.clone(),
                    })
                })
                .collect();
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
                if catalog.source(&saved.harness).is_none() {
                    entry["remedy"] = json!(format!(
                        "{} (`{}`) was not found on the service PATH. Install it or add its directory to the service PATH, then restart the service.",
                        saved.harness.label(),
                        executable(&saved.harness)
                    ));
                } else if saved.harness == Harness::Omp {
                    entry["note"] = json!(
                        "OMP lists models only from providers defined in the runner configuration. Configure the runner to list OMP models."
                    );
                }
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
        let view = view(&catalog, 100);
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
}
