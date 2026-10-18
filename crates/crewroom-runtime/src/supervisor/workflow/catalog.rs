//! Connect discovered metadata to the existing runtime's verified profiles.
use super::*;
use crate::model_catalog::{
    Access, Equivalence, Harness, discovery,
    policy::{self, Route},
    store::{Snapshot, Source},
};

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;

impl RuntimeConfig {
    pub(super) fn verify_team_admission(&self, proposal: &Value) -> Result<()> {
        if proposal["status"] == "pending" {
            for specialist in array(&proposal["specialists"]) {
                self.verify_model_preview(&specialist["role"], &specialist["model"])?;
            }
        }
        Ok(())
    }
    pub(crate) fn equivalences(&self) -> Result<Vec<Equivalence>> {
        serde_json::from_value(self.policy.get("equivalence").cloned().unwrap_or(json!([])))
            .map_err(|_| conflict("Invalid explicit model equivalence configuration."))
    }
    fn native_config(&self, harness: &Harness) -> &Value {
        match harness {
            Harness::Codex => &self.codex,
            Harness::ClaudeCode => &self.claude_code,
            Harness::Omp => &self.models,
        }
    }
    fn binary(&self, harness: &Harness) -> Option<PathBuf> {
        match harness {
            Harness::Omp => Some(PathBuf::from(&self.omp)),
            _ => self.native_config(harness)["binary"]
                .as_str()
                .map(PathBuf::from),
        }
    }
    pub(crate) fn catalog_configuration(&self, harness: &Harness) -> String {
        let config = self.native_config(harness);
        let account = if *harness == Harness::Omp {
            None
        } else {
            crate::runner::account_binding(harness.as_str(), config).ok()
        };
        let admitted: Vec<_> = self
            .catalog
            .iter()
            .filter(|v| {
                crate::runner::kind(&json!({"resolved":v})).is_ok_and(|k| k == harness.as_str())
            })
            .collect();
        policy::digest(
            &json!({"binary":self.binary(harness),"profile":config,"accountBinding":account,"admitted":admitted,"tools":self.tools}),
        )
    }
    pub(crate) fn discovery_sources(&self, helper: &Path, environment: &Value) -> Vec<Source> {
        [Harness::Omp, Harness::Codex, Harness::ClaudeCode]
            .into_iter()
            .filter_map(|harness| {
                let binary = self.binary(&harness)?;
                if binary == Path::new("/unconfigured/omp") {
                    return None;
                }
                Some(Source {
                    configuration_revision: self.catalog_configuration(&harness),
                    config: discovery::Config {
                        harness,
                        binary,
                        helper: helper.into(),
                        path: environment["PATH"]
                            .as_str()
                            .unwrap_or("/usr/bin:/bin")
                            .into(),
                        omp_models: self.models.clone(),
                    },
                })
            })
            .collect()
    }
    pub(crate) fn catalog_routes(
        &self,
        snapshot: &Snapshot,
        verify_binary: bool,
    ) -> Result<Vec<Route>> {
        let mut routes = Vec::new();
        for entry in &snapshot.harnesses {
            let current = self.catalog_configuration(&entry.harness);
            let fresh = entry.fresh(crate::model_catalog::store::now(), &current)
                && self.binary(&entry.harness).is_some_and(|b| {
                    entry.identity.as_ref().is_some_and(|i| {
                        discovery::binary_stamp(&b).is_ok_and(|s| s == i.binary_stamp)
                            && (!verify_binary
                                || discovery::binary_digest(&b).is_ok_and(|d| {
                                    d == (i.canonical_binary.clone(), i.binary_sha256.clone())
                                }))
                    })
                });
            for discovered in &entry.offerings {
                let matches: Vec<_> = self
                    .catalog
                    .iter()
                    .filter(|v| {
                        v["provider"] == discovered.provider
                            && v["id"] == discovered.selector
                            && crate::runner::kind(&json!({"resolved":v}))
                                .is_ok_and(|k| k == discovered.harness.as_str())
                    })
                    .collect();
                if matches.len() > 1 {
                    return Err(conflict(
                        "Verified model profiles contain duplicate offering identities.",
                    ));
                }
                let access = if discovered.harness == Harness::Omp {
                    if self.models["providers"][&discovered.provider].is_object() {
                        Access::Configured
                    } else {
                        Access::Unavailable
                    }
                } else if crate::runner::account_binding(
                    discovered.harness.as_str(),
                    self.native_config(&discovered.harness),
                )
                .is_ok()
                {
                    Access::Configured
                } else {
                    Access::Unavailable
                };
                routes.push(Route {
                    offering: policy::with_profile(discovered, matches.first().copied(), access),
                    advertised: discovered.capabilities.clone(),
                    fresh,
                    identity: entry.identity.clone(),
                    configuration_revision: entry.configuration_revision.clone(),
                });
            }
        }
        Ok(routes)
    }
    pub(crate) fn verify_model_preview(&self, role: &Value, model: &Value) -> Result<()> {
        if model.get("selection").is_none() {
            return Ok(());
        }
        let current = self.assignment_model(role)?;
        if current["selection"].is_null() {
            return Err(conflict(
                "Catalog selection changed to a legacy selector. Refresh the preview.",
            ));
        }
        policy::verify_preview(&current, &model["selection"]["previewRevision"])
    }
    pub(crate) fn verify_frozen_catalog_route(&self, model: &Value) -> Result<()> {
        let Some(selection) = model.get("selection") else {
            return Ok(());
        };
        if selection["schemaVersion"] != crate::model_catalog::SCHEMA_VERSION {
            return Err(conflict("Unsupported frozen model selection schema."));
        }
        let offering: crate::model_catalog::Offering =
            serde_json::from_value(selection["offering"].clone())
                .map_err(|_| conflict("Invalid frozen offering."))?;
        offering.validate()?;
        if model["resolved"]
            != offering.resolved(selection["requestedEffort"].as_str().unwrap_or(""))
        {
            return Err(conflict("Frozen route differs from its selected offering."));
        }
        let binary = self
            .binary(&offering.harness)
            .ok_or_else(|| conflict("Frozen harness is no longer configured."))?;
        let (canonical, digest) = discovery::binary_digest(&binary)?;
        if selection["identity"]["binary"] != json!(binary)
            || selection["identity"]["canonicalBinary"] != json!(canonical)
            || selection["identity"]["binarySha256"] != digest
            || selection["configurationRevision"] != self.catalog_configuration(&offering.harness)
        {
            return Err(conflict(
                "Frozen executable, account or tool profile changed. Re-admission is required.",
            ));
        }
        Ok(())
    }
}

impl<S: StoreAccess> Workflow<S> {
    pub fn refresh_model_catalog(&self) -> Result<Value> {
        self.check_open()?;
        self.0.authority.assert_current()?;
        self.0
            .config
            .discovery
            .as_ref()
            .ok_or_else(|| conflict("Model discovery is unavailable."))?
            .refresh_with_revisions(
                [Harness::Omp, Harness::Codex, Harness::ClaudeCode]
                    .into_iter()
                    .map(|h| {
                        (
                            h.as_str().to_owned(),
                            self.0.config.catalog_configuration(&h),
                        )
                    })
                    .collect(),
            )
    }
    pub fn catalog_view(&self) -> Result<Value> {
        let catalog = self
            .0
            .config
            .discovery
            .as_ref()
            .ok_or_else(|| conflict("Model discovery is unavailable."))?;
        let mut view = catalog.view(crate::model_catalog::store::now());
        let groups = self.0.config.equivalences()?;
        if let Ok(snapshot) = catalog.snapshot() {
            let routes = self.0.config.catalog_routes(&snapshot, false)?;
            match policy::choices(&routes, &groups, &self.0.config.requirements) {
                Ok(choices) => view["choices"] = choices,
                Err(_) => {
                    view["choices"] = json!([]);
                    view["choiceError"] = json!(
                        "The explicit equivalence configuration is invalid or references an unavailable offering. Restore those offerings or correct the configuration."
                    );
                }
            }
            if let Some(harnesses) = view["harnesses"].as_array_mut() {
                for entry in harnesses {
                    if let Some(saved) = snapshot
                        .harnesses
                        .iter()
                        .find(|h| json!(h.harness) == entry["harness"])
                    {
                        entry["fresh"] = json!(
                            saved.fresh(
                                crate::model_catalog::store::now(),
                                &self.0.config.catalog_configuration(&saved.harness)
                            ) && entry["fresh"] == true
                        );
                    }
                }
            }
            view["offerings"]=json!(routes.iter().map(|r|json!({"offering":r.offering,"fresh":r.fresh,"advertisedCapabilities":r.advertised,"identity":r.identity})).collect::<Vec<_>>());
        } else {
            view["offerings"] = json!([]);
            view["choices"] = json!([]);
        }
        view["equivalence"] = json!(groups);
        Ok(view)
    }
}
