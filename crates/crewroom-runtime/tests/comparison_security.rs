//! The actual backend permission predicate is tested independently of tool visibility.
use crewroom_runtime::host_protocol::{
    ToolCatalog, ToolRoute, backend_routes, host_tools, permits_backend_route,
};
use serde_json::{Value, json};

fn config(coordinator: bool) -> Value {
    // Deliberately spoof all older capability flags. Purpose remains authoritative.
    json!({"purpose":"evaluation","coordinator":coordinator,"coding":true,"publication":true,"jira":true,"reporter":true,"researchCoordinatorReporter":true,"choreography":true,"research":true,"researchRepository":true,"commandEngine":{"socket":"/fixture.sock"},"tools":[{"id":"arbitrary"}],"recipients":["Peer"]})
}

#[test]
fn evaluation_purpose_denies_direct_and_spoofed_coding_catalog_publication_and_shell_routes() {
    for coordinator in [true, false] {
        let config = config(coordinator);
        for route in [
            "coding/check-prepare",
            "coding/check-finish",
            "coding/shell",
            "coding/complete",
            "coding/freeze",
            "coding/review",
            "coding/checkpoint",
            "coding/choreography-propose",
            "publication/prepare",
            "publication/status",
            "jira/report",
            "jira/updates",
            "agent-roles",
            "agent-team",
            "learning/propose",
            "playbook/propose",
            "research/complete",
            "intake/propose",
            "commands/run",
            "coding/read",
            "coding/search",
        ] {
            assert!(
                !permits_backend_route(&config, route),
                "{coordinator}: {route}"
            );
        }
        for tool in host_tools(&config) {
            let routes = backend_routes(&config);
            let route = routes.get(tool["name"].as_str().unwrap()).unwrap();
            assert!(
                permits_backend_route(&config, route),
                "advertised tool must be usable"
            );
        }
    }
}

#[test]
fn coordinator_and_worker_cannot_spoof_each_others_capabilities() {
    let coordinator = config(true);
    let worker = config(false);
    for route in [
        "evaluation/write",
        "evaluation/edit",
        "evaluation/check",
        "evaluation/handoff",
    ] {
        assert!(!permits_backend_route(&coordinator, route));
        assert!(permits_backend_route(&worker, route));
    }
    for route in [
        "evaluation/playbook",
        "evaluation/playbook-step",
        "steering-receipt",
    ] {
        assert!(permits_backend_route(&coordinator, route));
        assert!(!permits_backend_route(&worker, route));
    }
    for route in ["evaluation/read", "evaluation/search", "agent-message"] {
        assert!(permits_backend_route(&coordinator, route));
        assert!(permits_backend_route(&worker, route));
    }
}

#[test]
fn unknown_or_malformed_purposes_fail_closed_even_with_all_old_flags() {
    for purpose in [
        json!("future-purpose"),
        json!(""),
        json!(null),
        json!(17),
        json!({"coding":true}),
    ] {
        let mut config = config(false);
        config["purpose"] = purpose;
        assert!(host_tools(&config).is_empty());
        for route in [
            "coding/read",
            "coding/check-prepare",
            "coding/check-finish",
            "coding/shell",
            "evaluation/check",
            "publication/prepare",
            "agent-message",
        ] {
            assert!(!permits_backend_route(&config, route));
        }
    }
}

#[test]
fn evaluation_checks_use_backend_receipts_without_host_local_coding_check_dispatch() {
    let catalog = ToolCatalog::new(&config(false)).unwrap();
    assert!(
        matches!(catalog.route("run_check").unwrap(),ToolRoute::Backend(route) if route=="evaluation/check")
    );
    assert!(catalog.route("run_shell").is_err());
    assert!(catalog.route("run_command").is_err());
    assert!(catalog.route("publish_change").is_err());
    assert!(catalog.route("handoff_assignment").is_ok());
    let coding = json!({"purpose":"coding","coordinator":false,"coding":true,"recipients":[],"commandEngine":{"socket":"/fixture.sock"}});
    assert!(permits_backend_route(&coding, "coding/check-prepare"));
    assert!(permits_backend_route(&coding, "coding/check-finish"));
}
