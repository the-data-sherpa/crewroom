//! Exact assignment selection and runner readback. Policy never grants execution itself.
use crate::conflict;
use crewroom_core::Result;
use serde_json::{Value, json};
use std::collections::HashSet;

const EFFORTS: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh", "max"];
fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("")
}
fn selector(value: &Value) -> String {
    format!("{}/{}", text(value, "provider"), text(value, "id"))
}
fn catalog_unique(catalog: &[Value]) -> Result<()> {
    let mut seen = HashSet::new();
    if catalog.iter().any(|item| !seen.insert(selector(item))) {
        return Err(conflict(
            "Model catalog contains ambiguous duplicate selectors. Refresh the catalog before admission.",
        ));
    }
    Ok(())
}
fn contains(list: &Value, value: &str) -> bool {
    list.as_array()
        .is_some_and(|a| a.iter().any(|v| v == value))
}
pub struct Intent<'a> {
    pub model: &'a str,
    pub effort: &'a str,
    pub job: &'a str,
    pub budget: Option<&'a str>,
    pub warning: Option<&'static str>,
}
pub fn intent<'a>(role: &'a Value, policy: &'a Value) -> Result<Intent<'a>> {
    let (job, warning) = crewroom_core::model_jobs::job(role);
    let jobs = policy["jobs"].is_object();
    let budget = policy["budget"].as_str().unwrap_or("large");
    let inherited = if jobs {
        crewroom_core::model_jobs::effort(budget, job)?
    } else {
        text(&policy["default"], "effort")
    };
    let model = if text(role, "model").is_empty() {
        if jobs {
            text(&policy["jobs"], job)
        } else {
            text(&policy["default"], "model")
        }
    } else {
        text(role, "model")
    };
    let effort = if ["", "default"].contains(&text(role, "effort")) {
        inherited
    } else {
        text(role, "effort")
    };
    if model.is_empty() || effort.is_empty() {
        return Err(conflict(&if jobs {
            format!("Configure a model for the {job} job or set a role override.")
        } else {
            "Configure an explicit default model and effort.".into()
        }));
    }
    Ok(Intent {
        model,
        effort,
        job,
        budget: jobs.then_some(budget),
        warning,
    })
}
/// Role/catalog/policy are trusted parsed configuration; unknown catalog metadata is retained.
pub fn resolve_assignment(
    role: &Value,
    catalog: &[Value],
    policy: &Value,
    requirements: &Value,
) -> Result<Value> {
    if role["enabled"] != true {
        return Err(conflict("Role is disabled."));
    }
    catalog_unique(catalog)?;
    let intent = intent(role, policy)?;
    let (job, warning) = (intent.job, intent.warning);
    let jobs = policy["jobs"].is_object();
    let budget = policy["budget"].as_str().unwrap_or("large");
    let mut model = intent.model;
    let mut effort = intent.effort;
    // Catalog selections require freshness/profile/revision checks at the
    // catalog boundary. The legacy selector resolver must never substitute one.
    if crate::model_catalog::is_selection(model) {
        return Err(conflict(
            "Model catalog selection requires current admission evidence.",
        ));
    }
    let mut capability = catalog.iter().find(|c| selector(c) == model);
    let mut fallback_from = None;
    if capability.is_none()
        && let Some(target) = policy["fallbacks"][model].as_array().and_then(|a| {
            a.iter()
                .find(|t| catalog.iter().any(|c| selector(c) == text(t, "model")))
        })
    {
        fallback_from = Some(model.to_owned());
        model = text(target, "model");
        effort = text(target, "effort");
        capability = catalog.iter().find(|c| selector(c) == model);
    }
    let capability = capability.ok_or_else(|| {
        conflict(&format!(
            "Model is unavailable in the admitted catalog: {model}"
        ))
    })?;
    if requirements.get("allowedModels").is_some()
        && !contains(&requirements["allowedModels"], model)
    {
        return Err(conflict(&format!("Project disallows model {model}.")));
    }
    if !EFFORTS.contains(&effort) || !contains(&capability["efforts"], effort) {
        return Err(conflict(&format!(
            "Unsupported effort {effort} for {model}."
        )));
    }
    if requirements["tools"] == true && capability["tools"] != true {
        return Err(conflict(
            "Required tool capability is unavailable or unverified.",
        ));
    }
    if requirements["inputs"].as_array().is_some_and(|inputs| {
        inputs.iter().any(|v| {
            !v.as_str()
                .is_some_and(|s| contains(&capability["inputs"], s))
        })
    }) {
        return Err(conflict("Required input capability is unavailable."));
    }
    let mut resolved = capability.clone();
    resolved["selector"] = json!(model);
    resolved["effort"] = json!(effort);
    let mut result =
        json!({"requested":{"model":role["model"],"effort":role["effort"]},"resolved":resolved});
    if jobs || role.get("job").is_some() {
        result["job"] = json!(job);
        result["budget"] = if jobs { json!(budget) } else { Value::Null };
        if let Some(warning) = warning {
            result["warning"] = json!(warning);
        }
    }
    if let Some(from) = fallback_from {
        result["fallbackFrom"] = json!(from);
    }
    Ok(result)
}
pub fn resolve_runtime_fallback(
    role: &Value,
    current: &Value,
    catalog: &[Value],
    policy: &Value,
    requirements: &Value,
    tried: &[String],
) -> Result<Option<Value>> {
    if current.get("selection").is_some() {
        return Ok(None);
    }
    catalog_unique(catalog)?;
    if let Some(targets) = policy["fallbacks"][text(&current["resolved"], "selector")].as_array() {
        for target in targets {
            let model = text(target, "model");
            let effort = text(target, "effort");
            if !EFFORTS.contains(&effort)
                || tried.iter().any(|t| t == model)
                || current["resolved"]["selector"] == model
            {
                continue;
            }
            let mut role = role.clone();
            role["model"] = json!(model);
            role["effort"] = json!(effort);
            if let Ok(mut result) = resolve_assignment(&role, catalog, &json!({}), requirements) {
                result["requested"] = current["requested"].clone();
                result["fallbackFrom"] = current["resolved"]["selector"].clone();
                return Ok(Some(result));
            }
        }
    }
    Ok(None)
}
/// Return only schema-declared identity fields. Missing/null thinking means off.
pub fn verify_assignment_identity(plan: &Value, input: &Value) -> Result<Value> {
    let model = &input["model"];
    if !["provider", "id", "api"]
        .iter()
        .all(|k| model[*k].is_string())
        || input["sessionId"].as_str().is_none_or(str::is_empty)
        || !input["isStreaming"].is_boolean()
        || !input["autoCompactionEnabled"].is_boolean()
        || !input["thinkingLevel"].is_null() && !input["thinkingLevel"].is_string()
    {
        return Err(crewroom_core::Error::Invalid(
            "Invalid runner identity.".into(),
        ));
    }
    let effort = input["thinkingLevel"].as_str().unwrap_or("off");
    let expected = &plan["resolved"];
    if ["provider", "id", "api"]
        .iter()
        .any(|k| model[*k] != expected[*k])
        || expected["effort"] != effort
    {
        return Err(conflict(
            "Runner identity or effort mismatch. Assignment cannot execute.",
        ));
    }
    if input["autoCompactionEnabled"] == true {
        return Err(conflict(
            "Automatic compaction must be disabled for this assignment.",
        ));
    }
    if input["isStreaming"] == true {
        return Err(conflict(
            "Runner is already executing; admission requires an idle session.",
        ));
    }
    Ok(
        json!({"model":{"provider":model["provider"],"id":model["id"],"api":model["api"]},"thinkingLevel":effort,"sessionId":input["sessionId"],"isStreaming":false,"autoCompactionEnabled":false}),
    )
}
pub fn isolated_model_settings(plan: &Value) -> Value {
    let selector = text(&plan["resolved"], "selector");
    let effort = text(&plan["resolved"], "effort");
    let roles = [
        "default", "smol", "slow", "vision", "plan", "commit", "tiny", "memory", "task", "advisor",
    ];
    let model_roles: serde_json::Map<String, Value> = roles
        .iter()
        .map(|r| (r.to_string(), json!(format!("{selector}:{effort}"))))
        .collect();
    let fallbacks: serde_json::Map<String, Value> = roles
        .into_iter()
        .chain(["image", "web", "speech", "dictation", "judge"])
        .map(|r| (r.to_string(), json!([])))
        .collect();
    json!({"modelRoles":model_roles,"enabledModels":[selector],"enabledProviders":[plan["resolved"]["provider"]],"defaultThinkingLevel":effort,
        "retry":{"enabled":false,"maxRetries":0,"modelFallback":false,"fallbackChains":fallbacks},
        "async":{"enabled":false},"advisor":{"enabled":false},"prewalk":{"enabled":false},"compaction":{"enabled":false},"contextPromotion":{"enabled":false},
        "recap":{"enabled":false},"memory":{"backend":"off"},"autolearn":{"enabled":false},"magicKeywords":{"enabled":false},"providers":{"anthropic":{"serverSideFallback":false}}})
}

pub async fn configure_assignment(
    rpc: &impl crate::rpc::RpcCommands,
    plan: &Value,
) -> Result<Value> {
    let available = rpc.command("get_available_models", json!({})).await?;
    let models = available["models"]
        .as_array()
        .filter(|models| {
            models
                .iter()
                .all(|m| ["provider", "id", "api"].iter().all(|k| m[*k].is_string()))
        })
        .ok_or_else(|| crewroom_core::Error::Invalid("Invalid runner catalog.".into()))?;
    let expected = &plan["resolved"];
    if !models.iter().any(|m| {
        ["provider", "id", "api"]
            .iter()
            .all(|k| m[*k] == expected[*k])
    }) {
        return Err(conflict(
            "Model is unavailable in the current runner catalog. Refresh the assignment.",
        ));
    }
    rpc.command(
        "set_model",
        json!({"provider":expected["provider"],"modelId":expected["id"]}),
    )
    .await?;
    let levels = rpc
        .command("get_available_thinking_levels", json!({}))
        .await?;
    let levels = levels["levels"]
        .as_array()
        .filter(|levels| levels.iter().all(Value::is_string))
        .ok_or_else(|| crewroom_core::Error::Invalid("Invalid runner effort catalog.".into()))?;
    if !levels.contains(&expected["effort"]) && !(expected["effort"] == "off" && levels.is_empty())
    {
        return Err(conflict(
            "Unsupported effort in the current runner catalog. Assignment cannot execute.",
        ));
    }
    rpc.command("set_thinking_level", json!({"level":expected["effort"]}))
        .await?;
    rpc.command("set_auto_compaction", json!({"enabled":false}))
        .await?;
    rpc.command("set_auto_retry", json!({"enabled":false}))
        .await?;
    verify_assignment_identity(plan, &rpc.command("get_state", json!({})).await?)
}
