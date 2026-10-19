//! One supervised native Claude session. SDK controls carry tools into the same
//! Crewroom host; they never replace workflow authorization or effect receipts.
use super::*;
use super::{
    auth, claude_history, claude_profile as policy,
    journal::SessionJournal,
    jsonl::{JsonlRpc, Protocol},
    profile,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::{
    sync::{Mutex as AsyncMutex, oneshot},
    time::{Duration, timeout},
};

struct Input {
    id: String,
    hash: String,
    ack: Option<oneshot::Sender<()>>,
}
struct Call {
    tool: String,
    args: Value,
    turn: String,
    allowed: bool,
}
#[derive(Default)]
struct State {
    active: Option<String>,
    consumed: BTreeSet<String>,
    inputs: BTreeMap<String, Input>,
    calls: BTreeMap<String, Call>,
    requests: BTreeSet<String>,
    tools: BTreeMap<String, (Value, String)>,
    cancelled: BTreeSet<String>,
    terminal: BTreeMap<String, Value>,
    messages: BTreeMap<String, String>,
    initialized: bool,
    compacting: bool,
}
pub struct Claude {
    rpc: OnceLock<JsonlRpc>,
    event: Event,
    state: Mutex<State>,
    journal: Mutex<SessionJournal>,
    session: String,
    model: Value,
    catalog: BTreeMap<String, Value>,
    home: PathBuf,
    cwd: PathBuf,
    account: String,
    /// The admitted (verified or probed) `--version` this executable
    /// reported before launch.
    version: String,
    submit_lock: AsyncMutex<()>,
    delivery_lock: AsyncMutex<()>,
    failed: AtomicBool,
    _lease: File,
}

pub(super) async fn open(
    helper: &Path,
    root: &Path,
    config: &Value,
    tools: &[Value],
    event: Event,
    stopping: &tokio_util::sync::CancellationToken,
) -> Result<(Runner, Value)> {
    let cwd = Path::new(text(config, "runnerCwd")?);
    private_dir(cwd)?;
    let saved = cwd.join("claude-session");
    private_dir(&saved)?;
    let lease = auth::lock(&saved.join("owner.lock")).await?;
    let binary = Path::new(text(&config["claudeCode"], "binary")?);
    let home = Path::new(text(&config["claudeCode"], "home")?);
    let (fingerprint, sessions, version) =
        policy::prepare(binary, home, cwd, &config["model"]).await?;
    let account = policy::account(home)?;
    let model = &config["model"]["resolved"];
    for key in ["id", "resolvedModel", "effort"] {
        text(model, key)?;
    }
    let binding = json!({"assignmentId":config["assignmentId"],"model":model,"profile":fingerprint,"account":account,"tools":profile::digest(stringify(&json!(tools)).as_bytes())});
    let mut journal = SessionJournal::open(&saved, &binding)?;
    let resume = journal.has_inputs;
    let session = journal
        .thread
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    uuid::Uuid::parse_str(&session)
        .map_err(|_| conflict("Claude retained session ID is invalid."))?;
    journal.bind_thread(&session)?;
    let history = if resume {
        let history = claude_history::read(
            &sessions.join(format!("{session}.jsonl")),
            &session,
            text(model, "resolvedModel")?,
        )?;
        journal.reconcile_receipts(&history.receipts)?;
        journal.reconcile_inputs(&history.inputs)?;
        Some(history)
    } else {
        None
    };
    let catalog: BTreeMap<_, _> = tools
        .iter()
        .map(|t| Ok((text(t, "name")?.to_owned(), t.clone())))
        .collect::<Result<_>>()?;
    if catalog.len() != tools.len() || catalog.len() > 128 {
        return Err(conflict("Claude tool catalog is ambiguous or too large."));
    }
    let adapter = Arc::new(Claude {
        rpc: OnceLock::new(),
        event,
        state: Mutex::new(State::default()),
        journal: Mutex::new(journal),
        session: session.clone(),
        model: model.clone(),
        catalog,
        home: home.into(),
        cwd: cwd.into(),
        account,
        version,
        submit_lock: AsyncMutex::new(()),
        delivery_lock: AsyncMutex::new(()),
        failed: AtomicBool::new(false),
        _lease: lease,
    });
    let spec = CommandSpec {
        command: binary.to_string_lossy().into(),
        args: policy::arguments(model, text(config, "prompt")?, &session, resume),
        cwd: cwd.into(),
        env: policy::environment(home),
    };
    let (send, mut frames) = tokio::sync::mpsc::channel(32);
    let weak = Arc::downgrade(&adapter);
    let rpc = JsonlRpc::start_protocol(
        helper,
        &spec,
        &root.join("claude-process"),
        Protocol::Claude,
        move |v| {
            if send.try_send(v).is_err()
                && let Some(a) = weak.upgrade()
            {
                a.fail("Claude event buffer exceeded its bound.");
            }
        },
    )
    .await?;
    adapter.rpc.set(rpc.clone()).ok().unwrap();
    let weak = Arc::downgrade(&adapter);
    tokio::spawn(async move {
        while let Some(frame) = frames.recv().await {
            let Some(a) = weak.upgrade() else { break };
            if let Err(e) = a.handle(frame).await {
                a.fail(&e.to_string());
            }
        }
    });
    let result:Result<Value>=async {
        let init=rpc.command("initialize",json!({"sdkMcpServers":["crewroom"],"hooks":{"PreToolUse":[{"hookCallbackIds":["crewroom-gate"]}]}})).await?;
        let offered=init["models"].as_array().and_then(|a|a.iter().find(|v|v["value"]==model["id"] || v["resolvedModel"]==model["id"]))
            .ok_or_else(||conflict("Claude model is absent from its current catalog."))?;
        if offered["resolvedModel"]!=model["resolvedModel"] || !offered["supportedEffortLevels"].as_array().is_some_and(|a|a.contains(&model["effort"])) {
            return Err(conflict("Claude alias or effort differs from the frozen assignment; explicit re-admission is required."));
        }
        if let Some(h)=history && let Some(usage)=h.usage {adapter.usage(&usage)?;}
        let retained=adapter.journal.lock().unwrap().usage.clone();
        if retained["totalTokens"].is_u64(){adapter.publish_usage(&retained)?;}
        if stopping.is_cancelled() || adapter.failed.load(Ordering::SeqCst){return Err(conflict("Claude stopped during initialization."));}
        let mut identity=json!({"hostPid":std::process::id(),"runnerKind":"claude-code","runnerPid":rpc.pid,"runnerVersion":adapter.version,
            "runnerProfile":fingerprint,"model":model,"observedModel":model["resolvedModel"],"thinkingLevel":model["effort"],
            "sessionId":session,"sessionDirectory":sessions,"capabilities":{"authentication":"native-claude","usage":"cumulative-tokens","cost":"list-price-estimate",
                "preRequestBudget":false,"hardOutputLimit":false,"compaction":"harness-managed","toolAuthority":"crewroom"}});
        if resume{identity["recoveryMode"]=json!("session");}
        Ok(identity)
    }.await;
    match result {
        Ok(identity) => Ok((Runner::Claude(adapter), identity)),
        Err(e) => {
            rpc.close().await?;
            Err(e)
        }
    }
}

impl Claude {
    fn rpc(&self) -> &JsonlRpc {
        self.rpc
            .get()
            .expect("Claude transport installed before controls")
    }
    fn emit(&self, v: Value) {
        (self.event)(v)
    }
    fn fail(&self, reason: &str) {
        if !self.failed.swap(true, Ordering::SeqCst) {
            self.emit(json!({"type":"model_policy_rejected","code":"claude_contract","recovery":"configuration","reason":reason}));
            self.emit(json!({"type":"rpc_stream_closed","reason":reason}));
        }
    }
    fn authorized(&self) -> Result<()> {
        if self.failed.load(Ordering::SeqCst) || policy::account(&self.home)? != self.account {
            return Err(conflict(
                "Claude authority or native account changed; reconcile before resuming.",
            ));
        }
        Ok(())
    }
    async fn respond(&self, id: &str, value: Value) -> Result<()> {
        if self.state.lock().unwrap().cancelled.contains(id) {
            return Ok(());
        }
        self.rpc().send(&json!({"type":"control_response","response":{"subtype":"success","request_id":id,"response":value}})).await
    }
    async fn handle(&self, frame: Value) -> Result<()> {
        let kind = frame["type"].as_str().unwrap_or("");
        if kind == "rpc_stream_closed" || kind == "process_exit" {
            self.failed.store(true, Ordering::SeqCst);
            self.emit(frame);
            return Ok(());
        }
        // Buffered frames can outlive a rejected contract or a fatal host effect.
        // None may turn that failure into output, accounting or completion.
        if self.failed.load(Ordering::SeqCst) {
            return Ok(());
        }
        if kind == "control_response" {
            return Ok(());
        } // CLI acknowledgment of our server response.
        if kind == "control_cancel_request" {
            let id = text(&frame, "request_id")?;
            let mut state = self.state.lock().unwrap();
            if !state.requests.contains(id) {
                return Err(conflict("Claude cancelled an unknown control."));
            }
            state.cancelled.insert(id.into());
            let tool = state.tools.contains_key(id);
            drop(state);
            if tool {
                self.emit(json!({"type":"host_tool_cancel","targetId":id}));
            }
            return Ok(());
        }
        if kind == "control_request" {
            return self.control(frame).await;
        }
        if frame["session_id"] != self.session {
            return Err(conflict("Claude frame belongs to another session."));
        }
        if frame
            .get("parent_tool_use_id")
            .is_some_and(|v| !v.is_null())
        {
            return Err(conflict("Claude emitted an unadmitted subagent frame."));
        }
        match kind {
            "system" if frame["subtype"] == "init" => {
                let expected: BTreeSet<_> = self
                    .catalog
                    .keys()
                    .map(|n| format!("mcp__crewroom__{n}"))
                    .collect();
                let actual: BTreeSet<_> = frame["tools"]
                    .as_array()
                    .ok_or_else(|| conflict("Claude omitted its tool inventory."))?
                    .iter()
                    .map(|n| n.as_str().unwrap_or("").to_owned())
                    .collect();
                if frame["model"] != self.model["resolvedModel"]
                    || frame["permissionMode"] != "dontAsk"
                    || frame["claude_code_version"] != policy::startup_version(&self.version)
                    || expected != actual
                {
                    return Err(conflict(
                        "Claude startup model, tool inventory or permission policy changed.",
                    ));
                }
                self.state.lock().unwrap().initialized = true;
            }
            "command_lifecycle" => {
                let wire = text(&frame, "command_uuid")?;
                let mut state = self.state.lock().unwrap();
                match frame["state"].as_str() {
                    Some("queued") => {
                        // A terminal receipt is also proof of acceptance; its
                        // queued notification may arrive after submit settled.
                        if state.terminal.contains_key(wire) {
                            return Ok(());
                        }
                        let input = state
                            .inputs
                            .get_mut(wire)
                            .ok_or_else(|| conflict("Claude queued an unrequested input."))?;
                        self.journal
                            .lock()
                            .unwrap()
                            .record(json!({"kind":"turn","inputId":input.id,"turnId":wire}))?;
                        if let Some(ack) = input.ack.take() {
                            let _ = ack.send(());
                        }
                    }
                    Some("started") => {
                        if state.active.is_none() && !state.tools.is_empty() {
                            return Err(conflict(
                                "Claude started another turn with an unresolved tool receipt.",
                            ));
                        }
                        if !state.inputs.contains_key(wire) || !state.consumed.insert(wire.into()) {
                            return Err(conflict(
                                "Claude started an unrequested or duplicate input.",
                            ));
                        }
                        // Claude can coalesce a batch or absorb steering into an
                        // active turn. Keep one stable local anchor for its calls.
                        if state.active.is_none() {
                            state.active = Some(wire.into());
                            drop(state);
                            self.emit(json!({"type":"agent_start","turnId":wire}));
                        }
                    }
                    Some("completed" | "cancelled") => (),
                    _ => return Err(conflict("Claude command lifecycle is invalid.")),
                }
            }
            "user" if frame["isReplay"] == true => {
                let state = self.state.lock().unwrap();
                let input = state
                    .inputs
                    .get(text(&frame, "uuid")?)
                    .ok_or_else(|| conflict("Claude acknowledged unknown input."))?;
                if frame["message"]["content"]
                    .as_str()
                    .map(|s| profile::digest(s.as_bytes()))
                    != Some(input.hash.clone())
                {
                    return Err(conflict("Claude acknowledged changed input."));
                }
            }
            "stream_event" => {
                let event = &frame["event"];
                let mut state = self.state.lock().unwrap();
                if state.active.is_none() {
                    return Err(conflict("Claude emitted output outside an admitted turn."));
                }
                if let Some(id) = frame["user_message_uuid"].as_str()
                    && !state.consumed.contains(id)
                {
                    return Err(conflict("Claude output belongs to another turn."));
                }
                if event["type"] == "message_start"
                    && event["message"]["model"] != self.model["resolvedModel"]
                {
                    return Err(conflict("Claude inference model changed."));
                }
                let message = text(&frame, "api_message_id")?;
                if event["type"] == "message_start" {
                    if state.messages.len() >= 4096
                        || state.messages.contains_key(message)
                        || event["message"]["id"] != message
                    {
                        return Err(conflict(
                            "Claude stream message identity was reused or changed.",
                        ));
                    }
                    let turn = state.active.clone().unwrap();
                    state.messages.insert(message.into(), turn);
                }
                if state.messages.get(message) != state.active.as_ref() {
                    return Err(conflict("Claude stream belongs to another turn."));
                }
                drop(state);
                if event["type"] == "content_block_delta"
                    && ["text_delta", "thinking_delta"]
                        .iter()
                        .any(|k| event["delta"]["type"] == *k)
                {
                    self.emit(json!({"type":"message_update","messageId":frame["api_message_id"],"assistantMessageEvent":{"type":event["delta"]["type"],"delta":if event["delta"]["type"]=="text_delta"{&event["delta"]["text"]}else{&event["delta"]["thinking"]}}}));
                }
            }
            "assistant" => {
                let message = &frame["message"];
                let mut state = self.state.lock().unwrap();
                let active = state
                    .active
                    .clone()
                    .ok_or_else(|| conflict("Claude assistant has no admitted turn."))?;
                if frame["user_message_uuid"]
                    .as_str()
                    .is_some_and(|id| !state.consumed.contains(id))
                    || message["model"] != self.model["resolvedModel"]
                {
                    return Err(conflict("Claude assistant model or turn changed."));
                }
                if state.messages.get(text(message, "id")?) != Some(&active) {
                    return Err(conflict("Claude assistant has no matching active stream."));
                }
                for item in message["content"]
                    .as_array()
                    .ok_or_else(|| conflict("Claude assistant content is invalid."))?
                {
                    if item["type"] == "tool_use" {
                        let call = text(item, "id")?;
                        let name = text(item, "name")?
                            .strip_prefix("mcp__crewroom__")
                            .ok_or_else(|| {
                                conflict("Claude attempted an unadmitted native tool.")
                            })?;
                        let tool = self.catalog.get(name).ok_or_else(|| {
                            conflict("Claude requested an unknown Crewroom tool.")
                        })?;
                        policy::schema(&tool["parameters"], &item["input"])?;
                        if state.calls.len() >= 4096 || state.calls.contains_key(call) {
                            return Err(conflict(
                                "Claude reused a tool call identity or exceeded its bound.",
                            ));
                        }
                        state.calls.insert(
                            call.into(),
                            Call {
                                tool: name.into(),
                                args: item["input"].clone(),
                                turn: active.clone(),
                                allowed: false,
                            },
                        );
                    }
                }
                drop(state);
                self.emit(json!({"type":"message_end","message":{"role":"assistant","content":message["content"],"model":message["model"]}}));
            }
            "system" if frame["subtype"] == "status" => {
                let compacting = frame["status"] == "compacting";
                let mut state = self.state.lock().unwrap();
                if state.compacting != compacting {
                    state.compacting = compacting;
                    drop(state);
                    self.emit(json!({"type":"harness_compaction","state":if compacting{"started"}else{"completed"}}));
                }
            }
            "result" => {
                // A fast runner can report completion before the writer task
                // resumes after flush. Settle only after delivery bookkeeping.
                let _delivery = self.delivery_lock.lock().await;
                if !self.failed.load(Ordering::SeqCst) {
                    self.terminal(&frame)?;
                }
            }
            // Tool-result and synthetic user messages are observations, not new inputs.
            "user" | "system" | "rate_limit_event" | "tool_progress" | "tool_use_summary" => (),
            _ => return Err(conflict("Claude emitted an unsupported protocol frame.")),
        }
        Ok(())
    }
    async fn control(&self, frame: Value) -> Result<()> {
        let id = text(&frame, "request_id")?;
        let request = &frame["request"];
        {
            let mut state = self.state.lock().unwrap();
            if state.requests.len() >= 4096 || !state.requests.insert(id.into()) {
                return Err(conflict(
                    "Claude control identity was reused or exceeded its bound.",
                ));
            }
        }
        match request["subtype"].as_str() {
            Some("hook_callback") => {
                self.pre_tool(request)?;
                self.respond(
                    id,
                    json!({"hookSpecificOutput": {
                        "hookEventName": "PreToolUse", "permissionDecision": "allow"
                    }}),
                )
                .await
            }
            Some("can_use_tool") => {
                self.respond(
                    id,
                    json!({
                        "behavior": "deny", "message": "Crewroom requires its per-call tool gate."
                    }),
                )
                .await
            }
            Some("mcp_message") => self.mcp(id, request).await,
            _ => Err(conflict(
                "Claude requested an unsupported native operation.",
            )),
        }
    }
    fn pre_tool(&self, request: &Value) -> Result<()> {
        self.authorized()?;
        let input = &request["input"];
        let call = text(request, "tool_use_id")?;
        let mut state = self.state.lock().unwrap();
        let active = state.active.clone();
        let initialized = state.initialized;
        let intent = state
            .calls
            .get_mut(call)
            .ok_or_else(|| conflict("Claude tool gate has no matching assistant intent."))?;
        if !initialized
            || active.as_deref() != Some(&intent.turn)
            || request["callback_id"] != "crewroom-gate"
            || input["hook_event_name"] != "PreToolUse"
            || input["session_id"] != self.session
            || input["tool_use_id"] != call
            || input["cwd"] != json!(self.cwd)
            || input["effort"]["level"] != self.model["effort"]
            || input["tool_name"] != format!("mcp__crewroom__{}", intent.tool)
            || input["tool_input"] != intent.args
            || intent.allowed
        {
            return Err(conflict(
                "Claude pre-tool address, model effort or arguments changed.",
            ));
        }
        intent.allowed = true;
        Ok(())
    }
    async fn mcp(&self, id: &str, request: &Value) -> Result<()> {
        if request["server_name"] != "crewroom" {
            return Err(conflict("Claude requested an unadmitted MCP server."));
        }
        let message = &request["message"];
        if message["jsonrpc"] != "2.0" {
            return Err(conflict("Claude MCP envelope is invalid."));
        }
        let result = match message["method"].as_str() {
            Some("initialize") => json!({
                "protocolVersion": "2025-11-25", "capabilities": {"tools": {}},
                "serverInfo": {"name": "crewroom", "version": "0.1.0"}
            }),
            Some("notifications/initialized") => json!({}),
            Some("notifications/cancelled") => {
                let targets: Vec<String> = {
                    let mut state = self.state.lock().unwrap();
                    let targets = state
                        .tools
                        .iter()
                        .filter(|(_, (request, _))| request == &message["params"]["requestId"])
                        .map(|(id, _)| id.clone())
                        .collect::<Vec<_>>();
                    if targets.len() != 1 {
                        return Err(conflict("Claude cancelled an unknown MCP request."));
                    }
                    state.cancelled.extend(targets.clone());
                    targets
                };
                for target in targets {
                    self.emit(json!({"type": "host_tool_cancel", "targetId": target}));
                }
                json!({})
            }
            Some("tools/list") => json!({"tools": self.catalog.values().map(|tool| json!({
                "name": tool["name"], "description": tool["description"], "inputSchema": tool["parameters"]
            })).collect::<Vec<_>>()}),
            Some("tools/call") => return self.tool_call(id, message).await,
            _ => return Err(conflict("Claude requested an unadmitted MCP operation.")),
        };
        let mut response = json!({"jsonrpc": "2.0", "result": result});
        if let Some(id) = message.get("id") {
            response["id"] = id.clone();
        }
        self.respond(id, json!({"mcp_response": response})).await
    }
    async fn tool_call(&self, id: &str, message: &Value) -> Result<()> {
        self.authorized()?;
        let params = &message["params"];
        let call = text(&params["_meta"], "claudecode/toolUseId")?;
        let (tool, retained) = {
            let mut state = self.state.lock().unwrap();
            let intent = state
                .calls
                .get(call)
                .ok_or_else(|| conflict("Claude MCP call has no approved original intent."))?;
            if !intent.allowed
                || state.active.as_deref() != Some(&intent.turn)
                || params["name"] != intent.tool
                || params["arguments"] != intent.args
                || !(message["id"].is_number() || message["id"].is_string())
                || state
                    .tools
                    .values()
                    .any(|(request, old)| request == &message["id"] || old == call)
            {
                return Err(conflict(
                    "Claude MCP address, input or permission does not match the original call.",
                ));
            }
            let tool = intent.tool.clone();
            let retained =
                self.journal
                    .lock()
                    .unwrap()
                    .admit(call, &tool, &intent.args, &intent.turn)?;
            state
                .tools
                .insert(id.into(), (message["id"].clone(), call.into()));
            (tool, retained)
        };
        if let Some(result) = retained {
            return self.reply(id, result).await;
        }
        self.emit(json!({
            "type": "host_tool_call", "id": id,
            "toolCallId": profile::digest(format!("{}:{call}", self.session).as_bytes()),
            "toolName": tool, "arguments": params["arguments"]
        }));
        Ok(())
    }
    fn usage(&self, models: &Value) -> Result<()> {
        let Some(rows) = models.as_object().filter(|v| !v.is_empty()) else {
            return Ok(());
        };
        if rows
            .keys()
            .any(|id| id != self.model["resolvedModel"].as_str().unwrap())
        {
            return Err(conflict("Claude accounting reports a different model."));
        }
        let row = &models[text(&self.model, "resolvedModel")?];
        let mut raw = json!({});
        for (from, to) in [
            ("inputTokens", "inputTokens"),
            ("outputTokens", "outputTokens"),
            ("cacheReadInputTokens", "cachedInputTokens"),
            ("cacheCreationInputTokens", "cacheWriteInputTokens"),
        ] {
            raw[to] = json!(
                row[from]
                    .as_u64()
                    .ok_or_else(|| conflict("Claude usage is incomplete."))?
            );
        }
        if let Some(value) = row.get("thinkingTokens") {
            raw["reasoningOutputTokens"] = value.clone();
        }
        raw["totalTokens"] = json!(
            raw["inputTokens"]
                .as_u64()
                .unwrap()
                .checked_add(raw["outputTokens"].as_u64().unwrap())
                .and_then(|n| n.checked_add(raw["cachedInputTokens"].as_u64().unwrap()))
                .and_then(|n| n.checked_add(raw["cacheWriteInputTokens"].as_u64().unwrap()))
                .ok_or_else(|| conflict("Claude token totals overflowed."))?
        );
        let cost = row["costUSD"]
            .as_f64()
            .filter(|n| n.is_finite() && *n >= 0.);
        if let Some(cost) = cost {
            raw["estimatedCostUsd"] = json!(cost);
        }
        self.publish_usage(&raw)
    }
    fn publish_usage(&self, raw: &Value) -> Result<()> {
        let mut journal = self.journal.lock().unwrap();
        let usage = journal.usage(raw)?;
        let cost = journal.usage["estimatedCostUsd"].clone();
        drop(journal);
        self.emit(json!({"type":"provider_usage","state":"snapshot","usage":usage,"sessionId":self.session,"source":"claude-native","accounting":"cumulative","cost":cost,"costBasis":"list-price-estimate"}));
        Ok(())
    }
    fn terminal(&self, frame: &Value) -> Result<()> {
        let id = text(frame, "user_message_uuid")?;
        let mut state = self.state.lock().unwrap();
        if let Some(old) = state.terminal.get(id) {
            return if old == frame {
                Ok(())
            } else {
                Err(conflict("Claude terminal receipt changed."))
            };
        }
        let consumed: BTreeSet<String> = match frame.get("user_message_uuids") {
            Some(Value::Array(ids)) if !ids.is_empty() && ids.len() <= 64 => {
                let unique = ids
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(String::from)
                            .ok_or_else(|| conflict("Claude terminal input identity is invalid."))
                    })
                    .collect::<Result<BTreeSet<_>>>()?;
                if unique.len() != ids.len() {
                    return Err(conflict("Claude terminal input identities repeat."));
                }
                unique
            }
            None => BTreeSet::from([id.into()]),
            _ => return Err(conflict("Claude terminal input identities are invalid.")),
        };
        if !consumed.contains(id) || consumed != state.consumed {
            return Err(conflict("Claude terminal result belongs to another turn."));
        }
        let status = match frame["terminal_reason"].as_str() {
            Some("aborted_streaming" | "aborted_tools") => "aborted",
            Some("completed") if frame["is_error"] == false && frame["subtype"] == "success" => {
                "completed"
            }
            _ => "error",
        };
        if status == "completed" && !state.tools.is_empty() {
            return Err(conflict(
                "Claude claimed completion with an unresolved tool receipt.",
            ));
        }
        self.usage(&frame["modelUsage"])?;
        let mut inputs = Vec::new();
        for wire in &consumed {
            let mut input = state
                .inputs
                .remove(wire)
                .ok_or_else(|| conflict("Claude terminal input is unknown."))?;
            let mut journal = self.journal.lock().unwrap();
            journal.record(json!({"kind":"turn","inputId":input.id,"turnId":wire}))?;
            journal.record(json!({"kind":"terminal","turnId":wire,"status":status}))?;
            if let Some(ack) = input.ack.take() {
                let _ = ack.send(());
            }
            inputs.push(input);
        }
        let pending: Vec<_> = state.tools.keys().cloned().collect();
        state.cancelled.extend(pending.clone());
        state.active = None;
        state.consumed.clear();
        state.compacting = false;
        if state.terminal.len() + consumed.len() > 4096 {
            return Err(conflict("Claude terminal history exceeded its bound."));
        }
        for wire in consumed {
            state.terminal.insert(wire, frame.clone());
        }
        let settled = state.inputs.is_empty();
        drop(state);
        for key in pending {
            self.emit(json!({"type":"host_tool_cancel","targetId":key}));
        }
        for input in inputs {
            self.emit(json!({"type":"prompt_result","id":input.id,"status":status,"error":if status=="error"{json!({"message":"Claude turn failed; inspect native access and retained operation receipts.","retryable":false})}else{Value::Null}}));
        }
        if settled {
            self.emit(json!({"type":"session_settled"}));
        }
        Ok(())
    }
    pub async fn state(&self) -> Result<Value> {
        self.authorized()?;
        let s = self.state.lock().unwrap();
        Ok(
            json!({"isStreaming":!s.inputs.is_empty(),"isCompacting":s.compacting,"sessionId":self.session}),
        )
    }
    pub async fn submit(&self, id: &str, message: &str, steer: bool) -> Result<()> {
        let _guard = self.submit_lock.lock().await;
        self.authorized()?;
        let wire = uuid::Uuid::new_v4().to_string();
        let (send, ack) = oneshot::channel();
        let hash = profile::digest(message.as_bytes());
        {
            let mut s = self.state.lock().unwrap();
            if s.active.is_none() && !s.tools.is_empty() {
                return Err(conflict(
                    "Claude has an unresolved tool receipt; reconcile before sending another input.",
                ));
            }
            if s.compacting || !steer && !s.inputs.is_empty() || s.inputs.len() >= 16 {
                return Err(conflict("Claude is busy."));
            }
            self.journal
                .lock()
                .unwrap()
                .record(json!({"kind":"input","id":id,"wireId":wire,"hash":hash}))?;
            s.inputs.insert(
                wire.clone(),
                Input {
                    id: id.into(),
                    hash,
                    ack: Some(send),
                },
            );
        }
        let result:Result<()>=async{self.rpc().send(&json!({"type":"user","uuid":wire,"session_id":self.session,"parent_tool_use_id":null,"client_composed":true,"message":{"role":"user","content":message}})).await?;
            timeout(Duration::from_secs(20),ack).await.map_err(|_|conflict("Claude input acknowledgment timed out."))?.map_err(|_|conflict("Claude input acknowledgment closed."))?;Ok(())}.await;
        if result.is_err() {
            self.fail(
                "Claude input dispatch is unconfirmed; retained intent requires reconciliation.",
            );
        }
        result
    }
    async fn reply(&self, key: &str, result: Value) -> Result<()> {
        let _delivery = self.delivery_lock.lock().await;
        let (mcp_id, call) = self
            .state
            .lock()
            .unwrap()
            .tools
            .get(key)
            .cloned()
            .ok_or_else(|| conflict("Unknown Claude tool response."))?;
        self.journal.lock().unwrap().result(&call, &result)?;
        if let Err(error) = self.respond(key,json!({"mcp_response":{"jsonrpc":"2.0","id":mcp_id,"result":{"content":result["contentItems"],"isError":result["success"]!=true}}})).await {
            self.fail("Claude tool result delivery is unconfirmed; reconcile the retained receipt.");
            return Err(error);
        }
        let cancelled = {
            let mut state = self.state.lock().unwrap();
            state.tools.remove(key);
            state.cancelled.contains(key)
        };
        if cancelled {
            self.fail("Claude cancelled tool delivery; reconcile the retained original result before further input.");
        }
        Ok(())
    }
    pub async fn resolve_tool(&self, frame: &Value) -> Result<()> {
        if frame["outcome"] == "unresolved" {
            self.fail("Claude host effect is unresolved; the original intent is retained and no result was asserted.");
            return Err(conflict("Claude host effect requires reconciliation."));
        }
        if !frame["result"]["content"].is_array() {
            return Err(conflict("Claude host result is invalid."));
        }
        self.reply(
            text(frame, "id")?,
            json!({"success":frame["isError"]!=true,"contentItems":frame["result"]["content"]}),
        )
        .await
    }
    pub async fn interrupt(&self) -> Result<()> {
        self.rpc().command("interrupt", json!({})).await?;
        Ok(())
    }
    pub async fn close(&self) -> Result<()> {
        self.rpc().close().await
    }
}
