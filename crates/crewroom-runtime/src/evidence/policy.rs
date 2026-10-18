use super::*;
fn image(value: &Value, key: &str) -> Result<String> {
    let value = text(value, key)?;
    if !value.strip_prefix("sha256:").is_some_and(|v| {
        v.len() == 64
            && v.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }) {
        return Err(Error::Invalid("Use a pinned image SHA-256.".into()));
    }
    Ok(value.into())
}
pub fn definition(input: &Value) -> Result<Value> {
    request(
        input,
        &[
            "id",
            "image",
            "command",
            "args",
            "timeoutMs",
            "memoryMiB",
            "tmpExecutable",
            "required",
        ],
    )?;
    let id = limited(input, "id", 1, 64)?;
    if !id.as_bytes()[0].is_ascii_lowercase()
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(Error::Invalid("Invalid check ID.".into()));
    }
    let image = image(input, "image")?;
    let command = text(input, "command")?;
    if !Path::new(command).is_absolute() {
        return Err(Error::Invalid("Check command must be absolute.".into()));
    }
    let args = input["args"]
        .as_array()
        .filter(|a| {
            a.len() <= 100
                && a.iter()
                    .all(|v| v.as_str().is_some_and(|s| s.chars().count() <= 10000))
        })
        .ok_or_else(|| Error::Invalid("Invalid check arguments.".into()))?;
    let mut value = json!({"id":id,"image":image,"command":command,"args":args,"timeoutMs":number(input,"timeoutMs",100,3600000)?});
    if input.get("memoryMiB").is_some() {
        value["memoryMiB"] = json!(number(input, "memoryMiB", 128, 8192)?);
    }
    for key in ["tmpExecutable", "required"] {
        if key == "required" || input.get(key).is_some() {
            value[key] = json!(
                input[key]
                    .as_bool()
                    .ok_or_else(|| Error::Invalid(format!("Invalid {key}.")))?
            );
        }
    }
    Ok(value)
}
impl<S: StoreAccess> Evidence<S> {
    pub fn configure(&self, run_id: &str, input: &Value) -> Result<Value> {
        let checks = input
            .as_array()
            .filter(|a| a.len() <= 100)
            .ok_or_else(|| Error::Invalid("At most 100 checks may be configured.".into()))?
            .iter()
            .map(definition)
            .collect::<Result<Vec<_>>>()?;
        if checks
            .iter()
            .map(|c| &c["id"])
            .collect::<HashSet<_>>()
            .len()
            != checks.len()
        {
            return Err(conflict("Check IDs must be distinct."));
        }
        self.work.store(|s| s.update_coding_record(run_id, |previous| {
            if let Some(previous) = previous {
                if stringify(&previous["requiredChecks"]) != stringify(&json!(checks)) { return Err(conflict("Check policy was already accepted for this run.")); }
                return Ok(previous);
            }
            Ok(json!({"runId":run_id,"requiredChecks":checks,"candidates":[],"checks":[],"reviews":[]}))
        }))
    }
    pub fn correct_check_environment(&self, run_id: &str, input: &Value) -> Result<Value> {
        request(
            input,
            &[
                "requestId",
                "checkId",
                "expectedImage",
                "image",
                "expectedMemoryMiB",
                "memoryMiB",
                "reason",
            ],
        )?;
        let mut value = json!({"requestId":prose(input,"requestId",200)?,"checkId":limited(input,"checkId",1,200)?,"expectedImage":image(input,"expectedImage")?,"image":image(input,"image")?});
        if input.get("memoryMiB").is_some() != input.get("expectedMemoryMiB").is_some() {
            return Err(Error::Invalid(
                "A memory change names both the expected and the new limit.".into(),
            ));
        }
        for key in ["expectedMemoryMiB", "memoryMiB"] {
            if input.get(key).is_some() {
                value[key] = json!(number(input, key, 128, 8192)?);
            }
        }
        value["reason"] = json!(prose(input, "reason", 4000)?);
        let result = self.work.store(|s| s.update_coding_record(run_id, |previous| {
            let mut record = previous.ok_or_else(|| Error::NotFound("Coding evidence is not configured.".into()))?;
            if let Some(existing) = array(&record["checkPolicyChanges"]).iter().find(|c| c["requestId"] == value["requestId"]) {
                if value.as_object().unwrap().iter().any(|(key, v)| existing.get(key) != Some(v)) { return Err(conflict("Environment correction request has different content.")); }
                return Ok(record);
            }
            let run = s.get_run(run_id)?;
            if run["status"] != "paused" || run["desiredState"] != "paused" || s.repository_work(run_id)?.is_some_and(|w| w.get("writer").is_some())
                || array(&s.runtime(run_id)?["attempts"]).iter().any(|a| a["state"] != "admitted" && a["ownership"] != "stopped") {
                return Err(conflict("A confirmed paused run is required before correcting a check environment."));
            }
            if s.publication(run_id)?.is_some() { return Err(conflict("A publication already exists; environment correction requires the feedback-continuation workflow.")); }
            if array(&record["checkOperations"]).iter().any(|o| !["recorded","rejected"].contains(&o["state"].as_str().unwrap_or(""))) { return Err(conflict("Reconcile outstanding check operations before correcting the environment.")); }
            let checks = record["requiredChecks"].as_array_mut().ok_or_else(|| conflict("Check policy unavailable."))?;
            let check = checks.iter_mut().find(|c| c["id"] == value["checkId"]).ok_or_else(|| conflict("The selected check environment changed. Reload its current image and memory limit."))?;
            let memory = check.get("memoryMiB").cloned().unwrap_or(json!(1024));
            if check["image"] != value["expectedImage"] || value.get("expectedMemoryMiB").is_some_and(|v| *v != memory) { return Err(conflict("The selected check environment changed. Reload its current image and memory limit.")); }
            if check["image"] == value["image"] && value.get("memoryMiB").is_none_or(|v| *v == memory) { return Err(conflict("The correction leaves the check environment unchanged.")); }
            check["image"] = value["image"].clone();
            if let Some(memory) = value.get("memoryMiB") { check["memoryMiB"] = memory.clone(); }
            let mut changes = array(&record["checkPolicyChanges"]).to_vec(); let mut correction = value.clone(); correction["revision"] = json!(changes.len()+1); correction["createdAt"] = json!(time()); changes.push(correction); record["checkPolicyChanges"] = json!(changes);
            Ok(record)
        }))?;
        array(&result["checkPolicyChanges"])
            .iter()
            .find(|c| c["requestId"] == value["requestId"])
            .cloned()
            .ok_or_else(|| conflict("Correction receipt disappeared."))
    }
}
