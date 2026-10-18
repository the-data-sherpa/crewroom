use crewroom_runtime::{
    host_journal::{
        EVENT_PAGE_BYTES, EventJournal, InputJournal, MAX_RECORD_BYTES, parse_saved_inputs,
        saved_inputs,
    },
    host_protocol::{HOST_INPUT_LIMIT, ToolCatalog, ToolRoute, parse_request, redact},
};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
};

fn root() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().canonicalize().unwrap();
    (temp, path)
}
fn write(path: &Path, bytes: &[u8]) {
    let mut f = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    f.write_all(bytes).unwrap();
    f.sync_all().unwrap();
}

#[test]
fn saved_inputs_merge_in_first_key_order_and_never_regress_terminal_to_received() {
    let text = concat!(
        "{\"id\":\"a\",\"message\":\"hello\",\"disposition\":\"accepted\",\"extra\":1}\n",
        "{\"id\":\"b\",\"message\":\"second\",\"disposition\":\"accepted\"}\n",
        "{\"id\":\"a\",\"disposition\":\"completed\",\"extra\":2}\n",
        "\n{\"id\":\"a\",\"disposition\":\"received\",\"extra\":3}"
    );
    let values = parse_saved_inputs(text.as_bytes()).unwrap();
    assert_eq!(
        values,
        vec![
            json!({"id":"a","message":"hello","disposition":"completed","extra":2}),
            json!({"id":"b","message":"second","disposition":"accepted"})
        ]
    );
    let (_temp, root) = root();
    assert!(saved_inputs(&root).unwrap().is_empty());
    write(&root.join("inputs.jsonl"), text.as_bytes());
    assert_eq!(saved_inputs(&root).unwrap(), values);
    assert!(InputJournal::open(&root).is_err()); // Read-only reconstruction accepts the final complete JSON; a writer needs the append delimiter.
    for corrupt in [
        b"{".as_slice(),
        b"{}",
        b"null",
        b"{\"id\":1,\"disposition\":\"received\"}",
        b"{\"id\":\"a\"}",
        b"\r\n",
        b"{\"id\":\"a\",\"disposition\":\"accepted\"}\n{",
        b"\xff",
    ] {
        write(&root.join("inputs.jsonl"), corrupt);
        assert!(saved_inputs(&root).is_err(), "{corrupt:?}");
    }
}

#[test]
fn input_admission_is_keyed_and_unknown_dispatch_cannot_be_replayed() {
    let (_temp, root) = root();
    let mut journal = InputJournal::open(&root).unwrap();
    journal.register("busy", "original").unwrap();
    assert!(journal.register("busy", "changed").is_err());
    assert!(journal.accept("busy", "original").unwrap());
    assert_eq!(saved_inputs(&root).unwrap()[0]["disposition"], "accepted");
    assert!(journal.accept("busy", "original").is_err());
    assert!(journal.accept("busy", "changed").is_err());
    journal.completed("busy", "completed").unwrap();
    journal.received("busy").unwrap();
    assert_eq!(journal.disposition("busy").unwrap(), Some("completed"));
    assert!(!journal.accept("busy", "original").unwrap());
    assert!(journal.accept("unknown", "untouched").unwrap());
    journal.completed("unregistered", "error").unwrap();
    assert_eq!(journal.disposition("unregistered").unwrap(), None);
    assert!(journal.completed("busy", "received").is_err());
    drop(journal);
    let mut journal = InputJournal::open(&root).unwrap();
    assert!(!journal.accept("busy", "original").unwrap());
    assert!(journal.accept("unknown", "untouched").is_err());
    assert!(journal.accept("unknown", "changed").is_err());
    assert_eq!(saved_inputs(&root).unwrap().len(), 2);
}

#[test]
fn event_pages_are_nonconsuming_bounded_and_reopen_without_losing_sequence() {
    let (_temp, root) = root();
    let mut events = EventJournal::open(&root).unwrap();
    for i in 1..=350 {
        assert_eq!(events.append(json!({"type":"event","value":i})).unwrap(), i);
    }
    let first = events.page(0.0).unwrap();
    assert_eq!(first.len(), 128);
    assert_eq!(events.page(0.0).unwrap(), first);
    assert_eq!(events.page(-1.0).unwrap(), first);
    assert_eq!(events.page(f64::NAN).unwrap(), first);
    assert!(events.page(f64::INFINITY).unwrap().is_empty());
    assert_eq!(events.page(128.9).unwrap()[0]["sequence"], 129);
    let last = events.page(256.0).unwrap();
    assert_eq!(last.len(), 94);
    assert_eq!(last.last().unwrap()["sequence"], 350);
    assert!(events.page(350.0).unwrap().is_empty());
    drop(events);
    let mut events = EventJournal::open(&root).unwrap();
    assert_eq!(events.sequence(), 350);
    assert_eq!(events.page(0.0).unwrap(), first);
    assert_eq!(events.append(json!({"type":"resumed"})).unwrap(), 351);
    assert_eq!(events.page(350.0).unwrap()[0]["event"]["type"], "resumed");
}

#[test]
fn completed_large_events_survive_page_budget_and_multiple_observers() {
    let (_temp, root) = root();
    let mut events = EventJournal::open(&root).unwrap();
    let large = json!({"type":"host_tool_call","arguments":{"content":"中".repeat(EVENT_PAGE_BYTES/3+200)}});
    events.append(large.clone()).unwrap();
    for _ in 0..3 {
        events
            .append(json!({"type":"message_end","text":"a".repeat(EVENT_PAGE_BYTES/2)}))
            .unwrap();
    }
    for _ in 0..4 {
        let page = events.page(0.0).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0]["event"], large);
        assert_eq!(events.page(1.0).unwrap().len(), 1);
    }
    drop(events);
    let mut events = EventJournal::open(&root).unwrap();
    assert_eq!(events.page(0.0).unwrap()[0]["event"], large);
}

#[test]
fn uncertain_appends_and_changed_inodes_cannot_be_acknowledged_or_read_as_empty() {
    for damage in [
        "truncate",
        "append",
        "replace",
        "same_length",
        "permissions",
    ] {
        let (_temp, root) = root();
        let mut events = EventJournal::open(&root).unwrap();
        events
            .append(json!({"type":"message_end","text":"safe"}))
            .unwrap();
        let path = root.join("events.jsonl");
        match damage {
            "truncate" => {
                OpenOptions::new()
                    .write(true)
                    .open(&path)
                    .unwrap()
                    .set_len(0)
                    .unwrap();
            }
            "append" => {
                OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .unwrap()
                    .write_all(b"{")
                    .unwrap();
            }
            "replace" => {
                fs::rename(&path, root.join("old")).unwrap();
                write(&path, b"");
            }
            "same_length" => {
                let text = fs::read_to_string(&path).unwrap().replace("safe", "fake");
                write(&path, text.as_bytes());
            }
            _ => fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap(),
        }
        assert!(events.page(0.0).is_err(), "{damage}");
        assert!(events.append(json!({"type":"new"})).is_err(), "{damage}");
        assert_eq!(events.sequence(), 1);
    }
    let (_temp, root) = root();
    let mut input = InputJournal::open(&root).unwrap();
    input.accept("a", "original").unwrap();
    OpenOptions::new()
        .append(true)
        .open(root.join("inputs.jsonl"))
        .unwrap()
        .write_all(b"{")
        .unwrap();
    assert!(input.received("a").is_err());
    assert!(input.disposition("a").is_err());
    drop(input);
    assert!(InputJournal::open(&root).is_err());
    assert!(saved_inputs(&root).is_err());
}

#[test]
fn concurrent_writers_symlinks_hardlinks_and_corrupt_sequences_are_refused() {
    let (_temp, root) = root();
    let events = EventJournal::open(&root).unwrap();
    assert!(EventJournal::open(&root).is_err());
    drop(events);
    let input = InputJournal::open(&root).unwrap();
    assert!(InputJournal::open(&root).is_err());
    drop(input);
    let path = root.join("events.jsonl");
    fs::remove_file(&path).unwrap();
    symlink(root.join("inputs.jsonl"), &path).unwrap();
    assert!(EventJournal::open(&root).is_err());
    fs::remove_file(&path).unwrap();
    fs::hard_link(root.join("inputs.jsonl"), &path).unwrap();
    assert!(EventJournal::open(&root).is_err());
    fs::remove_file(&path).unwrap();
    for bytes in [
        b"{\"sequence\":2,\"event\":{}}\n".as_slice(),
        b"{\"sequence\":1,\"event\":{}}\n{\"sequence\":1,\"event\":{}}\n",
        b"{\"sequence\":1,\"event\":null}\n",
        b"{\"sequence\":1,\"event\":{}}",
        b"{",
        b"\n",
    ] {
        write(&path, bytes);
        assert!(EventJournal::open(&root).is_err(), "{bytes:?}");
    }
    fs::remove_file(root.join("inputs.jsonl")).unwrap();
    symlink(&path, root.join("inputs.jsonl")).unwrap();
    assert!(saved_inputs(&root).is_err());
}

#[test]
fn oversized_records_are_rejected_without_consuming_a_sequence() {
    let (_temp, root) = root();
    let mut events = EventJournal::open(&root).unwrap();
    assert!(
        events
            .append(json!({"type":"large","text":"x".repeat(MAX_RECORD_BYTES)}))
            .is_err()
    );
    assert_eq!(events.sequence(), 0);
    assert_eq!(events.append(json!({"type":"small"})).unwrap(), 1);
    drop(events);
    let path = root.join("events.jsonl");
    write(&path, b"{");
    OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_len(MAX_RECORD_BYTES as u64 + 1)
        .unwrap();
    assert!(EventJournal::open(&root).is_err());
}

#[test]
fn wire_request_limits_count_utf16_after_complete_utf8_assembly() {
    for (character, count) in [
        ('a', HOST_INPUT_LIMIT - 2),
        ('中', HOST_INPUT_LIMIT - 2),
        ('😀', (HOST_INPUT_LIMIT - 2) / 2),
    ] {
        let text = format!("\"{}\"", character.to_string().repeat(count));
        let bytes = text.as_bytes();
        assert!(parse_request(bytes).is_ok());
        let mut assembled = Vec::new();
        for chunk in bytes.chunks(7) {
            assembled.extend_from_slice(chunk);
        }
        assert_eq!(
            parse_request(&assembled).unwrap(),
            serde_json::from_str::<Value>(&text).unwrap()
        );
        assert!(
            parse_request(
                format!("\"{}{}\"", character.to_string().repeat(count), character).as_bytes()
            )
            .is_err()
        );
    }
    assert!(parse_request(b"\"\xff\"").is_err());
    assert!(parse_request(b"{").is_err());
}

#[test]
fn only_registered_tools_route_and_credentials_are_recursively_removed() {
    let specialist =
        ToolCatalog::new(&json!({"recipients":["coordinator"],"coding":true,"commandEngine":{}}))
            .unwrap();
    assert_eq!(specialist.route("run_check").unwrap(), ToolRoute::Check);
    assert_eq!(specialist.route("run_shell").unwrap(), ToolRoute::Shell);
    assert!(specialist.route("claim_implementation").is_err());
    assert!(specialist.route("steering_receipt").is_err());
    assert!(specialist.route("publish_change").is_err());
    assert_eq!(
        specialist.route("write_file").unwrap(),
        ToolRoute::Backend("coding/edit".into())
    );
    let coordinator = ToolCatalog::new(
        &json!({"recipients":[],"coordinator":true,"coding":true,"commandEngine":{}}),
    )
    .unwrap();
    assert!(coordinator.route("run_shell").is_err());
    assert!(coordinator.route("write_file").is_err());
    assert!(
        ToolCatalog::new(
            &json!({"recipients":[],"coding":true,"research":true,"researchRepository":true})
        )
        .is_err()
    );
    assert!(ToolCatalog::new(&json!({"recipients":[],"coding":"yes"})).is_err());
    assert!(
        ToolCatalog::new(&json!({"recipients":[],"coding":true,"commandEngine":"true"})).is_err()
    );
    assert!(
        ToolCatalog::new(&json!({"recipients":[],"tools":[{"id":"same"},{"id":"same"}]})).is_err()
    );
    assert_eq!(
        redact(
            &json!({"APIKEY":"hidden","nested":[{"authorization":"hidden","text":"before secret after","token":"hidden"}],"count":2}),
            &["secret".into(), "".into()]
        ),
        json!({"nested":[{"text":"before [redacted] after"}],"count":2})
    );
}

#[test]
fn special_files_and_aliased_roots_cannot_block_or_replace_journals() {
    let (_temp, root) = root();
    let fifo = std::ffi::CString::new(root.join("inputs.jsonl").to_str().unwrap()).unwrap();
    // SAFETY: valid NUL-terminated path in this disposable fixture.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert!(saved_inputs(&root).is_err());
    assert!(InputJournal::open(&root).is_err());
    fs::remove_file(root.join("inputs.jsonl")).unwrap();
    fs::create_dir(root.join("events.jsonl")).unwrap();
    assert!(EventJournal::open(&root).is_err());
    fs::remove_dir(root.join("events.jsonl")).unwrap();
    fs::create_dir(root.join("actual")).unwrap();
    symlink(root.join("actual"), root.join("alias")).unwrap();
    assert!(EventJournal::open(&root.join("alias")).is_err());
}

#[test]
fn playbook_tools_are_only_registered_for_delivery_and_research_coordinators() {
    for config in [
        json!({"recipients":[],"coordinator":true,"coding":true}),
        json!({"recipients":[],"coordinator":true,"research":true}),
    ] {
        let tools = ToolCatalog::new(&config).unwrap();
        for name in [
            "playbook_status",
            "propose_playbook",
            "record_playbook_step",
        ] {
            assert!(tools.route(name).is_ok());
        }
    }
    for config in [
        json!({"recipients":[],"coordinator":false,"coding":true}),
        json!({"recipients":[],"coordinator":true,"refinement":true}),
    ] {
        let tools = ToolCatalog::new(&config).unwrap();
        for name in [
            "playbook_status",
            "propose_playbook",
            "record_playbook_step",
        ] {
            assert!(tools.route(name).is_err());
        }
    }
}

#[test]
fn staged_recipients_do_not_change_a_native_assignments_tool_identity() {
    let initial = serde_json::json!({"coordinator":true,"coding":true,"choreography":true,"recipients":["Coordinator"]});
    let mut rotated = initial.clone();
    rotated["recipients"] = serde_json::json!(["Coordinator", "Writer", "Navigator"]);
    assert_eq!(
        crewroom_runtime::host_protocol::host_tools(&initial),
        crewroom_runtime::host_protocol::host_tools(&rotated)
    );
    let legacy = serde_json::json!({"coordinator":true,"coding":true,"recipients":["Coordinator"]});
    assert_ne!(
        crewroom_runtime::host_protocol::host_tools(&initial),
        crewroom_runtime::host_protocol::host_tools(&legacy)
    );
}
