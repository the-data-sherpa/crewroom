//! Private fsynced host journals. A partial append is uncertain dispatch, never an
//! empty inbox. Event observers retain offsets, not every completed event payload.
use crate::{canonical, conflict, flock};
use crewroom_core::{Result, serialized::stringify};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

pub const MAX_RECORD_BYTES: usize = crate::rpc_frame::MAX_REASSEMBLED_BYTES + 4096;
pub const EVENT_PAGE_BYTES: usize = 4 * 1024 * 1024;
pub const EVENT_PAGE_RECORDS: usize = 128;

/// Read one bounded physical record without allocating an unbounded unterminated line.
fn line(reader: &mut impl BufRead) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            break;
        }
        let count = available
            .iter()
            .position(|b| *b == b'\n')
            .map_or(available.len(), |n| n + 1);
        if bytes.len() + count > MAX_RECORD_BYTES {
            return Err(conflict("Host journal record exceeded its limit."));
        }
        let finished = available[count - 1] == b'\n';
        bytes.extend_from_slice(&available[..count]);
        reader.consume(count);
        if finished {
            break;
        }
    }
    Ok(bytes)
}

pub(crate) struct Journal {
    path: PathBuf,
    file: File,
    device: u64,
    inode: u64,
    modified: (i64, i64),
    changed: (i64, i64),
    len: u64,
    offsets: Vec<(u64, usize)>,
    failed: bool,
}
impl Journal {
    pub(crate) fn open(path: &Path, mut inspect: impl FnMut(Value) -> Result<()>) -> Result<Self> {
        canonical(
            path.parent()
                .ok_or_else(|| conflict("Missing host journal parent."))?,
        )?;
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(path)?;
        if !flock(&file, libc::LOCK_EX | libc::LOCK_NB)? {
            return Err(conflict("Host journal already has a writer."));
        }
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(conflict("Host journal must be a private regular file."));
        }
        let mut offsets = Vec::new();
        let mut len = 0;
        {
            let mut reader = BufReader::new(&mut file);
            loop {
                let bytes = line(&mut reader)?;
                if bytes.is_empty() {
                    break;
                }
                if !bytes.ends_with(b"\n") {
                    return Err(conflict("Host journal has an incomplete append."));
                }
                inspect(serde_json::from_slice(&bytes)?)?;
                offsets.push((len, bytes.len()));
                len += bytes.len() as u64;
            }
        }
        let journal = Self {
            path: path.into(),
            file,
            device: metadata.dev(),
            inode: metadata.ino(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
            len,
            offsets,
            failed: false,
        };
        journal.validate()?;
        journal.file.sync_all()?;
        File::open(path.parent().unwrap())?.sync_all()?;
        Ok(journal)
    }
    pub(crate) fn validate(&self) -> Result<()> {
        if self.failed {
            return Err(conflict("Host journal append is uncertain."));
        }
        canonical(self.path.parent().unwrap())?;
        let actual = std::fs::symlink_metadata(&self.path)?;
        if !actual.is_file()
            || actual.dev() != self.device
            || actual.ino() != self.inode
            || actual.len() != self.len
            || (actual.mtime(), actual.mtime_nsec()) != self.modified
            || (actual.ctime(), actual.ctime_nsec()) != self.changed
            || actual.nlink() != 1
            || actual.permissions().mode() & 0o077 != 0
        {
            return Err(conflict("Host journal identity or contents changed."));
        }
        Ok(())
    }
    pub(crate) fn append(&mut self, value: &Value) -> Result<()> {
        self.validate()?;
        let mut bytes = stringify(value).into_bytes();
        bytes.push(b'\n');
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(conflict("Host journal record exceeded its limit."));
        }
        // Set the poison before the first write: no failed/partial append can be
        // acknowledged or followed by another write in this process.
        self.failed = true;
        self.file.seek(SeekFrom::Start(self.len))?;
        self.file.write_all(&bytes)?;
        self.file.sync_all()?;
        let metadata = self.file.metadata()?;
        self.modified = (metadata.mtime(), metadata.mtime_nsec());
        self.changed = (metadata.ctime(), metadata.ctime_nsec());
        self.offsets.push((self.len, bytes.len()));
        self.len += bytes.len() as u64;
        self.failed = false;
        let result = self.validate();
        self.failed = result.is_err();
        result
    }
    fn record(&mut self, index: usize) -> Result<Value> {
        let (offset, len) = self.offsets[index];
        self.file.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0; len];
        self.file.read_exact(&mut bytes)?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}

pub struct EventJournal {
    journal: Journal,
}
impl EventJournal {
    pub fn open(root: &Path) -> Result<Self> {
        let mut sequence = 0_u64;
        let journal = Journal::open(&root.join("events.jsonl"), |value| {
            sequence += 1;
            if value["sequence"].as_f64() != Some(sequence as f64) || !value["event"].is_object() {
                return Err(conflict("Host event journal sequence is incomplete."));
            }
            Ok(())
        })?;
        Ok(Self { journal })
    }
    pub fn sequence(&self) -> u64 {
        self.journal.offsets.len() as u64
    }
    /// Caller projects and redacts the event before persistence. The returned
    /// sequence is visible only after the complete append has reached storage.
    pub fn append(&mut self, event: Value) -> Result<u64> {
        if !event.is_object() || self.sequence() >= 9_007_199_254_740_991 {
            return Err(conflict("Invalid host event."));
        }
        let sequence = self.sequence() + 1;
        self.journal
            .append(&json!({"sequence":sequence,"event":event}))?;
        Ok(sequence)
    }
    /// Same cursor always reads the same events. A single large completed event
    /// remains intact even when it exceeds the ordinary four-MiB page budget.
    pub fn page(&mut self, after: f64) -> Result<Vec<Value>> {
        self.journal.validate()?;
        let start = if after.is_nan() || after < 0.0 {
            0
        } else {
            after.floor() as usize
        };
        let end = start
            .saturating_add(EVENT_PAGE_RECORDS)
            .min(self.journal.offsets.len());
        let mut result = Vec::new();
        let mut bytes = 0;
        for index in start..end {
            let size = self.journal.offsets[index].1;
            if !result.is_empty() && bytes + size > EVENT_PAGE_BYTES {
                break;
            }
            let value = self.journal.record(index)?;
            if value["sequence"].as_f64() != Some((index + 1) as f64) || !value["event"].is_object()
            {
                return Err(conflict("Host event journal changed."));
            }
            bytes += size;
            result.push(value);
        }
        self.journal.validate()?;
        Ok(result)
    }
}

fn merge_input(
    inputs: &mut Vec<Value>,
    indices: &mut HashMap<String, usize>,
    value: Value,
) -> Result<()> {
    let Some(id) = value["id"].as_str() else {
        return Err(conflict(
            "Input journal is incomplete; reconcile it before continuation.",
        ));
    };
    if !value["disposition"].is_string() {
        return Err(conflict(
            "Input journal is incomplete; reconcile it before continuation.",
        ));
    }
    if let Some(index) = indices.get(id) {
        let previous = &mut inputs[*index];
        if ["completed", "error", "aborted"]
            .iter()
            .any(|s| previous["disposition"] == *s)
            && value["disposition"] == "received"
        {
            return Ok(());
        }
        previous
            .as_object_mut()
            .unwrap()
            .extend(value.as_object().unwrap().clone());
    } else {
        indices.insert(id.into(), inputs.len());
        inputs.push(value);
    }
    Ok(())
}
pub fn parse_saved_inputs(bytes: &[u8]) -> Result<Vec<Value>> {
    let mut reader = BufReader::new(bytes);
    let mut inputs = Vec::new();
    let mut indices = HashMap::new();
    loop {
        let bytes = line(&mut reader)?;
        if bytes.is_empty() {
            break;
        }
        if bytes == b"\n" {
            continue;
        }
        merge_input(&mut inputs, &mut indices, serde_json::from_slice(&bytes)?)?;
    }
    Ok(inputs)
}
pub fn saved_inputs(root: &Path) -> Result<Vec<Value>> {
    let path = root.join("inputs.jsonl");
    // Only actual absence means no receipt. Permission, symlink and parse errors
    // must remain errors at restart/reconciliation boundaries.
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e.into()),
    };
    if !file.metadata()?.is_file() {
        return Err(conflict("Input journal must be a regular file."));
    }
    let mut reader = BufReader::new(file);
    let mut inputs = Vec::new();
    let mut indices = HashMap::new();
    loop {
        let bytes = line(&mut reader)?;
        if bytes.is_empty() {
            break;
        }
        if bytes == b"\n" {
            continue;
        }
        merge_input(&mut inputs, &mut indices, serde_json::from_slice(&bytes)?)?;
    }
    Ok(inputs)
}

pub struct InputJournal {
    journal: Journal,
    inputs: Vec<Value>,
    indices: HashMap<String, usize>,
    bodies: HashMap<String, String>,
}
impl InputJournal {
    pub fn open(root: &Path) -> Result<Self> {
        let mut inputs = Vec::new();
        let mut indices = HashMap::new();
        let journal = Journal::open(&root.join("inputs.jsonl"), |value| {
            merge_input(&mut inputs, &mut indices, value)
        })?;
        let bodies = inputs
            .iter()
            .filter_map(|v| {
                v["message"]
                    .as_str()
                    .map(|s| (v["id"].as_str().unwrap().into(), s.into()))
            })
            .collect();
        Ok(Self {
            journal,
            inputs,
            indices,
            bodies,
        })
    }
    pub fn disposition(&self, id: &str) -> Result<Option<&str>> {
        self.journal.validate()?;
        Ok(self
            .indices
            .get(id)
            .map(|i| self.inputs[*i]["disposition"].as_str().unwrap()))
    }
    /// Record body identity before the busy check. A failed preflight may retry
    /// the same body, but cannot reuse its key with different content.
    pub fn register(&mut self, id: &str, message: &str) -> Result<()> {
        self.journal.validate()?;
        if id.is_empty() {
            return Err(conflict("Input identity and message are required."));
        }
        if self.bodies.get(id).is_some_and(|body| body != message) {
            return Err(conflict("Input identity has different content."));
        }
        self.bodies.insert(id.into(), message.into());
        Ok(())
    }
    /// Persist admission before any prompt can be sent. False means an already
    /// acknowledged/terminal keyed replay, never permission to dispatch again.
    pub fn accept(&mut self, id: &str, message: &str) -> Result<bool> {
        self.register(id, message)?;
        if let Some(state) = self.disposition(id)? {
            if state == "accepted" {
                return Err(conflict("Input dispatch outcome is uncertain."));
            }
            return Ok(false);
        }
        self.append(json!({"id":id,"message":message,"disposition":"accepted"}))?;
        Ok(true)
    }
    pub fn received(&mut self, id: &str) -> Result<()> {
        if self.disposition(id)? == Some("accepted") {
            self.append(json!({"id":id,"disposition":"received"}))?;
        }
        Ok(())
    }
    pub fn completed(&mut self, id: &str, status: &str) -> Result<()> {
        if !["completed", "error", "aborted"].contains(&status) {
            return Err(conflict("Invalid terminal input disposition."));
        }
        if self.disposition(id)?.is_some() {
            self.append(json!({"id":id,"disposition":status}))?;
        }
        Ok(())
    }
    fn append(&mut self, value: Value) -> Result<()> {
        self.journal.append(&value)?;
        merge_input(&mut self.inputs, &mut self.indices, value)
    }
}
