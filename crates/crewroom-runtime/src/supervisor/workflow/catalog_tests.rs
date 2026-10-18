use super::*;
use crate::model_catalog::{
    Capabilities, Offering, offering_key,
    store::{HarnessSnapshot, Outcome},
};
use crewroom_core::{authority::write_private, store::Store};
use std::os::unix::fs::DirBuilderExt;

fn fixture() -> (tempfile::TempDir, RuntimeConfig, Vec<Value>) {
    let root = tempfile::tempdir().unwrap();
    let binary = root.path().join("omp");
    std::fs::write(&binary, "fixture executable").unwrap();
    let selector = offering_key(&Harness::Omp, "fixture", "model");
    let mut config=RuntimeConfig::parse(&json!({"root":root.path(),"socket":root.path().join("tmux.sock"),"ompBinary":binary,
        "mail":{"atcDisabled":true,"retentionSeconds":600,"projectRoot":root.path().join("mail"),"url":"http://127.0.0.1:4123/mcp/","token":"secret-canary"},
        "catalog":[{"provider":"fixture","id":"model","api":"openai-responses","tools":true,"inputs":["text"],"efforts":["high","xhigh","low"]}],
        "policy":{"jobs":{"code":selector,"judgment":selector,"review":selector,"fast":selector},"budget":"large"},
        "models":{"providers":{"fixture":{"apiKey":"secret-canary","baseUrl":"http://127.0.0.1:5555"}}}})).unwrap();
    config.jobs_path = Some(root.path().join("model-jobs.json"));
    let path = root.path().join("catalog");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .unwrap();
    let mut snapshot = Snapshot {
        state: "partial".into(),
        revision: 4,
        generation: 1,
        ..Snapshot::default()
    };
    let (canonical_binary, binary_sha256) = discovery::binary_digest(&binary).unwrap();
    snapshot.harnesses[0] = HarnessSnapshot {
        harness: Harness::Omp,
        outcome: Outcome::Success,
        last_success: Some(crate::model_catalog::store::now()),
        last_attempt: Some(crate::model_catalog::store::now()),
        identity: Some(discovery::Identity {
            binary: binary.clone(),
            canonical_binary,
            binary_sha256,
            binary_stamp: discovery::binary_stamp(&binary).unwrap(),
            version: "omp/18.4.12".into(),
            profile: "fixture".into(),
        }),
        configuration_revision: Some(config.catalog_configuration(&Harness::Omp)),
        offerings: vec![Offering {
            key: selector,
            harness: Harness::Omp,
            provider: "fixture".into(),
            selector: "model".into(),
            display_name: "Example".into(),
            api: None,
            observed_model: None,
            capabilities: Capabilities {
                efforts: Some(vec!["high".into(), "xhigh".into(), "low".into()]),
                inputs: Some(vec!["text".into()]),
                tools: None,
            },
            access: Access::Unknown,
            execution_profile: None,
            inference: None,
        }],
        error: None,
    };
    write_private(
        &path.join("catalog.json"),
        &serde_json::to_vec(&snapshot).unwrap(),
    )
    .unwrap();
    let sources = config.discovery_sources(
        Path::new("/fixture/process"),
        &json!({"PATH":"/usr/bin:/bin"}),
    );
    config.discovery =
        Some(crate::model_catalog::store::Catalog::open(&path, sources, || {}).unwrap());
    let roles = Roles::new(root.path().join("roles.json")).read().unwrap();
    (root, config, roles)
}

#[test]
fn launch_and_refinement_use_the_same_selection_and_require_current_preview() {
    let (_root, mut config, roles) = fixture();
    for purpose in ["explore", "grill"] {
        let run = json!({"purpose":purpose,"taskSnapshot":{"criteria":[]}});
        let input = config.refinement_team(&run, &roles).unwrap();
        let prepared =
            preparation::prepare_assignments(&config, &run, None, &[], &input, &roles).unwrap();
        assert_eq!(
            prepared[0]["model"]["selection"]["previewRevision"],
            input["team"][0]["modelPreview"]
        );
        let mut stale = input.clone();
        stale["team"][0]["modelPreview"] = json!("0".repeat(64));
        assert!(
            preparation::prepare_assignments(&config, &run, None, &[], &stale, &roles).is_err()
        );
        let policy = config.policy.clone();
        for job in ["code", "judgment", "review", "fast"] {
            config.policy["jobs"][job] = json!("fixture/model");
        }
        assert!(
            preparation::prepare_assignments(&config, &run, None, &[], &input, &roles).is_err(),
            "A catalog preview cannot silently become a legacy assignment"
        );
        config.policy = policy;
        stale["team"][0]
            .as_object_mut()
            .unwrap()
            .remove("modelPreview");
        assert!(
            preparation::prepare_assignments(&config, &run, None, &[], &stale, &roles).is_err()
        );
    }
}

#[test]
fn pending_team_checks_current_facts_but_applied_replay_keeps_frozen_models() {
    let (_root, mut config, roles) = fixture();
    let role = roles.iter().find(|r| r["id"] == "tech-writer").unwrap();
    let model = config.assignment_model(role).unwrap();
    let mut proposal = json!({"status":"pending","specialists":[{"role":role,"model":model}]});
    config.verify_team_admission(&proposal).unwrap();
    config.policy["budget"] = json!("unlimited");
    assert!(config.verify_team_admission(&proposal).is_err());
    proposal["status"] = json!("applied");
    config.verify_team_admission(&proposal).unwrap();
    assert_eq!(proposal["specialists"][0]["model"], model);
    config.verify_frozen_catalog_route(&model).unwrap();
}
#[test]
fn frozen_catalog_admission_survives_settings_edits_and_store_restart() {
    let (root, config, roles) = fixture();
    let role = roles.iter().find(|r| r["id"] == "coordinator").unwrap();
    let model = config.assignment_model(role).unwrap();
    let store = Store::open(&root.path().join("store.sqlite")).unwrap();
    let task = store
        .create_task(&json!({"title":"Catalog fixture","description":""}))
        .unwrap();
    let run = store
        .queue_refinement(
            task["id"].as_str().unwrap(),
            &json!({"revision":1,"purpose":"explore"}),
        )
        .unwrap();
    let admitted=store.admit_assignment(run["id"].as_str().unwrap(),&json!({"name":"Coordinator","coordinator":true,"role":role,"model":model,"input":"Inspect"})).unwrap();
    let mut jobs = crewroom_core::model_jobs::defaults(&config.policy);
    jobs["budget"] = json!("small");
    crewroom_core::model_jobs::save(config.jobs_path.as_ref().unwrap(), &config.policy, &jobs)
        .unwrap();
    assert!(config.verify_model_preview(role, &model).is_err());
    config.verify_frozen_catalog_route(&model).unwrap();
    assert!(!model.to_string().contains("secret-canary"));
    drop(store);
    let store = Store::open(&root.path().join("store.sqlite")).unwrap();
    assert_eq!(
        store
            .get_assignment(admitted["assignment"]["id"].as_str().unwrap())
            .unwrap(),
        admitted["assignment"]
    );
    std::fs::write(root.path().join("omp"), "changed executable").unwrap();
    assert!(config.assignment_model(role).is_err());
    assert!(config.verify_frozen_catalog_route(&model).is_err());
}

#[test]
fn wrong_profile_alias_or_legacy_route_cannot_replace_a_frozen_offering() {
    let (_root, mut config, roles) = fixture();
    let role = roles.iter().find(|r| r["id"] == "coordinator").unwrap();
    let model = config.assignment_model(role).unwrap();
    let mut tampered = model.clone();
    tampered["resolved"]["adapter"] = json!("codex");
    assert!(config.verify_frozen_catalog_route(&tampered).is_err());
    config.tools.push(json!({"name":"new-tool"}));
    assert!(config.verify_frozen_catalog_route(&model).is_err());
    config
        .verify_frozen_catalog_route(&json!({"resolved":{"provider":"codex","id":"historical"}}))
        .unwrap();
}

#[test]
fn native_account_profile_and_capability_drift_require_readmission() {
    let (root, mut config, roles) = fixture();
    let binary = root.path().join("omp");
    let auth = root.path().join("auth.json");
    let login = |account: &str| {
        write_private(&auth,serde_json::to_string(&json!({"auth_mode":"chatgpt","tokens":{"access_token":"account-secret-canary","account_id":account}})).unwrap().as_bytes()).unwrap()
    };
    login("account-one");
    config.codex = json!({"binary":binary,"authFile":auth});
    let key = offering_key(&Harness::Codex, "codex", "native-model");
    config.catalog = vec![
        json!({"adapter":"codex","provider":"codex","id":"native-model","api":"codex-app-server","tools":true,"inputs":["text"],"efforts":["high","xhigh","low"]}),
    ];
    for job in ["code", "judgment", "review", "fast"] {
        config.policy["jobs"][job] = json!(key);
    }
    let mut snapshot = config.discovery.as_ref().unwrap().snapshot().unwrap();
    let mut native = snapshot.harnesses[0].clone();
    native.harness = Harness::Codex;
    native.identity.as_mut().unwrap().version = "codex-cli 0.162.0".into();
    native.configuration_revision = Some(config.catalog_configuration(&Harness::Codex));
    let offering = &mut native.offerings[0];
    offering.harness = Harness::Codex;
    offering.provider = "codex".into();
    offering.selector = "native-model".into();
    offering.key = key.clone();
    snapshot.harnesses[1] = native;
    let path = root.path().join("native-catalog");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .unwrap();
    write_private(
        &path.join("catalog.json"),
        &serde_json::to_vec(&snapshot).unwrap(),
    )
    .unwrap();
    config.discovery = Some(
        crate::model_catalog::store::Catalog::open(
            &path,
            config.discovery_sources(Path::new("/fixture/process"), &json!({})),
            || {},
        )
        .unwrap(),
    );
    let role = roles.iter().find(|r| r["id"] == "coordinator").unwrap();
    let model = config.assignment_model(role).unwrap();
    config.verify_frozen_catalog_route(&model).unwrap();
    assert!(!model.to_string().contains("account-secret-canary"));
    assert!(!model.to_string().contains("account-one"));
    login("account-two");
    assert!(config.assignment_model(role).is_err());
    assert!(
        config
            .verify_frozen_catalog_route(&model)
            .unwrap_err()
            .to_string()
            .contains("Re-admission is required")
    );
    login("account-one");
    config.verify_frozen_catalog_route(&model).unwrap();
    config.codex["profile"] = json!("changed-profile");
    assert!(config.assignment_model(role).is_err());
    assert!(config.verify_frozen_catalog_route(&model).is_err());
    config.codex.as_object_mut().unwrap().remove("profile");
    config.catalog[0]["tools"] = json!(false);
    assert!(config.assignment_model(role).is_err());
    assert!(config.verify_frozen_catalog_route(&model).is_err());
    assert_eq!(model["selection"]["offering"]["key"], key);
    assert_eq!(model["resolved"]["adapter"], "codex");
}
