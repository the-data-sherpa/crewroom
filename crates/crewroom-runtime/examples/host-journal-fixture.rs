//! Test-only process for crash and actual failed-write journal evidence.
use crewroom_core::Result;
use crewroom_runtime::{
    host_journal::{EventJournal, InputJournal},
    host_protocol::redact,
};
use serde_json::{Value, json};
use std::{
    io::{BufRead, Write},
    path::Path,
};
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let config: Value = serde_json::from_str(&lines.next().unwrap()?)?;
    let root = Path::new(config["root"].as_str().unwrap());
    let mut events = EventJournal::open(root)?;
    let mut inputs = InputJournal::open(root)?;
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: writable pointer, valid resource constant; affects this fixture only.
    if unsafe { libc::getrlimit(libc::RLIMIT_FSIZE, &mut limit) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    println!("{{\"ready\":true}}");
    std::io::stdout().flush()?;
    for line in lines {
        let v: Value = serde_json::from_str(&line?)?;
        let result = (|| -> Result<Value> {
            let id = v["key"].as_str().unwrap_or("");
            Ok(match v["op"].as_str().unwrap() {
                "accept" => json!(inputs.accept(id, v["message"].as_str().unwrap())?),
                "received" => {
                    inputs.received(id)?;
                    Value::Null
                }
                "completed" => {
                    inputs.completed(id, v["status"].as_str().unwrap())?;
                    Value::Null
                }
                "register" => {
                    inputs.register(id, v["message"].as_str().unwrap())?;
                    Value::Null
                }
                "status" => json!(inputs.disposition(id)?),
                "event" => json!(events.append(redact(&v["event"], &["fixture-secret".into()]))?),
                "page" => json!(events.page(v["after"].as_f64().unwrap_or(0.0))?),
                "sequence" => json!(events.sequence()),
                "file_limit" => {
                    let new = libc::rlimit {
                        rlim_cur: v["bytes"].as_u64().unwrap_or(limit.rlim_cur),
                        rlim_max: limit.rlim_max,
                    };
                    // SAFETY: fixture-local signal disposition and valid limit pointer.
                    unsafe {
                        libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
                    }
                    if unsafe { libc::setrlimit(libc::RLIMIT_FSIZE, &new) } != 0 {
                        return Err(std::io::Error::last_os_error().into());
                    }
                    Value::Null
                }
                _ => panic!("Unknown fixture operation"),
            })
        })();
        let response = match result {
            Ok(value) => json!({"id":v["id"],"ok":true,"value":value}),
            Err(e) => json!({"id":v["id"],"ok":false,"error":e.to_string()}),
        };
        println!("{}", crewroom_core::serialized::stringify(&response));
        std::io::stdout().flush()?;
    }
    Ok(())
}
