//! A single Codex thread in a separately owned app-server process. All effects
//! cross the addressed dynamic-tool boundary into Crewroom's existing host.
use super::*;
use super::{auth, journal::SessionJournal, jsonl::JsonlRpc, profile};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::Mutex as AsyncMutex;

#[derive(Default)]
struct State {
    thread: String,
    turn: Option<String>,
    inputs: Vec<String>,
    awaiting: Option<String>,
    pending_steer: Option<(String, String)>,
    steer_terminal: Option<String>,
    tools: BTreeMap<String, (Value, String)>,
    compacting: bool,
}
pub struct Codex {
    rpc: OnceLock<JsonlRpc>,
    event: Event,
    config: Value,
    state: Mutex<State>,
    journal: Mutex<SessionJournal>,
    names: BTreeSet<String>,
    credential: Mutex<auth::Credentials>,
    submit_lock: AsyncMutex<()>,
    refresh_lock: AsyncMutex<()>,
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
    let home = root.join("codex-home");
    private_dir(&home)?;
    let cwd = Path::new(text(config, "runnerCwd")?);
    private_dir(cwd)?;
    let sessions = cwd.join("codex-session");
    private_dir(&sessions)?;
    let lease = auth::lock(&sessions.join("owner.lock")).await?;
    let binary = Path::new(text(&config["codex"], "binary")?);
    let (fingerprint, version) =
        profile::prepare(binary, &home, &sessions, &config["model"]).await?;
    let credential = auth::credentials(config, None).await?;
    let binding = json!({"assignmentId":config["assignmentId"],"model":config["model"]["resolved"],
        "profile":fingerprint,"account":profile::digest(credential.account.as_bytes()),
        "tools":profile::digest(stringify(&json!(tools)).as_bytes())});
    let journal = SessionJournal::open(&sessions, &binding)?;
    let old_thread = journal.thread.clone().filter(|_| journal.has_inputs);
    let adapter = Arc::new(Codex {
        rpc: OnceLock::new(),
        event,
        config: config.clone(),
        state: Mutex::new(State {
            thread: old_thread.clone().unwrap_or_default(),
            ..State::default()
        }),
        journal: Mutex::new(journal),
        names: tools
            .iter()
            .map(|v| text(v, "name").map(String::from))
            .collect::<Result<_>>()?,
        credential: Mutex::new(credential),
        submit_lock: AsyncMutex::new(()),
        refresh_lock: AsyncMutex::new(()),
        failed: AtomicBool::new(false),
        _lease: lease,
    });
    let observer = Arc::downgrade(&adapter);
    let spec = CommandSpec {
        command: binary.to_string_lossy().into(),
        args: vec![
            "app-server".into(),
            "--listen".into(),
            "stdio://".into(),
            "--strict-config".into(),
        ],
        cwd: root.into(),
        env: profile::environment(&home, &sessions),
    };
    let rpc = JsonlRpc::start(helper, &spec, move |v| {
        if let Some(a) = observer.upgrade() {
            a.frame(v)
        }
    })
    .await?;
    adapter.rpc.set(rpc.clone()).ok().unwrap();
    let result:Result<Value>=async {
        rpc.command("initialize",json!({"clientInfo":{"name":"crewroom","version":"0.1.0"},"capabilities":{"experimentalApi":true}})).await?;
        rpc.send(&json!({"method":"initialized"})).await?;
        let mut login=adapter.credential.lock().unwrap().reply();login["type"]=json!("chatgptAuthTokens");
        rpc.command("account/login/start",login).await?;
        let expected=&config["model"]["resolved"];
        let dynamic:Vec<_>=tools.iter().map(|t|json!({"type":"function","name":t["name"],"description":t["description"],"inputSchema":t["parameters"]})).collect();
        let mut params=json!({"model":expected["id"],"modelProvider":"openai","cwd":cwd,
            "sandbox":"read-only","approvalPolicy":"never","developerInstructions":config["prompt"],
            "allowProviderModelFallback":false});
        let method=if let Some(id)=&old_thread {params["threadId"]=json!(id);"thread/resume"}
            else {params["dynamicTools"]=json!(dynamic);"thread/start"};
        let actual=rpc.command(method,params).await?;
        if actual["model"]!=expected["id"] || actual["modelProvider"]!="openai" || actual["reasoningEffort"]!=expected["effort"]
            || actual["approvalPolicy"]!="never" || actual["sandbox"]["type"]!="readOnly" {
            return Err(conflict("Codex model, effort or execution policy differs from the frozen assignment."));
        }
        let id=text(&actual["thread"],"id")?;
        if let Some(old)=&old_thread {
            if old!=id {return Err(conflict("Codex resumed a different thread."));}
            adapter.journal.lock().unwrap().reconcile(&actual["thread"])?;
        }
        adapter.journal.lock().unwrap().bind_thread(id)?;
        adapter.state.lock().unwrap().thread=id.into();
        let retained_usage=adapter.journal.lock().unwrap().usage.clone();
        if retained_usage.get("totalTokens").is_some(){adapter.usage(&retained_usage)?;}
        if stopping.is_cancelled() || adapter.failed.load(Ordering::SeqCst) {return Err(conflict("Codex stopped during initialization."));}
        let mut identity=json!({"hostPid":std::process::id(),"runnerKind":"codex","runnerPid":rpc.pid,
            "runnerVersion":version,"runnerProfile":fingerprint,"model":expected,
            "thinkingLevel":expected["effort"],"sessionId":id,"sessionDirectory":sessions,
            "capabilities":{"authentication":"native-chatgpt","usage":"cumulative-tokens","cost":"unavailable",
                "preRequestBudget":false,"hardOutputLimit":false,"compaction":"harness-managed","toolAuthority":"crewroom"}});
        if old_thread.is_some(){identity["recoveryMode"]=json!("session");}
        Ok(identity)
    }.await;
    match result {
        Ok(identity) => Ok((Runner::Codex(adapter), identity)),
        Err(error) => {
            rpc.close().await?;
            Err(error)
        }
    }
}
impl Codex {
    fn rpc(&self) -> Result<&JsonlRpc> {
        self.rpc
            .get()
            .ok_or_else(|| conflict("Codex is not initialized."))
    }
    fn emit(&self, v: Value) {
        (self.event)(v)
    }
    fn fail(&self, reason: &str) {
        if !self.failed.swap(true, Ordering::SeqCst) {
            self.emit(json!({"type":"model_policy_rejected","code":"codex_contract","recovery":"configuration","reason":reason}));
            self.emit(json!({"type":"rpc_stream_closed","reason":reason}));
        }
    }
    fn frame(self: &Arc<Self>, frame: Value) {
        if let Err(error) = self.handle(frame) {
            self.fail(&error.to_string());
        }
    }
    fn handle(self: &Arc<Self>, frame: Value) -> Result<()> {
        if frame["type"] == "rpc_stream_closed" || frame["type"] == "process_exit" {
            self.emit(frame);
            return Ok(());
        }
        let method = frame["method"].as_str().unwrap_or("");
        let p = &frame["params"];
        if frame.get("id").is_some() {
            if method == "account/chatgptAuthTokens/refresh" {
                let this = self.clone();
                let id = frame["id"].clone();
                let previous = p["previousAccountId"].clone();
                tokio::spawn(async move {
                    let _guard = this.refresh_lock.lock().await;
                    let (account, fingerprint) = {
                        let c = this.credential.lock().unwrap();
                        (c.account.clone(), c.fingerprint())
                    };
                    if previous.as_str().is_some_and(|v| v != account) {
                        this.fail("Codex requested credentials for a different account.");
                        return;
                    }
                    match auth::credentials(&this.config, Some((&account, &fingerprint))).await {
                        Ok(c) => {
                            let reply = c.reply();
                            *this.credential.lock().unwrap() = c;
                            if this
                                .rpc()
                                .unwrap()
                                .send(&json!({"id":id,"result":reply}))
                                .await
                                .is_err()
                            {
                                this.fail("Codex credential delivery failed.");
                            }
                        }
                        Err(_) => this.fail(
                            "Native Codex credentials require reauthentication before resume.",
                        ),
                    }
                });
                return Ok(());
            }
            if method != "item/tool/call" {
                return Err(conflict("Codex requested an unadmitted native operation."));
            }
            if self.failed.load(Ordering::SeqCst) {
                return Err(conflict("Codex tool admission is closed."));
            }
            let tool = text(p, "tool")?;
            let call = text(p, "callId")?;
            let turn = text(p, "turnId")?;
            let mut state = self.state.lock().unwrap();
            if p["threadId"] != state.thread
                || state.turn.as_deref() != Some(turn)
                || !self.names.contains(tool)
                || !p["namespace"].is_null()
                || !p["arguments"].is_object()
                || !(frame["id"].is_string() || frame["id"].is_number())
            {
                return Err(conflict(
                    "Codex tool address, schema or authority does not match its active turn.",
                ));
            }
            let key = stringify(&frame["id"]);
            if state.tools.contains_key(&key) {
                return Err(conflict("Codex reused an outstanding server request ID."));
            }
            let retained = self
                .journal
                .lock()
                .unwrap()
                .admit(call, tool, &p["arguments"], turn)?;
            state
                .tools
                .insert(key.clone(), (frame["id"].clone(), call.into()));
            let logical = profile::digest(format!("{}:{call}", state.thread).as_bytes());
            drop(state);
            if let Some(result) = retained {
                let this = self.clone();
                tokio::spawn(async move {
                    if this.reply(&key, result).await.is_err() {
                        this.fail("Retained Codex tool result could not be delivered.");
                    }
                });
            } else {
                self.emit(json!({"type":"host_tool_call","id":key,"toolCallId":logical,"toolName":tool,"arguments":p["arguments"]}));
            }
            return Ok(());
        }
        if method.starts_with("item/") || method == "thread/tokenUsage/updated" {
            let state = self.state.lock().unwrap();
            if state.thread.is_empty() || p["threadId"] != state.thread {
                return Err(conflict("Codex notification belongs to another thread."));
            }
            if method.starts_with("item/")
                && (state.turn.is_none()
                    || p["turnId"].as_str().is_none_or(str::is_empty)
                    || p["turnId"].as_str() != state.turn.as_deref())
            {
                return Err(conflict("Codex item belongs to another turn."));
            }
        }
        match method {
            "model/rerouted"=>return Err(conflict("Codex attempted to reroute the frozen model.")),
            "turn/started"=>{
                let mut state=self.state.lock().unwrap();
                if p["threadId"]!=state.thread{return Err(conflict("Codex started a turn for another thread."));}
                let id=text(&p["turn"],"id")?.to_owned();
                let input=state.awaiting.take().ok_or_else(||conflict("Codex started an unrequested turn."))?;
                self.journal.lock().unwrap().record(json!({"kind":"turn","turnId":id,"inputId":input}))?;
                state.turn=Some(id.clone());state.inputs.push(input);drop(state);
                self.emit(json!({"type":"agent_start","turnId":id}));
            },
            "turn/completed"=>{
                let mut state=self.state.lock().unwrap();let id=text(&p["turn"],"id")?;
                if p["threadId"]!=state.thread || state.turn.as_deref()!=Some(id){return Err(conflict("Codex terminal turn identity changed."));}
                let status=match p["turn"]["status"].as_str(){Some("completed")=>"completed",Some("interrupted")=>"aborted",Some("failed")=>"error",_=>return Err(conflict("Codex terminal status is invalid."))};
                self.journal.lock().unwrap().record(json!({"kind":"terminal","turnId":id,"status":status}))?;
                if state.pending_steer.as_ref().is_some_and(|(_,turn)|turn==id){state.steer_terminal=Some(status.into());}
                let inputs=std::mem::take(&mut state.inputs);let pending:Vec<_>=state.tools.keys().cloned().collect();state.turn=None;state.compacting=false;drop(state);
                for key in pending {self.emit(json!({"type":"host_tool_cancel","targetId":key}));}
                for input in inputs {self.emit(json!({"type":"prompt_result","id":input,"status":status,"error":if status=="error"{json!({"message":"Codex turn failed.","retryable":false})}else{Value::Null}}));}
                self.emit(json!({"type":"session_settled"}));
            },
            "item/agentMessage/delta" | "item/reasoning/summaryTextDelta"=>{
                self.emit(json!({"type":"message_update","messageId":p["itemId"],"assistantMessageEvent":{"type":if method=="item/agentMessage/delta"{"text_delta"}else{"thinking_delta"},"delta":p["delta"]}}));
            },
            "item/started" | "item/completed"=>{
                let item=&p["item"];let kind=text(item,"type")?;
                if !["userMessage","agentMessage","reasoning","dynamicToolCall","contextCompaction"].contains(&kind){return Err(conflict("Codex emitted an unadmitted native tool item."));}
                if kind=="contextCompaction" {self.state.lock().unwrap().compacting=method=="item/started";
                    self.emit(json!({"type":"harness_compaction","state":if method=="item/started"{"started"}else{"completed"}}));}
                if method=="item/completed" && kind=="agentMessage" {self.emit(json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":item["text"]}]}}));}
            },
            "thread/tokenUsage/updated"=>{
                self.usage(&p["tokenUsage"]["total"])?;
            },
            "error"=>self.emit(json!({"type":"harness_error","willRetry":p["willRetry"],"reason":"Codex reported a provider error."})),
            _=>(),
        }
        Ok(())
    }
    fn usage(&self, raw: &Value) -> Result<()> {
        let usage = self.journal.lock().unwrap().usage(raw)?;
        let thread = self.state.lock().unwrap().thread.clone();
        // Replayable snapshot closes the crash window between two journals.
        self.emit(json!({"type":"provider_usage","state":"snapshot","usage":usage,"sessionId":thread,"source":"codex-native","accounting":"cumulative","cost":null}));
        Ok(())
    }
    pub async fn state(&self) -> Result<Value> {
        if self.failed.load(Ordering::SeqCst) {
            return Err(conflict("Codex assignment requires reconciliation."));
        }
        let state = self.state.lock().unwrap();
        Ok(
            json!({"isStreaming":state.turn.is_some()||state.awaiting.is_some()||state.pending_steer.is_some(),"isCompacting":state.compacting,"sessionId":state.thread}),
        )
    }
    pub async fn submit(&self, id: &str, message: &str, steer: bool) -> Result<()> {
        let _guard = self.submit_lock.lock().await;
        if self.failed.load(Ordering::SeqCst) {
            return Err(conflict("Codex assignment requires reconciliation."));
        }
        let (thread, turn) = {
            let mut state = self.state.lock().unwrap();
            if state.compacting
                || state.awaiting.is_some()
                || state.pending_steer.is_some()
                || state.turn.is_some() && !steer
            {
                return Err(conflict("Codex is busy."));
            }
            self.journal.lock().unwrap().record(
                json!({"kind":"input","id":id,"hash":profile::digest(message.as_bytes())}),
            )?;
            if state.turn.is_none() {
                state.awaiting = Some(id.into());
            } else {
                state.pending_steer = Some((id.into(), state.turn.clone().unwrap()));
            }
            (state.thread.clone(), state.turn.clone())
        };
        let mut params = json!({"threadId":thread,"input":[{"type":"text","text":message}]});
        let method = if let Some(turn) = &turn {
            params["expectedTurnId"] = json!(turn);
            "turn/steer"
        } else {
            "turn/start"
        };
        if self.rpc()?.command(method, params).await.is_err() {
            self.fail(
                "Codex input dispatch was not confirmed; reconcile retained history before retry.",
            );
            return Err(conflict("Codex input dispatch was not confirmed."));
        }
        if let Some(turn) = turn {
            self.accept_steer(id, &turn)?;
        }
        Ok(())
    }
    fn accept_steer(&self, id: &str, turn: &str) -> Result<()> {
        let terminal = {
            let mut state = self.state.lock().unwrap();
            if state.pending_steer.take() != Some((id.into(), turn.to_owned())) {
                return Err(conflict("Codex steering identity changed."));
            }
            self.journal
                .lock()
                .unwrap()
                .record(json!({"kind":"turn","turnId":turn,"inputId":id}))?;
            let terminal = state.steer_terminal.take();
            if terminal.is_none() {
                state.inputs.push(id.into());
            }
            terminal
        };
        if let Some(status) = terminal {
            self.emit(json!({"type":"prompt_result","id":id,"status":status}));
        }
        Ok(())
    }
    async fn reply(&self, key: &str, result: Value) -> Result<()> {
        let (id, call) = self
            .state
            .lock()
            .unwrap()
            .tools
            .get(key)
            .cloned()
            .ok_or_else(|| conflict("Unknown Codex server request."))?;
        self.journal.lock().unwrap().result(&call, &result)?;
        self.rpc()?.send(&json!({"id":id,"result":result})).await?;
        self.state.lock().unwrap().tools.remove(key);
        Ok(())
    }
    pub async fn resolve_tool(&self, frame: &Value) -> Result<()> {
        if frame["outcome"] == "unresolved" {
            self.fail("Host tool outcome requires reconciliation. The original intent is retained and no completed result was asserted.");
            return Err(conflict("Host tool outcome is unresolved."));
        }
        let content: Vec<_> = frame["result"]["content"]
            .as_array()
            .ok_or_else(|| conflict("Invalid host result."))?
            .iter()
            .map(|v| json!({"type":"inputText","text":v["text"]}))
            .collect();
        self.reply(
            text(frame, "id")?,
            json!({"success":frame["isError"]!=true,"contentItems":content}),
        )
        .await
    }
    pub async fn interrupt(&self) -> Result<()> {
        let current = {
            let state = self.state.lock().unwrap();
            state
                .turn
                .as_ref()
                .map(|id| (state.thread.clone(), id.clone()))
        };
        if let Some((thread, turn)) = current {
            self.rpc()?
                .command("turn/interrupt", json!({"threadId":thread,"turnId":turn}))
                .await?;
        }
        Ok(())
    }
    pub async fn close(&self) -> Result<()> {
        self.rpc()?.close().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(root: &Path) -> (Arc<Codex>, Arc<Mutex<Vec<Value>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let observed = events.clone();
        let mut journal = SessionJournal::open(root, &json!({"assignment":"fixture"})).unwrap();
        journal.bind_thread("thread").unwrap();
        let runner = Arc::new(Codex {
            rpc: OnceLock::new(),
            event: Arc::new(move |v| observed.lock().unwrap().push(v)),
            config: json!({}),
            state: Mutex::new(State {
                thread: "thread".into(),
                turn: Some("turn".into()),
                inputs: vec!["original".into()],
                ..State::default()
            }),
            journal: Mutex::new(journal),
            names: BTreeSet::from(["write".into()]),
            credential: Mutex::new(auth::Credentials {
                token: "fake".into(),
                account: "fake".into(),
            }),
            submit_lock: AsyncMutex::new(()),
            refresh_lock: AsyncMutex::new(()),
            failed: AtomicBool::new(false),
            _lease: File::create(root.join("fixture-lease")).unwrap(),
        });
        (runner, events)
    }
    #[test]
    fn terminal_before_steering_reply_does_not_acknowledge_rejected_or_unknown_input() {
        for accepted in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().canonicalize().unwrap();
            let (runner, events) = fixture(&path);
            runner.state.lock().unwrap().pending_steer = Some(("steer".into(), "turn".into()));
            runner
                .journal
                .lock()
                .unwrap()
                .record(json!({"kind":"input","id":"steer"}))
                .unwrap();
            runner.handle(json!({"method":"turn/completed","params":{"threadId":"thread","turn":{"id":"turn","status":"completed"}}})).unwrap();
            assert!(
                !events
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|e| e["type"] == "prompt_result" && e["id"] == "steer")
            );
            if accepted {
                runner.accept_steer("steer", "turn").unwrap();
            } else {
                runner.fail("Rejected steering.");
            }
            assert_eq!(
                events
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|e| e["type"] == "prompt_result" && e["id"] == "steer")
                    .count(),
                usize::from(accepted)
            );
            drop(runner);
            let journal = SessionJournal::open(&path, &json!({"assignment":"fixture"})).unwrap();
            assert_eq!(journal.reconcile(&json!({"turns":[]})).is_ok(), accepted);
        }
    }
    #[test]
    fn notifications_from_another_thread_or_turn_cannot_mutate_usage_or_output() {
        let root = tempfile::tempdir().unwrap();
        let (runner, events) = fixture(root.path());
        let before = runner.journal.lock().unwrap().usage.clone();
        assert!(runner.handle(json!({"method":"thread/tokenUsage/updated","params":{"threadId":"other","tokenUsage":{"total":{"totalTokens":999,"inputTokens":999,"outputTokens":0,"cachedInputTokens":0,"reasoningOutputTokens":0}}}})).is_err());
        assert_eq!(runner.journal.lock().unwrap().usage, before);
        for (thread, turn) in [("other", "turn"), ("thread", "other")] {
            assert!(runner.handle(json!({"method":"item/agentMessage/delta","params":{"threadId":thread,"turnId":turn,"itemId":"message","delta":"Wrong assignment"}})).is_err());
        }
        runner.state.lock().unwrap().turn = None;
        assert!(runner.handle(json!({"method":"item/agentMessage/delta","params":{"threadId":"thread","delta":"Idle unsolicited output"}})).is_err());
        assert!(events.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn a_lost_backend_reply_retains_unknown_effect_and_closes_admission() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().canonicalize().unwrap();
        let (runner, events) = fixture(&path);
        runner.handle(json!({"id":7,"method":"item/tool/call","params":{"threadId":"thread","turnId":"turn","callId":"original-effect","tool":"write","arguments":{"text":"one"}}})).unwrap();
        assert_eq!(
            events
                .lock()
                .unwrap()
                .iter()
                .filter(|e| e["type"] == "host_tool_call")
                .count(),
            1
        );
        assert!(
            runner
                .resolve_tool(&json!({"id":"7","outcome":"unresolved","isError":true}))
                .await
                .is_err()
        );
        assert!(runner.state().await.is_err());
        assert!(runner.handle(json!({"id":8,"method":"item/tool/call","params":{"threadId":"thread","turnId":"turn","callId":"new-effect","tool":"write","arguments":{"text":"one"}}})).is_err());
        drop(runner);
        let journal = SessionJournal::open(&path, &json!({"assignment":"fixture"})).unwrap();
        assert!(journal.reconcile(&json!({"turns":[]})).is_err());
        assert_eq!(
            events
                .lock()
                .unwrap()
                .iter()
                .filter(|e| e["type"] == "host_tool_call")
                .count(),
            1
        );
    }
}
