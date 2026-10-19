//! FOR-71 Codex protocol diff. The installed Codex describes its app-server
//! protocol offline (`codex app-server generate-json-schema --experimental`,
//! no model call). The probe compares the server requests and notifications
//! it reports with the reviewed baseline below, so a server request Crewroom
//! does not handle fails before a run starts instead of mid-turn.
use super::*;
use std::collections::BTreeSet;

/// Server requests the Codex runner answers (`runner/codex.rs`). Each must
/// still be offered: without them tool calls or token refresh cannot work.
pub const HANDLED_SERVER_REQUESTS: &[&str] =
    &["account/chatgptAuthTokens/refresh", "item/tool/call"];

/// Server requests reported by the verified Codex 0.163.0 with experimental
/// methods included (the launch negotiates `experimentalApi`). Those Crewroom
/// does not answer are not sent under its launch policy (approval `never`,
/// read-only sandbox, no MCP servers, only Crewroom dynamic tools); if one is
/// sent anyway the runner stops the turn as an unadmitted native operation.
/// Reviewed 2026-10-19; changes only with a Crewroom revision.
pub const BASELINE_SERVER_REQUESTS: &[&str] = &[
    "account/chatgptAuthTokens/refresh",
    "applyPatchApproval",
    "attestation/generate",
    "currentTime/read",
    "execCommandApproval",
    "item/commandExecution/requestApproval",
    "item/fileChange/requestApproval",
    "item/permissions/requestApproval",
    "item/tool/call",
    "item/tool/requestUserInput",
    "mcpServer/elicitation/request",
];

/// Notifications the Codex runner acts on. Losing one means a turn could not
/// be observed, so their absence fails the probe.
pub const REQUIRED_NOTIFICATIONS: &[&str] = &[
    "error",
    "item/agentMessage/delta",
    "item/completed",
    "item/reasoning/summaryTextDelta",
    "item/started",
    "model/rerouted",
    "thread/tokenUsage/updated",
    "turn/completed",
    "turn/started",
];

/// Notifications reported by the verified Codex 0.163.0 (experimental
/// included). Other notifications are ignored by the runner, so differences
/// are recorded on the receipt rather than failing the probe.
pub const BASELINE_NOTIFICATIONS: &[&str] = &[
    "account/gatewayOAuth/changed",
    "account/login/completed",
    "account/rateLimits/updated",
    "account/updated",
    "app/list/updated",
    "autoApprovalReview/strictReviewRequired",
    "command/exec/outputDelta",
    "configWarning",
    "deprecationNotice",
    "error",
    "externalAgentConfig/import/completed",
    "externalAgentConfig/import/progress",
    "fs/changed",
    "fuzzyFileSearch/sessionCompleted",
    "fuzzyFileSearch/sessionUpdated",
    "guardianWarning",
    "hook/completed",
    "hook/started",
    "item/agentMessage/delta",
    "item/autoApprovalReview/completed",
    "item/autoApprovalReview/started",
    "item/commandExecution/outputDelta",
    "item/commandExecution/terminalInteraction",
    "item/completed",
    "item/fileChange/outputDelta",
    "item/fileChange/patchUpdated",
    "item/mcpToolCall/progress",
    "item/plan/delta",
    "item/reasoning/summaryPartAdded",
    "item/reasoning/summaryTextDelta",
    "item/reasoning/textDelta",
    "item/started",
    "mcpServer/event/stream/notification",
    "mcpServer/oauthLogin/completed",
    "mcpServer/startupStatus/updated",
    "model/rerouted",
    "model/safetyBuffering/updated",
    "model/verification",
    "modelProvider/authRecoveryCompleted",
    "modelProvider/authRecoveryStarted",
    "process/exited",
    "process/outputDelta",
    "project/changed",
    "remoteControl/status/changed",
    "serverRequest/resolved",
    "skills/changed",
    "thread/archived",
    "thread/attachment/updated",
    "thread/closed",
    "thread/compacted",
    "thread/deleted",
    "thread/environment/connected",
    "thread/environment/disconnected",
    "thread/goal/cleared",
    "thread/goal/updated",
    "thread/name/updated",
    "thread/prediction/updated",
    "thread/project/updated",
    "thread/queue/changed",
    "thread/readState/changed",
    "thread/realtime/closed",
    "thread/realtime/error",
    "thread/realtime/item/completed",
    "thread/realtime/item/started",
    "thread/realtime/item/transcript/delta",
    "thread/realtime/itemAdded",
    "thread/realtime/outputAudio/delta",
    "thread/realtime/sdp",
    "thread/realtime/started",
    "thread/realtime/transcript/delta",
    "thread/realtime/transcript/done",
    "thread/reverted",
    "thread/settings/updated",
    "thread/started",
    "thread/status/changed",
    "thread/tokenUsage/updated",
    "thread/unarchived",
    "turn/completed",
    "turn/diff/updated",
    "turn/moderationMetadata",
    "turn/plan/updated",
    "turn/started",
    "warning",
    "windows/worldWritableWarning",
    "windowsSandbox/setupCompleted",
];

/// Notification differences from the baseline, kept on the probe receipt.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProtocolDiff {
    pub added_notifications: Vec<String>,
    pub removed_notifications: Vec<String>,
}

/// The `method` names of a generated `ServerRequest`/`ServerNotification`
/// schema: each `oneOf` variant names its method in an enum or const.
pub fn methods(schema: &Value) -> std::result::Result<BTreeSet<String>, String> {
    let variants = schema["oneOf"]
        .as_array()
        .filter(|v| !v.is_empty() && v.len() <= 10_000)
        .ok_or("The generated schema has no method variants.")?;
    let mut names = BTreeSet::new();
    for variant in variants {
        let method = &variant["properties"]["method"];
        let values: Vec<&Value> = match (method["enum"].as_array(), method.get("const")) {
            (Some(values), _) => values.iter().collect(),
            (None, Some(value)) => vec![value],
            _ => return Err("A generated schema variant names no method.".into()),
        };
        for value in values {
            let name = value
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 200 && valid_text(s))
                .ok_or("A generated schema method name is invalid.")?;
            names.insert(name.to_owned());
        }
    }
    Ok(names)
}

/// Compare the reported protocol with the baseline. A server request that is
/// neither handled nor in the baseline fails, naming each such method; so does
/// a handled request or required notification that is no longer offered.
pub fn compare(
    requests: &BTreeSet<String>,
    notifications: &BTreeSet<String>,
) -> std::result::Result<ProtocolDiff, String> {
    let unhandled: Vec<_> = requests
        .iter()
        .filter(|m| {
            !HANDLED_SERVER_REQUESTS.contains(&m.as_str())
                && !BASELINE_SERVER_REQUESTS.contains(&m.as_str())
        })
        .map(String::as_str)
        .collect();
    if !unhandled.is_empty() {
        return Err(format!(
            "Codex reports server request{} Crewroom does not handle: {}",
            if unhandled.len() == 1 { "" } else { "s" },
            unhandled.join(", ")
        ));
    }
    let missing: Vec<_> = HANDLED_SERVER_REQUESTS
        .iter()
        .filter(|m| !requests.contains(**m))
        .chain(
            REQUIRED_NOTIFICATIONS
                .iter()
                .filter(|m| !notifications.contains(**m)),
        )
        .copied()
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "Codex no longer offers protocol methods Crewroom relies on: {}",
            missing.join(", ")
        ));
    }
    Ok(ProtocolDiff {
        added_notifications: notifications
            .iter()
            .filter(|m| !BASELINE_NOTIFICATIONS.contains(&m.as_str()))
            .cloned()
            .collect(),
        removed_notifications: BASELINE_NOTIFICATIONS
            .iter()
            .filter(|m| !notifications.contains(**m))
            .map(|m| (*m).to_owned())
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn schema(names: &[&str]) -> Value {
        json!({"oneOf": names.iter().map(|n| json!({"properties":{"method":{"enum":[n]}}})).collect::<Vec<_>>()})
    }
    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }
    #[test]
    fn baseline_lists_are_sorted_unique_and_cover_what_the_runner_uses() {
        for list in [BASELINE_SERVER_REQUESTS, BASELINE_NOTIFICATIONS] {
            assert!(list.windows(2).all(|w| w[0] < w[1]));
        }
        for handled in HANDLED_SERVER_REQUESTS {
            assert!(BASELINE_SERVER_REQUESTS.contains(handled));
        }
        for required in REQUIRED_NOTIFICATIONS {
            assert!(BASELINE_NOTIFICATIONS.contains(required));
        }
    }
    #[test]
    fn the_baseline_passes_with_no_differences() {
        let diff = compare(&set(BASELINE_SERVER_REQUESTS), &set(BASELINE_NOTIFICATIONS)).unwrap();
        assert_eq!(diff, ProtocolDiff::default());
    }
    #[test]
    fn an_unhandled_server_request_fails_naming_the_method() {
        let mut requests = set(BASELINE_SERVER_REQUESTS);
        requests.insert("item/browser/requestNavigation".into());
        let error = compare(&requests, &set(BASELINE_NOTIFICATIONS)).unwrap_err();
        assert!(
            error.contains("does not handle: item/browser/requestNavigation"),
            "{error}"
        );
        requests.insert("account/consent/request".into());
        let error = compare(&requests, &set(BASELINE_NOTIFICATIONS)).unwrap_err();
        assert!(
            error.contains(
                "server requests Crewroom does not handle: account/consent/request, item/browser/requestNavigation"
            ),
            "{error}"
        );
    }
    #[test]
    fn a_missing_handled_request_or_required_notification_fails() {
        let requests = set(&BASELINE_SERVER_REQUESTS[1..]);
        let error = compare(&requests, &set(BASELINE_NOTIFICATIONS)).unwrap_err();
        assert!(
            error.contains("account/chatgptAuthTokens/refresh"),
            "{error}"
        );
        let notifications: BTreeSet<_> = set(BASELINE_NOTIFICATIONS)
            .into_iter()
            .filter(|n| n != "turn/completed")
            .collect();
        let error = compare(&set(BASELINE_SERVER_REQUESTS), &notifications).unwrap_err();
        assert!(error.contains("relies on: turn/completed"), "{error}");
    }
    #[test]
    fn notification_changes_are_recorded_without_failing() {
        let mut notifications: BTreeSet<_> = set(BASELINE_NOTIFICATIONS)
            .into_iter()
            .filter(|n| n != "thread/realtime/sdp")
            .collect();
        notifications.insert("thread/memory/updated".into());
        // Dropping a baseline request Crewroom does not handle is harmless.
        let requests: BTreeSet<_> = set(BASELINE_SERVER_REQUESTS)
            .into_iter()
            .filter(|r| r != "currentTime/read")
            .collect();
        let diff = compare(&requests, &notifications).unwrap();
        assert_eq!(diff.added_notifications, ["thread/memory/updated"]);
        assert_eq!(diff.removed_notifications, ["thread/realtime/sdp"]);
    }
    #[test]
    fn methods_are_read_from_enum_or_const_and_malformed_schemas_are_refused() {
        let mut value = schema(&["turn/started", "item/tool/call"]);
        value["oneOf"]
            .as_array_mut()
            .unwrap()
            .push(json!({"properties":{"method":{"const":"warning"}}}));
        assert_eq!(
            methods(&value).unwrap(),
            set(&["item/tool/call", "turn/started", "warning"])
        );
        for bad in [
            json!({}),
            json!({"oneOf":[]}),
            json!({"oneOf":[{"properties":{}}]}),
            json!({"oneOf":[{"properties":{"method":{"enum":[7]}}}]}),
            json!({"oneOf":[{"properties":{"method":{"enum":[""]}}}]}),
        ] {
            assert!(methods(&bad).is_err(), "{bad}");
        }
    }
}
