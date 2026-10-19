use super::*;
use crate::model_catalog::{
    Capabilities, Offering, offering_key, routed_key,
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
            compatibility: None,
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
            billing: None,
        }],
        error: None,
        login: None,
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
    // A selection saved before billing routes resolves to the only route.
    assert_eq!(
        model["selection"]["offering"]["key"],
        routed_key(
            &Harness::Codex,
            "codex",
            "native-model",
            BillingRoute::Subscription
        )
    );
    assert_eq!(model["selection"]["billingRoute"], "subscription");
    assert_eq!(model["resolved"]["adapter"], "codex");
}

#[test]
fn newer_listed_cli_keeps_its_models_without_an_execution_profile() {
    let (root, mut config, roles) = fixture();
    let role = roles.iter().find(|r| r["id"] == "coordinator").unwrap();
    config.assignment_model(role).unwrap();
    let path = root.path().join("catalog");
    let mut snapshot: Snapshot =
        serde_json::from_slice(&std::fs::read(path.join("catalog.json")).unwrap()).unwrap();
    snapshot.harnesses[0].identity.as_mut().unwrap().version = "omp/99.0.0".into();
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
    let routes = config
        .catalog_routes(
            &config.discovery.as_ref().unwrap().snapshot().unwrap(),
            false,
        )
        .unwrap();
    assert_eq!(routes.len(), 1, "the listed model stays visible");
    assert_eq!(routes[0].offering.execution_profile, None);
    let error = config.assignment_model(role).unwrap_err().to_string();
    assert!(error.contains("No verified adapter profile"), "{error}");
    let note = snapshot.harnesses[0].execution_note().unwrap();
    assert!(note.contains("OMP omp/99.0.0 lists models"), "{note}");
    assert!(note.contains("omp/18.4.12, omp/18.11.3"), "{note}");
}

/// FOR-52: OMP 18.11.3 passed the actual-binary contract suites and one live
/// catalog workflow launch, so its listed models get an execution profile and
/// carry no execution note.
#[test]
fn launch_verified_omp_18_11_3_offerings_are_eligible() {
    let (root, mut config, roles) = fixture();
    let role = roles.iter().find(|r| r["id"] == "coordinator").unwrap();
    let path = root.path().join("catalog");
    let mut snapshot: Snapshot =
        serde_json::from_slice(&std::fs::read(path.join("catalog.json")).unwrap()).unwrap();
    snapshot.harnesses[0].identity.as_mut().unwrap().version = "omp/18.11.3".into();
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
    let routes = config
        .catalog_routes(
            &config.discovery.as_ref().unwrap().snapshot().unwrap(),
            false,
        )
        .unwrap();
    assert_eq!(routes.len(), 1);
    assert!(routes[0].offering.execution_profile.is_some());
    let model = config.assignment_model(role).unwrap();
    assert_eq!(model["resolved"]["adapter"], "omp");
    assert_eq!(snapshot.harnesses[0].execution_note(), None);
    assert_eq!(
        snapshot.harnesses[0].compatibility().as_deref(),
        Some("verified")
    );
}

/// FOR-69: a newer unlisted version whose compatibility probe passed for this
/// exact executable is offered as `compatible (probed …)`, and its admission
/// records the receipt. A changed executable holds the launch.
#[tokio::test]
async fn probed_newer_versions_are_offered_and_changed_executables_hold_launch() {
    use crate::model_catalog::compatibility::{Basis, Compatibility, ProbeReceipt};
    let (root, mut config, roles) = fixture();
    let role = roles.iter().find(|r| r["id"] == "coordinator").unwrap();
    let path = root.path().join("catalog");
    let mut snapshot: Snapshot =
        serde_json::from_slice(&std::fs::read(path.join("catalog.json")).unwrap()).unwrap();
    let identity = snapshot.harnesses[0].identity.as_mut().unwrap();
    identity.version = "omp/19.0.0".into();
    let receipt = ProbeReceipt::new(
        &Harness::Omp,
        "omp/19.0.0",
        &identity.canonical_binary,
        &identity.binary_sha256,
        crate::model_catalog::store::now(),
    );
    identity.compatibility = Some(Compatibility {
        basis: Basis::Probed,
        probe: Some(receipt.clone()),
    });
    snapshot.probes = vec![receipt.clone()];
    write_private(
        &path.join("catalog.json"),
        &serde_json::to_vec(&snapshot).unwrap(),
    )
    .unwrap();
    let reopen = |config: &mut RuntimeConfig| {
        let sources = config.discovery_sources(
            Path::new("/fixture/process"),
            &json!({"PATH":"/usr/bin:/bin"}),
        );
        config.discovery =
            Some(crate::model_catalog::store::Catalog::open(&path, sources, || {}).unwrap());
    };
    reopen(&mut config);
    let routes = config
        .catalog_routes(
            &config.discovery.as_ref().unwrap().snapshot().unwrap(),
            false,
        )
        .unwrap();
    assert!(routes[0].offering.execution_profile.is_some());
    assert_eq!(snapshot.harnesses[0].execution_note(), None);
    assert_eq!(
        snapshot.harnesses[0].compatibility().as_deref(),
        Some("compatible (probed 19.0.0)")
    );
    let view = crate::model_catalog::store::Catalog::view(
        config.discovery.as_ref().unwrap(),
        crate::model_catalog::store::now(),
    );
    assert_eq!(view["probes"][0]["id"], json!(receipt.id));
    // The admitted selection records the version and its probe receipt.
    let model = config.assignment_model(role).unwrap();
    let frozen = &model["selection"]["identity"];
    assert_eq!(frozen["version"], "omp/19.0.0");
    assert_eq!(frozen["compatibility"]["basis"], "probed");
    assert_eq!(frozen["compatibility"]["probe"]["id"], json!(receipt.id));
    assert_eq!(
        frozen["compatibility"]["probe"]["inferenceRequested"],
        false
    );
    assert_eq!(config.hold_changed_executable(&model).await.unwrap(), None);
    // The executable changes between admission and launch and its re-probe
    // fails: the launch is held with the reason, never started.
    std::fs::write(root.path().join("omp"), "rebuilt executable").unwrap();
    let held = config
        .hold_changed_executable(&model)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        held.contains("Launch held: the OMP executable changed after admission (omp/19.0.0)"),
        "{held}"
    );
    assert!(held.contains("re-probe failed"), "{held}");
    assert!(!held.contains("secret-canary"), "{held}");
    assert!(config.verify_frozen_catalog_route(&model).is_err());
    // A receipt bound to other content never makes a version launchable.
    let mut tampered = snapshot.harnesses[0].clone();
    tampered.identity.as_mut().unwrap().binary_sha256 = "other".into();
    assert_eq!(tampered.compatibility(), None);
    assert!(tampered.execution_note().is_some());
}

/// A simulated OMP that reports `version`, lists `listed` models from the
/// `fixture` provider and answers the protocol-v2 RPC handshake.
fn simulated_omp(path: &Path, version: &str, listed: &str) {
    let script = format!(
        r#"#!/usr/bin/python3
import json,sys,os
if '--version' in sys.argv:
 print({version:?});sys.exit(0)
if sys.argv[1:]==['models','--json','--no-extensions']:
 print(json.dumps({{'models':[{{'provider':'fixture','id':{listed:?},'selector':'fixture/'+{listed:?},'name':'Model','thinking':['high','xhigh','low'],'input':['text']}}]}}));sys.exit(0)
assert sys.argv[1:3]==['--mode','rpc'] and '--no-tools' in sys.argv
assert sys.argv[sys.argv.index('--model')+1]=='crewroom-probe/probe'
print(json.dumps({{'type':'ready','protocolVersion':1,'supportedProtocolVersions':[1,2]}}),flush=True)
sessions=sys.argv[sys.argv.index('--session-dir')+1]
for line in sys.stdin:
 v=json.loads(line)
 data={{'negotiate_protocol':{{'protocolVersion':2}},'get_state':{{'sessionFile':os.path.join(sessions,'s.jsonl')}},'set_host_tools':{{'toolNames':[]}}}}[v['type']]
 print(json.dumps({{'id':v['id'],'type':'response','command':v['type'],'success':True,'data':data}}),flush=True)
"#
    );
    // The install replaces atomically, so the path never names a
    // half-written executable, and this process never holds its writer.
    crewroom_test_support::install_executable(path, script);
}

/// FOR-69: when the harness self-updates between admission and launch and
/// the new executable passes its re-probe, the launch proceeds with the new
/// identity and probe receipt recorded on the attempt; route, offering and
/// effort stay frozen. A new executable that no longer lists the admitted
/// model holds the launch with that reason.
#[tokio::test]
async fn passing_reprobe_of_a_changed_executable_lets_the_launch_proceed() {
    let (root, mut config, roles) = fixture();
    let helpers =
        crate::test_binaries::ensure(&crate::test_binaries::profile_dir_of_current_test());
    let sources = config.discovery_sources(&helpers.process, &json!({"PATH":"/usr/bin:/bin"}));
    config.discovery = Some(
        crate::model_catalog::store::Catalog::open(&root.path().join("catalog"), sources, || {})
            .unwrap(),
    );
    let role = roles.iter().find(|r| r["id"] == "coordinator").unwrap();
    let model = config.assignment_model(role).unwrap();
    let admitted_sha = model["selection"]["identity"]["binarySha256"].clone();
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
    let attempt_id = admitted["attempt"]["id"].as_str().unwrap();

    // Self-update to a newer unlisted release that still lists the model.
    let binary = root.path().join("omp");
    simulated_omp(&binary, "omp/19.0.1", "model");
    assert!(config.verify_frozen_catalog_route(&model).is_err());
    let (next, receipt) = config
        .hold_changed_executable(&model)
        .await
        .unwrap()
        .expect("a changed executable is re-probed");
    let identity = &next["selection"]["identity"];
    assert_eq!(identity["version"], "omp/19.0.1");
    assert_eq!(identity["compatibility"]["basis"], "probed");
    assert_eq!(
        identity["compatibility"]["probe"]["inferenceRequested"],
        false
    );
    assert_eq!(
        identity["binarySha256"],
        json!(discovery::binary_digest(&binary).unwrap().1)
    );
    assert_eq!(receipt["admittedVersion"], "omp/18.4.12");
    assert_eq!(receipt["admittedBinarySha256"], admitted_sha);
    assert_eq!(receipt["version"], "omp/19.0.1");
    assert_eq!(receipt["compatibility"], "compatible (probed 19.0.1)");
    // FOR-73: the launch runs with the new version's capability record, and
    // its fallbacks take effect: steering is held and resume is degraded.
    assert_eq!(
        model["selection"]["harnessCapabilities"]["basis"],
        "verified"
    );
    assert_eq!(next["selection"]["harnessCapabilities"]["basis"], "probed");
    assert_eq!(
        next["selection"]["harnessCapabilities"]["version"],
        "omp/19.0.1"
    );
    assert!(
        next["selection"]["degradedModes"][0]
            .as_str()
            .unwrap()
            .starts_with("Steering waits until the current turn ends")
    );
    assert_eq!(super::steering_mode(&model), Some("steer"));
    assert_eq!(super::steering_mode(&next), None);
    assert!(super::resumes_session(&model));
    assert!(!super::resumes_session(&next));
    // Only the executable identity changed; the frozen route still verifies.
    for key in [
        "offering",
        "billingRoute",
        "requestedEffort",
        "previewRevision",
    ] {
        assert_eq!(next["selection"][key], model["selection"][key], "{key}");
    }
    assert_eq!(next["resolved"], model["resolved"]);
    config.verify_frozen_catalog_route(&next).unwrap();
    assert!(!next.to_string().contains("secret-canary"));
    // The receipt is retained with the catalog's probes.
    let probe_id = identity["compatibility"]["probe"]["id"].clone();
    assert!(
        config
            .discovery
            .as_ref()
            .unwrap()
            .snapshot()
            .unwrap()
            .probes
            .iter()
            .any(|p| json!(p.id) == probe_id)
    );

    // Recorded on the attempt, which the launch then uses.
    let recorded = store
        .record_attempt_reprobe(attempt_id, &next, &receipt)
        .unwrap();
    assert_eq!(recorded["model"], next);
    assert_eq!(recorded["reprobes"][0]["version"], "omp/19.0.1");
    assert_eq!(recorded["reprobes"][0]["admittedVersion"], "omp/18.4.12");
    assert!(recorded["reprobes"][0]["at"].is_string());
    // The recorded selection is now current: no further re-probe.
    assert_eq!(config.hold_changed_executable(&next).await.unwrap(), None);
    // The store refuses a re-probe that changes anything but the identity.
    let mut rerouted = next.clone();
    rerouted["selection"]["requestedEffort"] = json!("low");
    assert!(
        store
            .record_attempt_reprobe(attempt_id, &rerouted, &receipt)
            .is_err()
    );

    // A further update that drops the admitted model holds the launch.
    simulated_omp(&binary, "omp/19.0.2", "other");
    let held = config
        .hold_changed_executable(&next)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        held.contains("Launch held: the OMP executable changed after admission (omp/19.0.1)"),
        "{held}"
    );
    assert!(held.contains("compatible (probed 19.0.2)"), "{held}");
    assert!(held.contains("no longer lists Example"), "{held}");
}

/// FOR-71: resume and process replacement launch a replacement attempt that
/// carries the frozen selection, and every launch (`launch_assignments`, used
/// by start, resume and replacement) compares the executable with that
/// recorded identity and probe receipt. A changed executable is re-probed and
/// a failure holds with the reason; a receipt that no longer satisfies the
/// probe contract is re-probed even when the executable is unchanged; an
/// admitted version on the known-broken list is held.
#[tokio::test]
async fn replacement_attempts_reprobe_changed_executables_and_stale_receipts() {
    use crate::model_catalog::known_broken::KnownBroken;
    let (root, mut config, roles) = fixture();
    let helpers =
        crate::test_binaries::ensure(&crate::test_binaries::profile_dir_of_current_test());
    let sources = config.discovery_sources(&helpers.process, &json!({"PATH":"/usr/bin:/bin"}));
    config.discovery = Some(
        crate::model_catalog::store::Catalog::open(&root.path().join("catalog"), sources, || {})
            .unwrap(),
    );
    let role = roles.iter().find(|r| r["id"] == "coordinator").unwrap();
    let model = config.assignment_model(role).unwrap();
    let store = Store::open(&root.path().join("store.sqlite")).unwrap();
    let task = store
        .create_task(&json!({"title":"Replacement fixture","description":""}))
        .unwrap();
    let run = store
        .queue_refinement(
            task["id"].as_str().unwrap(),
            &json!({"revision":1,"purpose":"explore"}),
        )
        .unwrap();
    let run_id = run["id"].as_str().unwrap();
    let admitted=store.admit_assignment(run_id,&json!({"name":"Coordinator","coordinator":true,"role":role,"model":model,"input":"Inspect"})).unwrap();
    let first = admitted["attempt"]["id"].as_str().unwrap();
    store.claim_attempt(first).unwrap();
    store
        .record_attempt_identity(
            first,
            &json!({"hostPid":2_000_000_001_u32,"hostUrl":"http://127.0.0.1:49999","pane":"%999","sessionId":"retained","ompPid":2_000_000_002_u32}),
        )
        .unwrap();
    store
        .record_ownership(first, true, Some("Fixture process stopped"))
        .unwrap();
    store.set_desired_state(run_id, "paused").unwrap();
    store.begin_pause(run_id, None).unwrap();
    store.finish_pause(run_id).unwrap();
    // Resume replaces the stopped attempt; the replacement keeps the frozen
    // selection, including the executable identity recorded at admission.
    let replacement = store
        .replace_attempt(
            admitted["assignment"]["id"].as_str().unwrap(),
            &json!({"automatic":false}),
        )
        .unwrap();
    assert_eq!(replacement["state"], "admitted");
    assert_eq!(replacement["previousAttemptId"], first);
    assert_eq!(replacement["model"], model);
    assert_eq!(config.hold_changed_executable(&model).await.unwrap(), None);

    // The harness updated itself while the run was paused and the new
    // executable fails its re-probe: the replacement is held with the reason.
    let binary = root.path().join("omp");
    crewroom_test_support::install_executable(&binary, "#!/bin/sh\nexit 3\n");
    let held = config
        .hold_changed_executable(&replacement["model"])
        .await
        .unwrap_err()
        .to_string();
    assert!(
        held.contains("Launch held: the OMP executable changed after admission (omp/18.4.12)"),
        "{held}"
    );
    assert!(held.contains("re-probe failed"), "{held}");

    // A passing newer release is recorded on the replacement attempt.
    simulated_omp(&binary, "omp/19.1.0", "model");
    let (next, receipt) = config
        .hold_changed_executable(&replacement["model"])
        .await
        .unwrap()
        .expect("a changed executable is re-probed on replacement");
    let recorded = store
        .record_attempt_reprobe(replacement["id"].as_str().unwrap(), &next, &receipt)
        .unwrap();
    assert_eq!(recorded["reprobes"][0]["version"], "omp/19.1.0");
    assert_eq!(
        next["selection"]["identity"]["compatibility"]["probe"]["contract"],
        crate::model_catalog::compatibility::CONTRACT
    );
    assert_eq!(config.hold_changed_executable(&next).await.unwrap(), None);

    // Same executable, but its recorded receipt predates the probe contract:
    // it is re-probed (identical content, new receipt) instead of trusted.
    let mut stale = next.clone();
    stale["selection"]["identity"]["compatibility"]["probe"]["contract"] =
        json!("harness-compatibility-probe-v1");
    let (renewed, receipt) = config
        .hold_changed_executable(&stale)
        .await
        .unwrap()
        .expect("a stale receipt is re-probed");
    assert_eq!(receipt["binarySha256"], receipt["admittedBinarySha256"]);
    assert_eq!(
        renewed["selection"]["identity"]["compatibility"]["probe"]["contract"],
        crate::model_catalog::compatibility::CONTRACT
    );

    // An admitted version later added to the known-broken list is held.
    let list = [KnownBroken {
        harness: Harness::Omp,
        from: [19, 1, 0],
        through: [19, 1, 9],
        reason: "Synthetic: host tool results are lost after resume.",
        reviewed: "test fixture",
    }];
    let reason = known_broken_hold(&Harness::Omp, "omp/19.1.0", &list).unwrap();
    assert!(
        reason.starts_with("Launch held: OMP omp/19.1.0"),
        "{reason}"
    );
    assert!(reason.contains("host tool results are lost"), "{reason}");
    assert_eq!(known_broken_hold(&Harness::Omp, "omp/19.2.0", &list), None);
    assert_eq!(
        known_broken_hold(&Harness::Codex, "omp/19.1.0", &list),
        None
    );
}

/// A configured Codex harness whose listed model is `native-model`, with the
/// given login file contents. Returns the config with its catalog reopened.
fn codex_catalog(login: &str) -> (tempfile::TempDir, RuntimeConfig, Vec<Value>) {
    let (root, mut config, roles) = fixture();
    let auth = root.path().join("auth.json");
    write_private(&auth, login.as_bytes()).unwrap();
    config.codex = json!({"binary":root.path().join("omp"),"authFile":auth});
    config.catalog = vec![
        json!({"adapter":"codex","provider":"codex","id":"native-model","api":"codex-app-server","tools":true,"inputs":["text"],"efforts":["high","xhigh","low"]}),
    ];
    let mut snapshot = config.discovery.as_ref().unwrap().snapshot().unwrap();
    let mut native = snapshot.harnesses[0].clone();
    native.harness = Harness::Codex;
    native.identity.as_mut().unwrap().version = "codex-cli 0.162.0".into();
    native.configuration_revision = Some(config.catalog_configuration(&Harness::Codex));
    let offering = &mut native.offerings[0];
    offering.harness = Harness::Codex;
    offering.provider = "codex".into();
    offering.selector = "native-model".into();
    offering.key = offering_key(&Harness::Codex, "codex", "native-model");
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
    (root, config, roles)
}
fn select(config: &mut RuntimeConfig, key: &str) {
    for job in ["code", "judgment", "review", "fast"] {
        config.policy["jobs"][job] = json!(key);
    }
}
const SUBSCRIPTION_AND_KEY: &str = r#"{"auth_mode":"chatgpt","OPENAI_API_KEY":"api-secret-canary","tokens":{"access_token":"account-secret-canary","account_id":"account-one"}}"#;

#[test]
fn a_model_on_both_billing_routes_is_two_offerings_and_needs_an_explicit_choice() {
    let (_root, mut config, roles) = codex_catalog(SUBSCRIPTION_AND_KEY);
    let role = roles.iter().find(|r| r["id"] == "coordinator").unwrap();
    let subscription = routed_key(
        &Harness::Codex,
        "codex",
        "native-model",
        BillingRoute::Subscription,
    );
    let api = routed_key(&Harness::Codex, "codex", "native-model", BillingRoute::Api);
    let snapshot = config.discovery.as_ref().unwrap().snapshot().unwrap();
    let routes = config.catalog_routes(&snapshot, false).unwrap();
    let codex: Vec<_> = routes
        .iter()
        .filter(|r| r.offering.harness == Harness::Codex)
        .map(|r| (r.offering.key.clone(), r.offering.billing))
        .collect();
    assert_eq!(
        codex,
        [
            (subscription.clone(), Some(BillingRoute::Subscription)),
            (api.clone(), Some(BillingRoute::Api))
        ]
    );
    let choices = policy::choices(&routes, &[], &config.requirements).unwrap();
    let labels: Vec<_> = choices
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["label"].as_str().unwrap().to_owned())
        .collect();
    for expected in [
        "Codex · Example · subscription",
        "Codex · Example · API",
        "OMP · Example · unknown billing",
    ] {
        assert!(labels.iter().any(|l| l == expected), "{labels:?}");
    }
    assert!(!choices.to_string().contains("secret-canary"));

    // A selection saved without a route cannot pick one of the two.
    select(
        &mut config,
        &offering_key(&Harness::Codex, "codex", "native-model"),
    );
    let error = config.assignment_model(role).unwrap_err().to_string();
    assert!(
        error.contains("Choose one billing route explicitly"),
        "{error}"
    );

    // The API route is listed but never admitted, and never replaced by the
    // subscription route of the same model.
    select(&mut config, &api);
    let error = config.assignment_model(role).unwrap_err().to_string();
    assert!(
        error.contains("only with its ChatGPT subscription login"),
        "{error}"
    );

    select(&mut config, &subscription);
    let model = config.assignment_model(role).unwrap();
    assert_eq!(model["selection"]["billingRoute"], "subscription");
    assert_eq!(model["selection"]["offering"]["billing"], "subscription");
    assert_eq!(model["selection"]["offering"]["key"], subscription);
    config.verify_frozen_catalog_route(&model).unwrap();
    assert!(!model.to_string().contains("secret-canary"));
}

#[test]
fn admitted_billing_route_survives_restart_and_never_falls_back_at_launch() {
    let (root, mut config, roles) = codex_catalog(SUBSCRIPTION_AND_KEY);
    let role = roles.iter().find(|r| r["id"] == "coordinator").unwrap();
    let subscription = routed_key(
        &Harness::Codex,
        "codex",
        "native-model",
        BillingRoute::Subscription,
    );
    select(&mut config, &subscription);
    let preview = config.preview_model(role).unwrap();
    assert_eq!(preview["selection"]["billingRoute"], "subscription");
    let model = config.assignment_model(role).unwrap();
    let store = Store::open(&root.path().join("store.sqlite")).unwrap();
    let task = store
        .create_task(&json!({"title":"Billing fixture","description":""}))
        .unwrap();
    let run = store
        .queue_refinement(
            task["id"].as_str().unwrap(),
            &json!({"revision":1,"purpose":"explore"}),
        )
        .unwrap();
    let admitted = store.admit_assignment(run["id"].as_str().unwrap(),&json!({"name":"Coordinator","coordinator":true,"role":role,"model":model,"input":"Inspect"})).unwrap();
    drop(store);
    let store = Store::open(&root.path().join("store.sqlite")).unwrap();
    let restored = store
        .get_assignment(admitted["assignment"]["id"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        restored["model"]["selection"]["billingRoute"],
        "subscription"
    );
    assert_eq!(
        restored["model"]["selection"]["offering"]["billing"],
        "subscription"
    );
    config
        .verify_frozen_catalog_route(&restored["model"])
        .unwrap();

    // The login switches to API-key billing: the frozen subscription route is
    // refused at launch and the saved choice does not move to another route.
    let auth = root.path().join("auth.json");
    std::fs::remove_file(&auth).unwrap();
    write_private(
        &auth,
        br#"{"auth_mode":"apikey","OPENAI_API_KEY":"api-secret-canary"}"#,
    )
    .unwrap();
    let error = config
        .verify_frozen_catalog_route(&restored["model"])
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("Admitted billing route (subscription)"),
        "{error}"
    );
    assert!(error.contains("Re-admission is required"), "{error}");
    assert!(!error.contains("secret-canary"));
    assert!(config.assignment_model(role).is_err());
}

#[test]
fn omp_gateway_provider_billing_is_unknown_and_recorded() {
    let (_root, config, roles) = fixture();
    let role = roles.iter().find(|r| r["id"] == "coordinator").unwrap();
    // Saved before billing routes existed; the only route is unknown.
    let model = config.assignment_model(role).unwrap();
    assert_eq!(model["selection"]["billingRoute"], "unknown");
    assert_eq!(
        model["selection"]["offering"]["key"],
        routed_key(&Harness::Omp, "fixture", "model", BillingRoute::Unknown)
    );
    let summary = config.billing_sources().summary(&Harness::Omp);
    assert_eq!(summary[0]["provider"], "fixture");
    assert_eq!(summary[0]["detection"]["routes"], json!(["unknown"]));
    assert!(!summary.to_string().contains("secret-canary"));
}

/// FOR-79: a project's model override replaces the workspace budget at
/// admission, the preview must match the project's resolution, and an
/// admitted assignment keeps its captured model after the override changes.
#[test]
fn project_model_overrides_resolve_at_admission_and_never_alter_captured_models() {
    use crewroom_core::project_settings::effective_roles;
    let (root, config, roles) = fixture();
    let implementer = roles.iter().find(|r| r["id"] == "implementer").unwrap();
    let workspace = config.assignment_model(implementer).unwrap();
    assert_eq!(workspace["selection"]["requestedEffort"], "high");

    let settings = json!({"workspace":{},"projects":{"alpha":{"models.budget":"small"}}});
    let resolved = effective_roles(&roles, &settings, "alpha");
    let role = resolved.iter().find(|r| r["id"] == "implementer").unwrap();
    let model = config.assignment_model(role).unwrap();
    assert_eq!(model["selection"]["requestedEffort"], "low");
    assert_eq!(model["projectOverrides"], json!({"budget":"small"}));
    // A workspace preview does not admit a project-resolved assignment.
    assert!(
        crate::model_catalog::policy::verify_preview(
            &model,
            &workspace["selection"]["previewRevision"]
        )
        .is_err()
    );
    crate::model_catalog::policy::verify_preview(&model, &model["selection"]["previewRevision"])
        .unwrap();
    // Another project inherits the workspace budget.
    let other = effective_roles(&roles, &settings, "beta");
    let inherited = other.iter().find(|r| r["id"] == "implementer").unwrap();
    assert_eq!(
        config.assignment_model(inherited).unwrap()["selection"]["requestedEffort"],
        "high"
    );

    let store = Store::open(&root.path().join("store.sqlite")).unwrap();
    let task = store
        .create_task(&json!({"title":"Captured model","description":""}))
        .unwrap();
    let run = store
        .queue_refinement(
            task["id"].as_str().unwrap(),
            &json!({"revision":1,"purpose":"explore"}),
        )
        .unwrap();
    let admitted = store
        .admit_assignment(
            run["id"].as_str().unwrap(),
            &json!({"name":"Implementer","coordinator":true,"role":role,"model":model,"input":"Inspect"}),
        )
        .unwrap();
    // The override changes; the next admission resolves the new value, the
    // admitted assignment keeps what it captured.
    let changed = json!({"workspace":{},"projects":{"alpha":{"models.budget":"large"}}});
    let next = effective_roles(&roles, &changed, "alpha");
    let next = next.iter().find(|r| r["id"] == "implementer").unwrap();
    assert_eq!(
        config.assignment_model(next).unwrap()["selection"]["requestedEffort"],
        "high"
    );
    let saved = store
        .get_assignment(admitted["assignment"]["id"].as_str().unwrap())
        .unwrap();
    assert_eq!(saved["model"]["selection"]["requestedEffort"], "low");
    assert_eq!(
        saved["role"]["projectSettings"]["models"]["budget"],
        "small"
    );
}

/// FOR-79: operator instructions resolved for the project reach the prompt
/// from the captured role, labelled with their source.
#[test]
fn captured_project_instructions_reach_the_assignment_prompt() {
    let (_root, _config, roles) = fixture();
    let settings = json!({"workspace":{"instructions":"Workspace rule."},"projects":{"alpha":{"instructions":"Alpha rule."}}});
    for (project, expected) in [
        (
            "alpha",
            "Operator project instructions (Settings): Alpha rule.",
        ),
        (
            "beta",
            "Operator workspace instructions (Settings): Workspace rule.",
        ),
    ] {
        let resolved = crewroom_core::project_settings::effective_roles(&roles, &settings, project);
        let role = resolved.iter().find(|r| r["id"] == "coordinator").unwrap();
        let assignment = json!({"name":"Coordinator","coordinator":true,"role":role,"input":"Inspect","brief":{"criteria":[],"dependsOn":[]}});
        let run = json!({"purpose":"coding","taskSnapshot":{"criteria":[]},"taskRevision":1});
        let prompt = configuration::assignment_prompt(
            &run,
            &assignment,
            &assignment,
            std::slice::from_ref(&assignment),
            None,
            false,
        )
        .unwrap();
        assert!(prompt.contains(expected), "{prompt}");
    }
}

/// FOR-79: `list_roles` lists the roles the run's project may assign, in
/// both directions: a project enabling a workspace-disabled role lists it,
/// a project disabling a role hides it, and other projects inherit.
#[test]
fn coordinator_role_listing_follows_the_projects_role_overrides() {
    use crewroom_core::{capacity::run_project, project_settings::effective_roles};
    let (_root, _config, mut roles) = fixture();
    for role in &mut roles {
        if role["id"] == "sre" {
            role["enabled"] = json!(false);
        }
    }
    let settings =
        json!({"workspace":{},"projects":{"alpha":{"roles.sre":true,"roles.reviewer":false}}});
    let listed = |project: &str| -> Vec<String> {
        let run = json!({"taskSnapshot":{"repositoryId":project}});
        let resolved = effective_roles(&roles, &settings, &run_project(&run));
        super::super::tools::listed_roles(&resolved)
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap().to_owned())
            .collect()
    };
    let alpha = listed("alpha");
    assert!(alpha.contains(&"sre".to_owned()), "{alpha:?}");
    assert!(!alpha.contains(&"reviewer".to_owned()), "{alpha:?}");
    let beta = listed("beta");
    assert!(!beta.contains(&"sre".to_owned()), "{beta:?}");
    assert!(beta.contains(&"reviewer".to_owned()), "{beta:?}");
}
