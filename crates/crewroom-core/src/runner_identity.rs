//! Additive runner identity. Untagged historical models retain their OMP route.
use crate::{Error, Result};
use serde_json::Value;
pub fn runner_kind(model: &Value) -> Result<&'static str> {
    match model["resolved"].get("adapter") {
        None => Ok("omp"),
        Some(Value::String(v)) if v == "omp" => Ok("omp"),
        Some(Value::String(v))
            if v == "codex"
                && model["resolved"]["api"] == "codex-app-server"
                && model["resolved"]["provider"] == "codex" =>
        {
            Ok("codex")
        }
        Some(Value::String(v))
            if v == "claude-code"
                && model["resolved"]["api"] == "claude-code-stream"
                && model["resolved"]["provider"] == "claude-code"
                && model["resolved"]["resolvedModel"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty()) =>
        {
            Ok("claude-code")
        }
        _ => Err(Error::Conflict(
            "Unknown or inconsistent frozen runner adapter.".into(),
        )),
    }
}

pub fn verify_runner_identity(model: &Value, identity: &Value) -> Result<()> {
    let kind = runner_kind(model)?;
    let supplied = identity["runnerKind"].as_str().unwrap_or("omp");
    if kind != supplied
        || (identity.get("runnerKind").is_some() && !identity["runnerKind"].is_string())
        || (kind != "omp" && identity.get("ompPid").is_some())
        || (kind == "omp"
            && identity.get("runnerPid").is_some()
            && identity["runnerPid"] != identity["ompPid"])
    {
        return Err(Error::Conflict(
            "Runner identity differs from the frozen assignment.".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn historical_omp_and_explicit_codex_identities_cannot_be_confused() {
        let legacy = json!({"resolved":{"provider":"openai","id":"model"}});
        let native =
            json!({"resolved":{"adapter":"codex","provider":"codex","api":"codex-app-server"}});
        assert_eq!(runner_kind(&legacy).unwrap(), "omp");
        verify_runner_identity(&legacy, &json!({"ompPid":42})).unwrap();
        verify_runner_identity(&native, &json!({"runnerKind":"codex","runnerPid":42})).unwrap();
        assert!(verify_runner_identity(&native, &json!({"ompPid":42})).is_err());
        assert!(
            verify_runner_identity(
                &native,
                &json!({"runnerKind":"codex","runnerPid":42,"ompPid":42})
            )
            .is_err()
        );
        assert!(
            verify_runner_identity(&legacy, &json!({"runnerKind":"codex","runnerPid":42})).is_err()
        );
        assert!(runner_kind(&json!({"resolved":{"adapter":"unknown"}})).is_err());
    }
}
