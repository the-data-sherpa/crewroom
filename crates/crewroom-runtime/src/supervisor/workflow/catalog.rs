//! Connect discovered metadata to the existing runtime's verified profiles.
use super::*;
use crate::model_catalog::{
    Access, BillingRoute, Equivalence, Harness, billing, discovery,
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
    /// The logins Crewroom launches each harness with, for billing detection.
    pub(crate) fn billing_sources(&self) -> billing::Sources {
        billing::Sources {
            codex_auth: self.codex["authFile"].as_str().map(PathBuf::from),
            claude_home: self.claude_code["home"].as_str().map(PathBuf::from),
            omp_providers: self.models["providers"].clone(),
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
        let sources = self.billing_sources();
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
                // A verified profile describes a specific CLI version. Newer
                // listed versions get one only after their compatibility probe
                // passed for this exact executable (FOR-69).
                let executable = entry.identity.as_ref().is_some_and(|i| {
                    crate::model_catalog::compatibility::launchable(&entry.harness, i)
                });
                // One offering per reachable billing route; never merged.
                for route in sources
                    .detect(&discovered.harness, &discovered.provider)
                    .routes
                {
                    routes.push(Route {
                        offering: policy::with_profile(
                            &discovered.with_billing(route),
                            matches.first().copied().filter(|_| executable),
                            access.clone(),
                        ),
                        advertised: discovered.capabilities.clone(),
                        fresh,
                        identity: entry.identity.clone(),
                        configuration_revision: entry.configuration_revision.clone(),
                    });
                }
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
    /// FOR-69: a frozen catalog selection whose harness executable changed
    /// after admission is re-probed before launch. A failed re-probe, or a
    /// new executable that no longer lists the admitted offering, holds the
    /// launch with the reason. A passing re-probe returns the selection with
    /// the new executable identity and the receipt to record on the attempt;
    /// route, offering and effort stay as admitted.
    pub(crate) async fn hold_changed_executable(
        &self,
        model: &Value,
    ) -> Result<Option<(Value, Value)>> {
        let Some(selection) = model.get("selection") else {
            return Ok(None);
        };
        let Ok(offering) =
            serde_json::from_value::<crate::model_catalog::Offering>(selection["offering"].clone())
        else {
            // The frozen-route check refuses an invalid selection.
            return Ok(None);
        };
        let Some(binary) = self.binary(&offering.harness) else {
            return Ok(None);
        };
        let current = tokio::task::spawn_blocking(move || discovery::binary_digest(&binary))
            .await
            .map_err(|_| conflict("Harness executable hashing did not finish."))?;
        if current.as_ref().is_ok_and(|(canonical, digest)| {
            selection["identity"]["canonicalBinary"] == json!(canonical)
                && selection["identity"]["binarySha256"] == json!(digest)
        }) {
            return Ok(None);
        }
        let harness = &offering.harness;
        let label = harness.label();
        let admitted = selection["identity"]["version"]
            .as_str()
            .unwrap_or("unknown");
        let Some(catalog) = self.discovery.as_ref() else {
            return Err(conflict(&format!(
                "Launch held: the {label} executable changed after admission ({admitted}), and model discovery is unavailable to re-probe it."
            )));
        };
        let receipt = catalog.probe_now(harness).await.map_err(|reason| {
            conflict(&format!(
                "Launch held: the {label} executable changed after admission ({admitted}) and its re-probe failed. {reason}"
            ))
        })?;
        let identity = receipt.identity;
        let compatibility = identity
            .compatibility
            .as_ref()
            .filter(|_| crate::model_catalog::compatibility::launchable(harness, &identity))
            .map(|c| c.label(harness, &identity.version))
            .ok_or_else(|| {
                conflict(&format!(
                    "Launch held: the {label} executable changed after admission ({admitted}) and its re-probe produced no compatibility receipt. Refresh models, then re-admit the assignment."
                ))
            })?;
        if !receipt
            .offerings
            .iter()
            .any(|o| o.legacy_key() == offering.legacy_key())
        {
            return Err(conflict(&format!(
                "Launch held: the {label} executable changed after admission ({admitted}). The new executable reports {} and is {compatibility}, but no longer lists {}. Refresh models, then re-admit the assignment.",
                identity.version, offering.display_name
            )));
        }
        let record = json!({
            "admittedVersion": selection["identity"]["version"],
            "admittedBinarySha256": selection["identity"]["binarySha256"],
            "version": identity.version,
            "binarySha256": identity.binary_sha256,
            "compatibility": compatibility,
        });
        let mut next = model.clone();
        next["selection"]["identity"] = json!(identity);
        Ok(Some((next, record)))
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
        // The admitted billing route is part of the frozen identity. A login
        // that no longer offers it never falls back to another route.
        if let Some(admitted) = offering.billing {
            let current = self
                .billing_sources()
                .detect(&offering.harness, &offering.provider);
            if !current.routes.contains(&admitted) {
                return Err(conflict(&format!(
                    "Admitted billing route ({}) is no longer available from the {} login (now {}). Re-admission is required.",
                    admitted.label(),
                    offering.harness.label(),
                    current
                        .routes
                        .iter()
                        .map(BillingRoute::label)
                        .collect::<Vec<_>>()
                        .join(" and ")
                )));
            }
        }
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
    /// FOR-69 launch-time re-probe; see `RuntimeConfig::hold_changed_executable`.
    /// A passing re-probe of a changed executable is recorded on the attempt
    /// (new identity and receipt) and the updated attempt is returned.
    pub(super) async fn hold_changed_executable(
        &self,
        attempt: &Value,
        assignment: &Value,
    ) -> Result<Value> {
        let model = attempt
            .get("model")
            .filter(|v| !v.is_null())
            .unwrap_or(&assignment["model"]);
        match self.0.config.hold_changed_executable(model).await? {
            None => Ok(attempt.clone()),
            Some((model, receipt)) => {
                let id = text(attempt, "id")?;
                self.store(|s| s.record_attempt_reprobe(id, &model, &receipt))
            }
        }
    }
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
            let sources = self.0.config.billing_sources();
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
                        entry["executionNote"] = json!(saved.execution_note());
                        entry["compatibility"] = json!(saved.compatibility());
                        if entry["configured"] == true {
                            entry["billing"] = sources.summary(&saved.harness);
                        }
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
