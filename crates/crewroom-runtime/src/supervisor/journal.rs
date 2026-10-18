//! Passive streaming recovery. Only complete event records have durable receipt;
//! input journals retain their stricter uncertain-dispatch semantics.
use crate::{conflict, host_journal::MAX_RECORD_BYTES};
use crewroom_core::Result;
use serde_json::Value;
use std::{
    fs::OpenOptions,
    io::{BufRead, BufReader},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

pub fn visit_events(root: &Path, mut record: impl FnMut(u64, &Value) -> Result<()>) -> Result<u64> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(root.join("events.jsonl"))
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e.into()),
    };
    if !file.metadata()?.is_file() {
        return Err(conflict("Event journal must be a regular file."));
    }
    let mut reader = BufReader::new(file);
    let mut sequence = 0;
    loop {
        let mut line = Vec::new();
        loop {
            let available = reader.fill_buf()?;
            if available.is_empty() {
                return Ok(sequence);
            }
            let length = available
                .iter()
                .position(|b| *b == b'\n')
                .map_or(available.len(), |i| i + 1);
            if line.len() + length > MAX_RECORD_BYTES {
                return Err(conflict("Event journal record exceeded its bound."));
            }
            let complete = available[length - 1] == b'\n';
            line.extend_from_slice(&available[..length]);
            reader.consume(length);
            if complete {
                break;
            }
        }
        let value: Value = serde_json::from_slice(&line)?;
        if value["sequence"].as_u64() != Some(sequence + 1) || !value["event"].is_object() {
            return Err(conflict(
                "Saved event journal has an invalid sequence or payload.",
            ));
        }
        sequence += 1;
        record(sequence, &value["event"])?;
    }
}
