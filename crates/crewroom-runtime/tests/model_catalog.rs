use crewroom_runtime::{
    model_catalog::*,
    model_policy::{resolve_assignment, resolve_runtime_fallback},
};
use serde_json::json;

fn offering(harness: Harness, provider: &str, selector: &str) -> Offering {
    Offering {
        key: offering_key(&harness, provider, selector),
        harness,
        provider: provider.into(),
        selector: selector.into(),
        display_name: "Same label".into(),
        api: Some("fixture".into()),
        observed_model: None,
        capabilities: Capabilities {
            efforts: Some(vec!["high".into()]),
            inputs: Some(vec!["text".into()]),
            tools: Some(true),
        },
        access: Access::Configured,
        execution_profile: Some("fixture-v1".into()),
        inference: None,
    }
}

#[test]
fn labels_do_not_establish_identity_or_equivalence() {
    let a = offering(Harness::Omp, "provider", "same");
    let b = offering(Harness::Codex, "codex", "same");
    let c = offering(Harness::Omp, "other-provider", "same");
    let entries = vec![a.clone(), b.clone(), c];
    validate_contract(&entries, &[]).unwrap();
    assert_eq!(candidates(&a.key, &entries, &[]).unwrap(), vec![&a]);
    assert!(candidates("Same label", &entries, &[]).is_err());
    assert_ne!(
        offering_key(&Harness::Omp, "a:b", "c"),
        offering_key(&Harness::Omp, "a", "b:c")
    );
    assert!(validate_contract(&[a.clone(), a], &[]).is_err());
}
#[test]
fn encoded_offering_keys_fit_the_public_selection_limit() {
    let too_long = offering(Harness::Codex, "codex", &"%".repeat(400));
    assert!(too_long.key.len() > 1000);
    assert!(too_long.validate().is_err());
    let accepted = offering(Harness::Codex, "codex", &"%".repeat(300));
    accepted.validate().unwrap();
    let root = tempfile::tempdir().unwrap();
    crewroom_core::model_jobs::save(&root.path().join("jobs.json"), &json!({}), &json!({"jobs":{"code":accepted.key,"judgment":"","review":"","fast":""},"budget":"small","revision":0})).unwrap();
}

#[test]
fn equivalence_requires_exact_members_and_a_total_preference_order() {
    let a = offering(Harness::Omp, "provider", "same");
    let b = offering(Harness::Codex, "codex", "same");
    let entries = vec![a.clone(), b.clone()];
    let group = Equivalence {
        key: "equivalent:verified-model".into(),
        display_name: "Verified equivalence".into(),
        members: vec![a.key.clone(), b.key.clone()],
        preference: vec![b.key.clone(), a.key.clone()],
    };
    assert_eq!(
        candidates(&group.key, &entries, std::slice::from_ref(&group)).unwrap(),
        vec![&b, &a]
    );
    for preference in [
        vec![],
        vec![a.key.clone()],
        vec![a.key.clone(), a.key.clone()],
        vec![a.key.clone(), "missing".into()],
    ] {
        let mut invalid = group.clone();
        invalid.preference = preference;
        assert!(validate_contract(&entries, &[invalid]).is_err());
    }
    let mut recursive = group.clone();
    recursive.members[0] = group.key.clone();
    recursive.preference[1] = group.key.clone();
    assert!(validate_contract(&entries, &[recursive]).is_err());
    assert!(validate_contract(&entries, &[group.clone(), group]).is_err());
}

#[test]
fn aliases_observations_and_unknown_capabilities_are_separate() {
    let mut alias = offering(Harness::ClaudeCode, "claude-code", "default");
    let key = alias.key.clone();
    alias.observed_model = Some(Observation {
        model: "claude-observed".into(),
        source: "discovery-menu".into(),
        observed_at: 123,
    });
    alias.validate().unwrap();
    let resolved = alias.resolved("high");
    assert_eq!(resolved["id"], "default");
    assert_eq!(resolved["resolvedModel"], "claude-observed");
    assert_eq!(alias.key, key);
    let requirements = json!({"tools":true,"inputs":["text"]});
    assert!(alias.blockers(true, "high", &requirements).is_empty());
    assert_eq!(alias.inference, None);
    alias.capabilities = Capabilities {
        efforts: None,
        inputs: None,
        tools: None,
    };
    alias.access = Access::Unknown;
    alias.execution_profile = None;
    assert_eq!(alias.blockers(false, "high", &requirements).len(), 6);
    assert!(
        alias
            .blockers(true, "high", &json!({"allowedModels":[]}))
            .iter()
            .any(|s| s.contains("disallows"))
    );
}

#[test]
fn legacy_selectors_remain_omp_and_catalog_intent_cannot_fallback() {
    let old =
        json!({"provider":"codex","id":"looks-native","api":"openai-responses","efforts":["high"]});
    let role = json!({"enabled":true,"model":"codex/looks-native","effort":"high"});
    let plan =
        resolve_assignment(&role, std::slice::from_ref(&old), &json!({}), &json!({})).unwrap();
    assert_eq!(crewroom_core::runner_kind(&plan).unwrap(), "omp");
    let mut frozen = plan.clone();
    frozen["selection"] = json!({"schemaVersion":1});
    let policy = json!({"fallbacks":{"codex/looks-native":[{"model":"codex/looks-native","effort":"high"}]}});
    assert!(
        resolve_runtime_fallback(
            &role,
            &frozen,
            std::slice::from_ref(&old),
            &policy,
            &json!({}),
            &[]
        )
        .unwrap()
        .is_none()
    );
    let selection = offering_key(&Harness::Codex, "codex", "looks-native");
    let role = json!({"enabled":true,"model":selection,"effort":"high"});
    let policy = json!({"fallbacks":{selection:[{"model":"codex/looks-native","effort":"high"}]}});
    assert!(resolve_assignment(&role, &[old], &policy, &json!({})).is_err());
}
