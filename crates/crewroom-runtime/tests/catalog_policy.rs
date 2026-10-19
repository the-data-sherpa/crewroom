use crewroom_runtime::model_catalog::{policy::*, *};
use serde_json::{Value, json};
fn route(harness: Harness, selector: &str) -> Route {
    let provider = harness.as_str().to_owned();
    let capabilities = Capabilities {
        efforts: Some(
            ["off", "minimal", "low", "medium", "high", "xhigh", "max"]
                .map(String::from)
                .to_vec(),
        ),
        inputs: Some(vec!["text".into()]),
        tools: Some(true),
    };
    Route::new(
        Offering {
            key: offering_key(&harness, &provider, selector),
            harness,
            provider,
            selector: selector.into(),
            display_name: "Equal label".into(),
            api: Some("fixture".into()),
            observed_model: None,
            capabilities: capabilities.clone(),
            access: Access::Configured,
            execution_profile: Some("fixture".into()),
            inference: None,
            billing: None,
        },
        capabilities,
        true,
        None,
        Some("configuration".into()),
    )
}
fn policy(key: &str) -> Value {
    json!({"jobs":{"code":key,"judgment":key,"review":key,"fast":key},"budget":"large","revision":1})
}
#[test]
fn all_jobs_and_role_overrides_share_precedence_and_preserve_selection_sources() {
    let routes = [route(Harness::Codex, "one"), route(Harness::Omp, "two")];
    let policy = policy(&routes[0].offering.key);
    for (job, effort) in [
        ("code", "high"),
        ("judgment", "xhigh"),
        ("review", "xhigh"),
        ("fast", "low"),
    ] {
        let mut role = json!({"enabled":true,"job":job,"model":"","effort":"default"});
        let result = resolve(
            &role,
            &policy,
            &json!({"tools":true,"inputs":["text"]}),
            &routes,
            &[],
            12,
        )
        .unwrap();
        assert_eq!(result["resolved"]["effort"], effort);
        assert_eq!(result["selection"]["modelSource"], "job");
        assert_eq!(result["selection"]["catalogRevision"], 12);
        role["model"] = json!(routes[1].offering.key);
        role["effort"] = json!("off");
        let result = resolve(&role, &policy, &json!({}), &routes, &[], 12).unwrap();
        assert_eq!(result["resolved"]["adapter"], "omp");
        assert_eq!(result["resolved"]["effort"], "off");
        assert_eq!(result["selection"]["modelSource"], "role_override");
        assert_eq!(result["selection"]["effortSource"], "role_override");
    }
}
#[test]
fn unavailable_unsupported_unknown_and_disallowed_choices_explain_the_blocker() {
    let first = route(Harness::Codex, "one");
    let policy = policy(&first.offering.key);
    let role = json!({"enabled":true,"job":"code"});
    let requirements = json!({"tools":true,"inputs":["text"]});
    for condition in 0..6 {
        let mut candidate = first.clone();
        match condition {
            0 => candidate.fresh = false,
            1 => candidate.offering.access = Access::Unknown,
            2 => candidate.offering.execution_profile = None,
            3 => candidate.offering.capabilities.efforts = Some(vec!["low".into()]),
            4 => candidate.offering.capabilities.tools = None,
            _ => candidate.offering.capabilities.inputs = None,
        }
        assert!(resolve(&role, &policy, &requirements, &[candidate], &[], 1).is_err());
    }
    assert!(
        resolve(
            &role,
            &policy,
            &json!({"allowedModels":[]}),
            &[first],
            &[],
            1
        )
        .unwrap_err()
        .to_string()
        .contains("disallows")
    );
}
#[test]
fn configured_profile_cannot_supply_missing_discovery_efforts() {
    let mut candidate = route(Harness::Codex, "one");
    candidate.offering.capabilities.efforts = None;
    candidate.offering.capabilities.inputs = None;
    candidate.offering.capabilities.tools = None;
    let profile = json!({"adapter":"codex","provider":"codex","id":"one","api":"codex-app-server","efforts":["low"],"inputs":["text"],"tools":true});
    candidate.offering = with_profile(&candidate.offering, Some(&profile), Access::Configured);
    assert_eq!(candidate.offering.capabilities.efforts, None);
    assert_eq!(
        candidate.offering.capabilities.inputs,
        Some(vec!["text".into()])
    );
    assert_eq!(candidate.offering.capabilities.tools, Some(true));
    let key = candidate.offering.key.clone();
    let requirements = json!({"tools":true,"inputs":["text"]});
    let role = json!({"enabled":true,"model":key,"effort":"low"});
    assert!(
        resolve(
            &role,
            &policy(&key),
            &requirements,
            &[candidate.clone()],
            &[],
            1
        )
        .is_err()
    );
    assert_eq!(
        choices(&[candidate], &[], &requirements).unwrap()[0]["eligibility"]["low"]["eligible"],
        false
    );
}
#[test]
fn equivalence_preferences_skip_ineligible_routes_and_changed_facts_reject_a_preview() {
    let mut routes = [route(Harness::Codex, "one"), route(Harness::Omp, "two")];
    let keys = routes
        .iter()
        .map(|r| r.offering.key.clone())
        .collect::<Vec<_>>();
    let group = Equivalence {
        key: "equivalent:one-model".into(),
        display_name: "One model".into(),
        members: keys.clone(),
        preference: keys,
    };
    let mut policy = policy(&group.key);
    let role = json!({"enabled":true,"job":"code"});
    let first = resolve(
        &role,
        &policy,
        &json!({}),
        &routes,
        std::slice::from_ref(&group),
        1,
    )
    .unwrap();
    let preview = &first["selection"]["previewRevision"];
    verify_preview(&first, preview).unwrap();
    assert!(verify_preview(&first, &Value::Null).is_err());
    routes[0].fresh = false;
    let changed = resolve(
        &role,
        &policy,
        &json!({}),
        &routes,
        std::slice::from_ref(&group),
        2,
    )
    .unwrap();
    assert_eq!(changed["resolved"]["adapter"], "omp");
    assert!(verify_preview(&changed, preview).is_err());
    policy["revision"] = json!(2);
    let changed = resolve(
        &role,
        &policy,
        &json!({}),
        &routes,
        std::slice::from_ref(&group),
        2,
    )
    .unwrap();
    assert!(verify_preview(&changed, preview).is_err());
    routes[1].fresh = false;
    assert!(resolve(&role, &policy, &json!({}), &routes, &[group], 3).is_err());
}
#[test]
fn profile_facts_do_not_invent_unknown_capabilities_or_replace_an_alias() {
    let mut discovered = route(Harness::ClaudeCode, "default").offering;
    discovered.api = None;
    discovered.execution_profile = None;
    discovered.capabilities.tools = None;
    discovered.capabilities.inputs = None;
    discovered.observed_model = Some(Observation {
        model: "claude-observed".into(),
        source: "metadata".into(),
        observed_at: 100,
    });
    let mut profile = json!({"adapter":"claude-code","provider":"claude-code","id":"default","api":"claude-code-stream","resolvedModel":"claude-observed","tools":true,"inputs":["text"],"efforts":["high"]});
    let supported = with_profile(&discovered, Some(&profile), Access::Configured);
    assert_eq!(supported.capabilities.tools, Some(true));
    assert_eq!(supported.selector, "default");
    assert_eq!(supported.capabilities.efforts, Some(vec!["high".into()]));
    profile["resolvedModel"] = json!("new-alias-target");
    assert_eq!(
        with_profile(&discovered, Some(&profile), Access::Configured).execution_profile,
        None
    );
    assert_eq!(
        with_profile(&discovered, None, Access::Configured)
            .capabilities
            .tools,
        None
    );
}

#[test]
fn picker_eligibility_matches_admission_for_each_effort_and_equivalent_route() {
    let mut routes = [route(Harness::Codex, "one"), route(Harness::Omp, "two")];
    routes[0].offering.capabilities.efforts = Some(vec!["high".into()]);
    routes[1].offering.capabilities.tools = None;
    let keys = routes
        .iter()
        .map(|r| r.offering.key.clone())
        .collect::<Vec<_>>();
    let groups = [Equivalence {
        key: "equivalent:pair".into(),
        display_name: "Equal label".into(),
        members: keys.clone(),
        preference: keys,
    }];
    let requirements = json!({"tools":true,"inputs":["text"]});
    let rows = choices(&routes, &groups, &requirements).unwrap();
    for row in rows.as_array().unwrap() {
        for effort in ["off", "minimal", "low", "medium", "high", "xhigh", "max"] {
            let role = json!({"enabled":true,"job":"code","model":row["key"],"effort":effort});
            let resolved = resolve(&role, &policy("unused"), &requirements, &routes, &groups, 1);
            let eligibility = &row["eligibility"][effort];
            assert_eq!(eligibility["eligible"], resolved.is_ok());
            if let Ok(model) = resolved {
                assert_eq!(
                    eligibility["routeKey"],
                    model["selection"]["offering"]["key"]
                );
            } else {
                assert!(!eligibility["reasons"].as_array().unwrap().is_empty());
            }
        }
    }
}

fn probe(harness: &Harness, version: &str, full: bool) -> compatibility::ProbeReceipt {
    let mut receipt = compatibility::ProbeReceipt::new(
        harness,
        version,
        std::path::Path::new("/fixture/bin"),
        "abc",
        1,
    );
    if !full {
        receipt.checks.truncate(2);
    }
    receipt
}

#[test]
fn admission_freezes_the_capability_record_and_states_its_fallbacks() {
    let mut candidate = route(Harness::ClaudeCode, "sonnet");
    candidate.harness_capabilities = Some(capabilities::HarnessCapabilities::probed(&probe(
        &Harness::ClaudeCode,
        "3.0.0 (Claude Code)",
        true,
    )));
    let key = candidate.offering.key.clone();
    let role = json!({"enabled":true,"model":key,"effort":"low"});
    let requirements = json!({"tools":true,"inputs":["text"]});
    let result = resolve(
        &role,
        &policy(&key),
        &requirements,
        &[candidate.clone()],
        &[],
        1,
    )
    .expect("a probed version still runs");
    let selection = &result["selection"];
    assert_eq!(selection["harnessCapabilities"]["basis"], "probed");
    let notes: Vec<_> = selection["degradedModes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap())
        .collect();
    assert!(notes[0].starts_with("Steering waits until the current turn ends"));
    assert!(
        notes
            .iter()
            .any(|n| n.starts_with("A replacement agent starts a fresh session"))
    );
    assert!(
        notes
            .iter()
            .any(|n| n.starts_with("Cost is a list-price estimate"))
    );
    let choice = &choices(&[candidate], &[], &requirements).unwrap()[0];
    assert_eq!(choice["degradedModes"], selection["degradedModes"]);
    assert_eq!(choice["harness"], "claude-code");
    assert_eq!(choice["source"], "runner");
    assert_eq!(choice["eligibility"]["low"]["eligible"], true);
}

#[test]
fn a_capability_refusal_blocks_admission_and_the_picker_alike() {
    let mut candidate = route(Harness::Codex, "one");
    candidate.harness_capabilities = Some(capabilities::HarnessCapabilities::probed(&probe(
        &Harness::Codex,
        "codex-cli 1.0.0",
        false,
    )));
    let key = candidate.offering.key.clone();
    let role = json!({"enabled":true,"model":key,"effort":"low"});
    let requirements = json!({"tools":true,"inputs":["text"]});
    let refused = resolve(
        &role,
        &policy(&key),
        &requirements,
        &[candidate.clone()],
        &[],
        1,
    )
    .unwrap_err()
    .to_string();
    assert!(refused.contains("gated host tools"), "{refused}");
    let choice = &choices(&[candidate], &[], &requirements).unwrap()[0];
    assert_eq!(choice["eligibility"]["low"]["eligible"], false);
    let blockers = choice["eligibility"]["low"]["blockers"].as_array().unwrap();
    assert_eq!(blockers.len(), 1, "bare reasons, without the route prefix");
    assert!(
        blockers[0]
            .as_str()
            .unwrap()
            .starts_with("Codex codex-cli 1.0.0")
    );
}

#[test]
fn a_login_launch_blocker_is_the_only_reason_shown() {
    let mut candidate = route(Harness::Omp, "login-model");
    candidate.launch_blocker = Some("Listed by your OMP login (zai).".into());
    candidate.fresh = false;
    let requirements = json!({"tools":true,"inputs":["text"]});
    let choice = &choices(&[candidate], &[], &requirements).unwrap()[0];
    assert_eq!(choice["source"], "omp-login");
    for effort in ["low", "high"] {
        assert_eq!(
            choice["eligibility"][effort]["blockers"],
            json!(["Listed by your OMP login (zai)."])
        );
    }
}

#[test]
fn choices_carry_bundled_status_and_unknown_capabilities() {
    let mut legacy = route(Harness::Codex, "gpt-5.5");
    legacy.offering.capabilities.tools = None;
    let unknown = route(Harness::Codex, "not-in-the-file");
    let rows = choices(&[legacy, unknown], &[], &json!({})).unwrap();
    assert_eq!(rows[0]["status"], "legacy");
    assert_eq!(rows[0]["facts"], "bundled");
    assert_eq!(rows[0]["unknownCapabilities"], json!(["tools"]));
    assert_eq!(rows[1]["status"], Value::Null);
    assert_eq!(rows[1]["facts"], "live-only");
    assert_eq!(rows[1]["efforts"][2], "low");
}
