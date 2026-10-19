//! Derived execution profiles are accepted by the runner's own checks:
//! `RuntimeConfig::parse` and `policy::with_profile`, per harness.
use super::*;
use crewroom_runtime::model_catalog::{Access, Observation, policy};

fn offering(harness: Harness, provider: &str, selector: &str, efforts: &[&str]) -> Offering {
    Offering {
        key: crewroom_runtime::model_catalog::offering_key(&harness, provider, selector),
        harness,
        provider: provider.into(),
        selector: selector.into(),
        display_name: format!("Synthetic {selector}"),
        api: None,
        observed_model: None,
        capabilities: Capabilities {
            efforts: Some(efforts.iter().map(|e| (*e).to_owned()).collect()),
            inputs: Some(vec!["text".into()]),
            tools: None,
        },
        access: Access::Unknown,
        execution_profile: None,
        inference: None,
        billing: None,
    }
}
fn efforts(list: &[&str]) -> Vec<String> {
    list.iter().map(|e| (*e).to_owned()).collect()
}
fn providers() -> Value {
    json!({"fixture":{"baseUrl":"http://127.0.0.1:9","api":"anthropic-messages"}})
}
/// The derived profile in a complete configuration parses, and the runner's
/// pairing rule turns the discovered offering into an executable one.
fn accepted(discovered: &Offering, profile: &Value, expected: &[&str]) {
    let config = json!({"root":"/synthetic/hosts","socket":"/synthetic/hosts/tmux.sock",
        "mail":{"url":"http://127.0.0.1:9/mcp/","token":"synthetic","projectRoot":"/synthetic","retentionSeconds":86400,"atcDisabled":true},
        "catalog":[profile],"policy":{},"models":{"providers":providers()}});
    RuntimeConfig::parse(&config).unwrap();
    assert!(matches(
        profile,
        &discovered.harness,
        &discovered.provider,
        &discovered.selector
    ));
    let result = policy::with_profile(discovered, Some(profile), Access::Configured);
    assert!(
        result.execution_profile.is_some(),
        "{profile} was not accepted"
    );
    assert_eq!(result.capabilities.efforts, Some(efforts(expected)));
    assert_eq!(result.capabilities.tools, Some(true));
    assert!(
        result
            .blockers(true, expected[0], &json!({"inputs":["text"],"tools":true}))
            .is_empty(),
        "{:?}",
        result.blockers(true, expected[0], &json!({"inputs":["text"],"tools":true}))
    );
}

#[test]
fn codex_profiles_derive_from_discovery_with_the_chosen_efforts() {
    let discovered = offering(
        Harness::Codex,
        "codex",
        "gpt-synthetic",
        &["low", "medium", "high"],
    );
    let profile = derive(&discovered, &efforts(&["high", "low", "low"]), &json!({})).unwrap();
    assert_eq!(
        profile,
        json!({"adapter":"codex","provider":"codex","id":"gpt-synthetic","api":"codex-app-server","inputs":["text"],"efforts":["low","high"],"tools":true})
    );
    accepted(&discovered, &profile, &["low", "high"]);
}

#[test]
fn claude_code_profiles_name_the_observed_model() {
    let mut discovered = offering(
        Harness::ClaudeCode,
        "claude-code",
        "default",
        &["low", "high"],
    );
    let refused = derive(&discovered, &efforts(&["low"]), &json!({})).unwrap_err();
    assert!(refused.contains("did not report the model"), "{refused}");
    discovered.observed_model = Some(Observation {
        model: "claude-synthetic-resolved".into(),
        source: "discovery-metadata".into(),
        observed_at: 1,
    });
    let profile = derive(&discovered, &efforts(&["high"]), &json!({})).unwrap();
    assert_eq!(profile["resolvedModel"], "claude-synthetic-resolved");
    assert_eq!(profile["api"], "claude-code-stream");
    accepted(&discovered, &profile, &["high"]);
}

#[test]
fn omp_profiles_take_the_api_of_the_configured_provider() {
    for api in OMP_APIS {
        let discovered = offering(Harness::Omp, "fixture", "model", &["low", "xhigh"]);
        let providers = json!({"fixture":{"baseUrl":"http://127.0.0.1:9","api":api}});
        let profile = derive(&discovered, &efforts(&["xhigh"]), &providers).unwrap();
        assert_eq!(
            profile,
            json!({"provider":"fixture","id":"model","api":api,"inputs":["text"],"efforts":["xhigh"],"tools":true})
        );
        accepted(&discovered, &profile, &["xhigh"]);
    }
    let discovered = offering(Harness::Omp, "fixture", "model", &["low"]);
    let missing = derive(&discovered, &efforts(&["low"]), &json!({})).unwrap_err();
    assert!(missing.contains("not defined"), "{missing}");
    let unknown = derive(
        &discovered,
        &efforts(&["low"]),
        &json!({"fixture":{"api":"openai-completions"}}),
    )
    .unwrap_err();
    assert!(
        unknown.contains("Set api for provider fixture"),
        "{unknown}"
    );
}

#[test]
fn efforts_are_chosen_within_the_advertised_set() {
    let discovered = offering(Harness::Codex, "codex", "gpt-synthetic", &["low"]);
    assert!(
        derive(&discovered, &[], &json!({}))
            .unwrap_err()
            .contains("at least one")
    );
    assert!(
        derive(&discovered, &efforts(&["xhigh"]), &json!({}))
            .unwrap_err()
            .contains("not advertised")
    );
    let mut silent = discovered.clone();
    silent.capabilities.efforts = None;
    assert!(
        derive(&silent, &efforts(&["low"]), &json!({}))
            .unwrap_err()
            .contains("did not advertise")
    );
}

fn catalog_view(offerings: &[Offering]) -> Value {
    json!({"state":"ready","offerings":offerings.iter().flat_map(|o| {
        // Two billing routes for one offering are listed once.
        let mut narrowed = o.with_billing(crewroom_runtime::model_catalog::BillingRoute::Subscription);
        narrowed.capabilities.efforts = Some(vec!["low".into()]);
        [json!({"offering":narrowed,"fresh":true,"advertisedCapabilities":o.capabilities}),
         json!({"offering":o.with_billing(crewroom_runtime::model_catalog::BillingRoute::Api),"fresh":false,"advertisedCapabilities":o.capabilities})]
    }).collect::<Vec<_>>()})
}
fn saved() -> Value {
    json!({"root":"/synthetic/hosts","socket":"/synthetic/hosts/tmux.sock",
        "mail":{"url":"http://127.0.0.1:9/mcp/","token":{"$crewroomSecret":"runner-mail"},"projectRoot":"/synthetic","retentionSeconds":86400,"atcDisabled":true},
        "codex":{"binary":"/synthetic/codex","authFile":"/synthetic/auth.json"},
        "catalog":[{"provider":"other","id":"kept","api":"openai-responses","inputs":["text"],"efforts":["low"],"tools":true}],
        "policy":{},"models":{"providers":{"other":{"baseUrl":"http://127.0.0.1:9","api":"openai-responses","apiKey":{"$crewroomSecret":"runner-provider-other"}}}}})
}

#[test]
fn the_listing_shows_profiles_per_configured_harness_and_approves_nothing() {
    let codex = offering(Harness::Codex, "codex", "gpt-synthetic", &["low", "high"]);
    let omp = offering(Harness::Omp, "fixture", "model", &["low"]);
    let view = catalog_view(&[codex.clone(), omp]);
    let listing = listing(Some(&saved()), &view);
    let harnesses = array(&listing["harnesses"]);
    assert_eq!(harnesses[0]["harness"], "omp");
    assert_eq!(harnesses[0]["configured"], false);
    assert!(array(&harnesses[0]["offerings"]).is_empty());
    let rows = array(&harnesses[1]["offerings"]);
    assert_eq!(rows.len(), 1, "{listing}");
    assert_eq!(rows[0]["selector"], "gpt-synthetic");
    assert_eq!(rows[0]["advertisedEfforts"], json!(["low", "high"]));
    assert_eq!(
        rows[0]["profile"],
        Value::Null,
        "discovery approves nothing"
    );
    assert_eq!(rows[0]["approvable"], true);
    assert_eq!(rows[0]["fresh"], false, "one stale route keeps it stale");
    assert_eq!(harnesses[2]["configured"], false);
    // A saved profile that discovery did not list is still shown.
    assert_eq!(
        listing["unmatched"],
        json!([{"harness":"omp","provider":"other","selector":"kept","efforts":["low"]}])
    );
    assert!(listing["note"].as_str().unwrap().contains("does not prove"));

    // Approve, then the listing shows the profile; removal takes it away.
    let approve = request(&json!({"harness":"codex","provider":"codex","selector":"gpt-synthetic","approve":true,"efforts":["high"]})).unwrap();
    let body = change(&saved(), &approve, &view).unwrap();
    let catalog = array(&body["advanced"]["catalog"]);
    assert_eq!(catalog.len(), 2);
    assert_eq!(catalog[1]["efforts"], json!(["high"]));
    assert!(
        body["advanced"].get("codex").is_none(),
        "form keys stay in fields"
    );
    assert_eq!(body["fields"]["codexBinary"], "/synthetic/codex");
    assert!(
        body["advanced"]["models"]["providers"]["other"]
            .get("apiKey")
            .is_none(),
        "stored key references are kept by the save path, never resent"
    );
    let mut approved = saved();
    approved["catalog"] = body["advanced"]["catalog"].clone();
    let listing = super::listing(Some(&approved), &view);
    assert_eq!(
        listing["harnesses"][1]["offerings"][0]["profile"],
        json!({"efforts":["high"],"api":"codex-app-server"})
    );
    let remove = request(
        &json!({"harness":"codex","provider":"codex","selector":"gpt-synthetic","approve":false}),
    )
    .unwrap();
    let body = change(&approved, &remove, &view).unwrap();
    assert_eq!(array(&body["advanced"]["catalog"]).len(), 1);
    assert!(matches!(
        change(&saved(), &remove, &view),
        Err(Error::NotFound(_))
    ));
}

#[test]
fn approval_needs_a_discovered_offering_on_a_configured_harness() {
    let view = catalog_view(&[offering(
        Harness::ClaudeCode,
        "claude-code",
        "default",
        &["low"],
    )]);
    let missing = request(&json!({"harness":"codex","provider":"codex","selector":"absent","approve":true,"efforts":["low"]})).unwrap();
    assert!(
        matches!(change(&saved(), &missing, &view), Err(Error::NotFound(m)) if m.contains("Refresh models"))
    );
    let unconfigured = request(&json!({"harness":"claude-code","provider":"claude-code","selector":"default","approve":true,"efforts":["low"]})).unwrap();
    assert!(
        matches!(change(&saved(), &unconfigured, &view), Err(Error::Conflict(m)) if m.contains("not configured"))
    );
    for bad in [
        json!({"harness":"codex","provider":"codex","selector":"x","approve":true,"extra":1}),
        json!({"harness":"other","provider":"codex","selector":"x","approve":true}),
        json!({"harness":"codex","provider":"codex","selector":"x"}),
        json!({"harness":"codex","provider":"codex","selector":"x","approve":false,"efforts":["low"]}),
    ] {
        assert!(request(&bad).is_err(), "{bad}");
    }
}
