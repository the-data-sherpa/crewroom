use super::*;
use crewroom_core::{playbooks::Playbooks, principles::Principles};

impl<S: StoreAccess> Workflow<S> {
    fn evaluation_catalog(&self, kind: &str) -> Result<Vec<Value>> {
        match kind {
            "role" => self.0.roles.read(),
            "principle" => Principles::beside_roles(self.0.roles.path()).read(),
            "playbook" => Playbooks::beside_roles(&self.0.roles).read(),
            _ => Err(conflict("Unsupported instruction kind.")),
        }
    }
    fn evaluation_identity(&self, model: &Value) -> Result<Value> {
        let kind = crate::runner::kind(model)?;
        let binary = match kind {
            "codex" => text(&self.0.config.codex, "binary")?,
            "claude-code" => text(&self.0.config.claude_code, "binary")?,
            "omp" => &self.0.config.omp,
            _ => return Err(conflict("Unsupported frozen runner.")),
        };
        let path = std::fs::canonicalize(binary)?;
        let application = std::env::current_exe()?;
        let helpers = self.0.hosts.execution_identity()?;
        let prefix_executable = self
            .0
            .config
            .prefix
            .first()
            .map(|path| crate::supervisor::hosts::executable_identity(Path::new(path)))
            .transpose()?;
        let native = match kind {
            "codex" => &self.0.config.codex,
            "claude-code" => &self.0.config.claude_code,
            _ => &Value::Null,
        };
        let account = if native.is_null() {
            Value::Null
        } else {
            json!(crate::runner::account_binding(kind, native)?)
        };
        let mut provider = self.0.config.models["providers"]
            [model["resolved"]["provider"].as_str().unwrap_or("")]
        .clone();
        remove(
            &mut provider,
            &["apiKey", "headers", "authorization", "token"],
        );
        let profile = json!({"kind":kind,"native":native,"providerRouting":provider,"launchPrefix":self.0.config.prefix,"path":self.0.environment["PATH"],"inspector":self.0.environment["CREWROOM_PROCESS_INSPECTOR"],"accountBinding":account});
        let discovered = self
            .0
            .config
            .discovery
            .as_ref()
            .and_then(|c| c.snapshot().ok())
            .and_then(|s| s.harnesses.into_iter().find(|h| h.harness.as_str() == kind))
            .and_then(|h| h.identity);
        // Several CLI versions can be launch-verified; record the observed
        // one and whether the adapter will require it at launch.
        let version = discovered.as_ref().map(|i| i.version.clone());
        let basis = match (&version, &discovered) {
            (Some(v), _)
                if crate::runner::pins_version(kind)
                    && crate::runner::execution_versions(kind).contains(&v.as_str()) =>
            {
                "required-runtime-profile"
            }
            // FOR-69: launch requires the probed version for this executable.
            (Some(_), Some(identity))
                if crate::runner::pins_version(kind)
                    && crate::model_catalog::Harness::from_kind(kind).is_some_and(|h| {
                        crate::model_catalog::compatibility::launchable(&h, identity)
                    }) =>
            {
                "probed-runtime-profile"
            }
            (Some(_), _) => "discovery",
            (None, _) => "unavailable-until-launch",
        };
        Ok(
            json!({"runner":{"kind":kind,"path":path,"sha256":files::hash(crate::read(&path)?),"version":version,"versionBasis":basis,"profileHash":digest(&profile),"credentialReference":digest(&json!({"kind":kind,"reference":native,"account":account,"provider":model["resolved"]["provider"]}))},"application":{"path":application,"sha256":files::hash(crate::read(&application)?),"helpers":helpers,"launchPrefixExecutable":prefix_executable},"platform":{"os":std::env::consts::OS,"arch":std::env::consts::ARCH},"image":self.0.config.evaluation_image}),
        )
    }
    pub fn comparison_preflight(&self, input: &Value) -> Result<Value> {
        crate::repository::request(
            input,
            &[
                "kind",
                "targetId",
                "baseRevision",
                "draft",
                "proposalId",
                "requestId",
            ],
        )?;
        let kind = text(input, "kind")?;
        let target_id = text(input, "targetId")?;
        let current = self
            .evaluation_catalog(kind)?
            .into_iter()
            .find(|v| v["id"] == target_id)
            .ok_or_else(|| conflict("Saved instruction no longer exists."))?;
        if current["revision"].as_u64().unwrap_or(1) != input["baseRevision"].as_u64().unwrap_or(0)
        {
            return Err(conflict(
                "Saved instruction changed. Reload before comparing.",
            ));
        }
        let current = normalized_target(&current);
        let mut proposed = normalized_target(&input["draft"]);
        proposed["revision"] = input["baseRevision"].clone();
        if current == proposed {
            return Err(conflict("Comparison needs a changed instruction."));
        }
        let changed = match kind {
            "role" => vec!["instructions"],
            "principle" => vec!["text"],
            "playbook" => vec!["description", "steps"],
            _ => unreachable!(),
        };
        let mut a = current.clone();
        let mut b = proposed.clone();
        remove(&mut a, &changed);
        remove(&mut b, &changed);
        if a != b {
            return Err(conflict(
                "Compare instruction changes only; keep names, models, effort and other role policy unchanged.",
            ));
        }
        let roles = self
            .0
            .roles
            .read()?
            .iter()
            .map(normalized_target)
            .collect::<Vec<_>>();
        let coordinator = roles
            .iter()
            .find(|r| r["id"] == "coordinator" && r["enabled"] == true)
            .or_else(|| {
                roles
                    .iter()
                    .find(|r| r["id"] == "triage" && r["enabled"] == true)
            })
            .ok_or_else(|| conflict("Coordinator role is unavailable."))?;
        let worker = roles
            .iter()
            .find(|r| r["id"] == "implementer" && r["enabled"] == true)
            .ok_or_else(|| conflict("Implementer role is unavailable."))?;
        if kind == "role" && worker["id"] != target_id {
            return Err(conflict(
                "This case pack supports the Implementer role. Other role tasks need a compatible case pack.",
            ));
        }
        if kind == "principle" && !array(&worker["principles"]).iter().any(|p| p == target_id) {
            return Err(conflict(
                "This case pack requires the selected principle to be attached to Implementer.",
            ));
        }
        let worker_model = self.0.config.assignment_model(worker)?;
        let coordinator_model = self.0.config.assignment_model(coordinator)?;
        let identity = self.evaluation_identity(&worker_model)?;
        let supporting_identity = self.evaluation_identity(&coordinator_model)?;
        let image = self.0.config.evaluation_image.as_deref().ok_or_else(|| {
            conflict("Configure the protected comparison check image before starting.")
        })?;
        if image
            .strip_prefix("sha256:")
            .is_none_or(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(conflict(
                "Comparison checks require an immutable local image digest.",
            ));
        }
        if self.0.config.engine.is_none() {
            return Err(conflict(
                "Comparison checks require a contained command engine.",
            ));
        }
        let pack = pack();
        let principles = self
            .0
            .config
            .principles()?
            .iter()
            .map(normalized_target)
            .collect::<Vec<_>>();
        let playbook = if kind == "playbook" {
            Some(current.clone())
        } else {
            None
        };
        let mut cases = Vec::new();
        for case in array(&pack["cases"]) {
            let mut c = case.clone();
            let dependencies = array(&case["requiredChecks"])
                .iter()
                .flat_map(|check| array(&check["dependencies"]).iter())
                .filter_map(Value::as_str)
                .collect::<HashSet<_>>();
            c["checkFiles"] = json!(
                array(&pack["checks"])
                    .iter()
                    .filter(|file| file["path"]
                        .as_str()
                        .is_some_and(|path| dependencies.contains(path)))
                    .collect::<Vec<_>>()
            );
            let prompt = text(case, "prompt")?;
            let brief = json!({"criteria":[{"index":0,"text":prompt}],"dependsOn":[],"projectGuidance":case["projectGuidance"]});
            let mut coordinator_brief =
                crewroom_core::principles::snapshot(&brief, coordinator, &principles)?;
            if let Some(p) = &playbook {
                coordinator_brief["playbook"] = p.clone();
            }
            let worker_brief = crewroom_core::principles::snapshot(&brief, worker, &principles)?;
            let old = json!([
                {"name":"Lead","coordinator":true,"input":prompt,"role":coordinator,"model":coordinator_model,"brief":coordinator_brief},
                {"name":"Builder","coordinator":false,"input":prompt,"role":worker,"model":worker_model,"brief":worker_brief}
            ]);
            let mut new = old.clone();
            match kind {
                "role" => new[1]["role"]["instructions"] = proposed["instructions"].clone(),
                "principle" => {
                    for p in new[1]["brief"]["principles"].as_array_mut().unwrap() {
                        if p["id"] == target_id {
                            p["text"] = proposed["text"].clone();
                        }
                    }
                }
                "playbook" => new[0]["brief"]["playbook"] = proposed.clone(),
                _ => unreachable!(),
            }
            c["inputs"] = json!({"current":old,"proposed":new});
            c["repository"] = json!({"kind":"materialized","sourceHash":c["sourceHash"],"packHash":digest(&pack),"publicationRemote":null});
            c["requirements"] = json!({"title":"Complete the assigned change","description":prompt,"criteria":[prompt]});
            cases.push(c);
        }
        let worker_tools = crate::host_protocol::host_tools(
            &json!({"purpose":"evaluation","coordinator":false,"recipients":["Lead"]}),
        );
        let coordinator_tools = crate::host_protocol::host_tools(
            &json!({"purpose":"evaluation","coordinator":true,"recipients":["Builder"]}),
        );
        let manifest = json!({"version":1,"mode":self.0.config.evaluation_mode,"target":{"kind":kind,"id":target_id,"baseRevision":input["baseRevision"],"baseHash":digest(&current),"current":current,"proposed":proposed,"proposedHash":digest(&proposed),"draftId":format!("{kind}:{target_id}:{}",digest(&proposed)),"draftVersion":1,"consumer":if kind=="playbook"{"coordinator"}else{"specialist"},"proposalId":input["proposalId"]},"cases":cases,
            "execution":{"model":worker_model,"coordinatorModel":coordinator_model,"runner":identity["runner"],"coordinatorRunner":supporting_identity["runner"],"tools":{"worker":worker_tools,"coordinator":coordinator_tools},"capabilities":["scoped-read","scoped-edit","contained-check","fixed-team-mail","handoff"],"application":identity["application"],"platform":identity["platform"],"image":image,"credentialReference":identity["runner"]["credentialReference"]},
            "design":{"repetitions":2,"variants":["current","proposed"],"scorer":{"id":"for14-v1","hash":crate::repository::files::hash(include_str!("scoring.rs")),"oracleHash":pack["privateOracleHash"]},"perCellActiveMs":180000,"totalActiveMs":2880000,"coordinator":{"role":coordinator,"model":coordinator_model},"variantMapping":{"A":"current","B":"proposed"}}});
        Ok(
            json!({"canStart":true,"reasons":[],"manifest":manifest,"summary":{"cases":4,"repetitions":2,"variants":2,"workers":16,"perCellActiveMs":180000,"totalActiveMs":2880000,"mode":self.0.config.evaluation_mode,"model":worker_model}}),
        )
    }
    pub fn verify_comparison_manifest(&self, manifest: &Value) -> Result<()> {
        let target = &manifest["target"];
        let expected=self.comparison_preflight(&json!({"kind":target["kind"],"targetId":target["id"],"baseRevision":target["baseRevision"],"draft":target["proposed"],"proposalId":target["proposalId"]}))?;
        if expected["manifest"] != *manifest {
            return Err(conflict(
                "Preflight inputs changed or the manifest was modified. Review a new preflight.",
            ));
        }
        Ok(())
    }
    pub(super) fn verify_evaluation_identity(&self, manifest: &Value) -> Result<()> {
        let current = self.evaluation_identity(&manifest["execution"]["model"])?;
        for key in ["runner", "application", "platform", "image"] {
            if current[key] != manifest["execution"][key] {
                return Err(conflict(
                    "Frozen execution identity is unavailable or changed.",
                ));
            }
        }
        if self.evaluation_identity(&manifest["execution"]["coordinatorModel"])?["runner"]
            != manifest["execution"]["coordinatorRunner"]
        {
            return Err(conflict("Frozen coordinator executable changed."));
        }
        Ok(())
    }
    pub fn verify_comparison_adoption(&self, comparison: &Value, input: &Value) -> Result<()> {
        let target = &comparison["manifest"]["target"];
        let current = self
            .evaluation_catalog(text(target, "kind")?)?
            .into_iter()
            .find(|v| v["id"] == target["id"])
            .ok_or_else(|| conflict("Adopted instruction is unavailable."))?;
        let mut saved = normalized_target(&current);
        saved["revision"] = target["baseRevision"].clone();
        if current["revision"] != input["adoptedRevision"]
            || digest(&saved) != target["proposedHash"]
        {
            return Err(conflict(
                "Saved instruction does not match the tested revision.",
            ));
        }
        Ok(())
    }
}
