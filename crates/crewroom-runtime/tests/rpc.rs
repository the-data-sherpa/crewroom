//! Actual owned processes with scripted RPC replies; real OMP has a separate guarded harness.
use crewroom_runtime::{CommandSpec, rpc::OmpRpc};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::{Instant, sleep, timeout};
const HELPER: &str = env!("CARGO_BIN_EXE_crewroom-process");
const SCRIPT: &str = r#"
import sys,json,time,threading,base64
lock=threading.Lock()
def emit(v):
 raw=json.dumps(v,ensure_ascii=False,separators=(',',':')).encode()
 with lock:
  if len(raw)+1<=1048576: print(raw.decode(),flush=True)
  else:
   count=(len(raw)+262143)//262144
   for i in range(count): print(json.dumps({'type':'rpc_chunk','chunkId':'large','index':i,'count':count,'byteLength':len(raw),'data':base64.b64encode(raw[i*262144:(i+1)*262144]).decode()}),flush=True)
def reply(f,value=None): emit({'type':'response','id':f['id'],'command':f['type'],'success':True,'data':value})
def later(f): time.sleep(.3);reply(f,f.get('value'))
for line in sys.stdin:
 f=json.loads(line);kind=f['type']
 if kind=='negotiate_protocol': reply(f,{'protocolVersion':2})
 elif kind=='echo': emit({'type':'message_end','message':{'text':'event'}});reply(f,f.get('value'))
 elif kind=='delay': threading.Thread(target=later,args=(f,),daemon=True).start()
 elif kind=='large':
  text='🦀'*300000
  emit({'type':'host_tool_call','id':'event-id','toolCallId':'call-id','toolName':'fixture','arguments':{'content':text}})
  reply(f,{'text':text})
 elif kind=='stop_reading': reply(f);time.sleep(30)
 elif kind=='malformed': print('{',flush=True)
 elif kind=='no_reply': pass
 elif kind=='abort': reply(f)
 else: emit({'type':'response','id':f['id'],'command':kind,'success':False,'error':'private detail'})
"#;
struct Fixture {
    data: Option<tempfile::TempDir>,
    rpc: OmpRpc,
    events: Arc<Mutex<Vec<Value>>>,
}
impl Fixture {
    async fn new() -> Self {
        let data = tempfile::tempdir().unwrap();
        let path = data.path().canonicalize().unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let observed = events.clone();
        let spec = CommandSpec {
            command: "/usr/bin/env".into(),
            args: vec!["python3".into(), "-u".into(), "-c".into(), SCRIPT.into()],
            cwd: path,
            env: BTreeMap::from([("PATH".into(), std::env::var("PATH").unwrap())]),
        };
        let rpc = OmpRpc::start(Path::new(HELPER), &spec, move |v| {
            observed.lock().unwrap().push(v)
        })
        .await
        .unwrap();
        Self {
            data: Some(data),
            rpc,
            events,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // Even a failed assertion reconciles the real test child before deleting its lease.
        let result = std::process::Command::new(HELPER)
            .arg("stop")
            .arg(&self.rpc.ownership_root)
            .output();
        let stopped = result
            .ok()
            .and_then(|r| serde_json::from_slice::<Value>(&r.stdout).ok())
            .is_some_and(|v| v["state"] == "stopped");
        if !stopped {
            eprintln!(
                "Retaining uncertain RPC fixture: {}",
                self.data.take().unwrap().keep().display()
            );
        }
    }
}
#[tokio::test]
async fn correlated_responses_keep_events_separate_and_reassemble_large_tool_content() {
    let fixture = Fixture::new().await;
    assert_eq!(
        fixture
            .rpc
            .command("echo", json!({"value":17}))
            .await
            .unwrap(),
        17
    );
    assert!(
        fixture
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "message_end")
    );
    assert_eq!(
        fixture
            .rpc
            .command("rejected", json!({}))
            .await
            .unwrap_err()
            .to_string(),
        "OMP rejected rejected."
    );
    let large = fixture.rpc.command("large", json!({})).await.unwrap();
    let expected = "🦀".repeat(300000);
    assert_eq!(large["text"], expected);
    assert!(
        fixture
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "host_tool_call" && e["arguments"]["content"] == expected)
    );
    fixture.rpc.close().await.unwrap();
}
#[tokio::test]
async fn duplicate_pending_id_does_not_displace_the_original_request() {
    let fixture = Fixture::new().await;
    let rpc = fixture.rpc.clone();
    let original = tokio::spawn(async move {
        rpc.command_with_id("delay", json!({"value":123}), "same")
            .await
    });
    sleep(Duration::from_millis(30)).await;
    assert!(
        fixture
            .rpc
            .command_with_id("echo", json!({"value":0}), "same")
            .await
            .unwrap_err()
            .to_string()
            .contains("already pending")
    );
    assert_eq!(
        fixture
            .rpc
            .command("echo", json!({"value":"independent"}))
            .await
            .unwrap(),
        "independent"
    );
    assert_eq!(original.await.unwrap().unwrap(), 123);
    fixture.rpc.close().await.unwrap();
}
#[tokio::test]
async fn command_timeout_yields_and_keeps_unknown_identity_reserved() {
    let fixture = Fixture::new().await;
    let rpc = fixture.rpc.clone();
    let started = Instant::now();
    let waiting =
        tokio::spawn(async move { rpc.command_with_id("no_reply", json!({}), "timeout").await });
    sleep(Duration::from_millis(30)).await;
    assert_eq!(
        fixture
            .rpc
            .command("echo", json!({"value":true}))
            .await
            .unwrap(),
        true
    );
    assert_eq!(
        waiting.await.unwrap().unwrap_err().to_string(),
        "OMP no_reply timed out."
    );
    assert!(started.elapsed() >= Duration::from_secs(19));
    assert!(started.elapsed() < Duration::from_secs(24));
    assert!(
        fixture
            .rpc
            .command_with_id("echo", json!({}), "timeout")
            .await
            .unwrap_err()
            .to_string()
            .contains("unresolved")
    );
    assert_eq!(
        fixture
            .rpc
            .command("echo", json!({"value":"fresh"}))
            .await
            .unwrap(),
        "fresh"
    );
    fixture.rpc.close().await.unwrap();
}
#[tokio::test]
async fn cancelled_pipe_write_revokes_transport_and_close_still_stops_the_reader() {
    let fixture = Fixture::new().await;
    fixture
        .rpc
        .command("stop_reading", json!({}))
        .await
        .unwrap();
    let huge =
        json!({"type":"host_tool_result","id":"partial","result":{"text":"x".repeat(4*1024*1024)}});
    assert!(
        timeout(Duration::from_millis(100), fixture.rpc.send(&huge))
            .await
            .is_err()
    );
    assert!(fixture.rpc.command("echo", json!({})).await.is_err());
    timeout(Duration::from_secs(5), fixture.rpc.close())
        .await
        .unwrap()
        .unwrap();
}
#[tokio::test]
async fn malformed_stream_fails_pending_requests_and_cannot_dispatch_more_work() {
    let fixture = Fixture::new().await;
    assert!(fixture.rpc.command("malformed", json!({})).await.is_err());
    assert!(fixture.rpc.command("echo", json!({})).await.is_err());
    fixture.rpc.close().await.unwrap();
    assert!(
        fixture
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "rpc_stream_closed")
    );
}

#[tokio::test]
async fn cancelled_response_cannot_be_mistaken_for_a_reused_command_id() {
    let fixture = Fixture::new().await;
    assert!(
        timeout(
            Duration::from_millis(50),
            fixture
                .rpc
                .command_with_id("delay", json!({"value":"old"}), "cancelled")
        )
        .await
        .is_err()
    );
    assert!(
        fixture
            .rpc
            .command_with_id("echo", json!({"value":"new"}), "cancelled")
            .await
            .unwrap_err()
            .to_string()
            .contains("unresolved")
    );
    sleep(Duration::from_millis(400)).await;
    assert_eq!(
        fixture
            .rpc
            .command_with_id("echo", json!({"value":"new"}), "cancelled")
            .await
            .unwrap(),
        "new"
    );
    fixture.rpc.close().await.unwrap();
}

#[tokio::test]
async fn cancelled_negotiation_reconciles_the_owned_child() {
    let data = tempfile::tempdir().unwrap();
    let path = data.path().canonicalize().unwrap();
    let spec = CommandSpec {
        command: "/usr/bin/env".into(),
        args: vec![
            "python3".into(),
            "-u".into(),
            "-c".into(),
            "import time;time.sleep(30)".into(),
        ],
        cwd: path.clone(),
        env: BTreeMap::from([("PATH".into(), std::env::var("PATH").unwrap())]),
    };
    assert!(
        timeout(
            Duration::from_millis(100),
            OmpRpc::start(Path::new(HELPER), &spec, |_| {})
        )
        .await
        .is_err()
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if crewroom_runtime::inspect_owned(&path.join("omp-process"), true)
            .await
            .state
            == "stopped"
        {
            break;
        }
        if Instant::now() >= deadline {
            let state = crewroom_runtime::stop_owned(&path.join("omp-process")).await;
            if state.state != "stopped" {
                eprintln!("Retaining uncertain fixture {}", data.keep().display());
            }
            panic!("Cancelled negotiation did not reconcile ownership");
        }
        sleep(Duration::from_millis(50)).await;
    }
}
