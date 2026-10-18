//! Bounded decoding of OMP protocol-v2 output. Commands remain plain JSONL.
//! Wire reference: oh-my-pi/v18.4.12/packages/coding-agent/src/modes/rpc/rpc-frame.ts.
//! OMP 18.11.3 keeps the same chunk and error frames; its encoder only serializes a
//! shared `message_update` snapshot once, byte-identical to `JSON.stringify`.
use crate::conflict;
use base64::{Engine, engine::general_purpose::STANDARD};
use crewroom_core::Result;
use serde_json::Value;

pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
pub const MAX_REASSEMBLED_BYTES: usize = 64 * 1024 * 1024;
const CHUNK_BYTES: usize = 256 * 1024;
struct Pending {
    id: String,
    count: usize,
    length: usize,
    next: usize,
    bytes: Vec<u8>,
}
#[derive(Default)]
pub struct Decoder {
    pending: Option<Pending>,
    failed: bool,
}
fn integer(value: &Value) -> Option<usize> {
    value
        .as_f64()
        .filter(|v| {
            v.is_finite() && *v >= 0.0 && v.fract() == 0.0 && *v <= MAX_REASSEMBLED_BYTES as f64
        })
        .map(|v| v as usize)
}
impl Decoder {
    /// Each physical frame includes its newline in the byte ceiling. No partial logical
    /// frame is exposed, and a corrupt sequence permanently closes this decoder.
    pub fn line(&mut self, line: &[u8]) -> Result<Option<Value>> {
        if self.failed {
            return Err(conflict("RPC framing already failed."));
        }
        let result = (|| {
            if line.len() > MAX_FRAME_BYTES {
                return Err(conflict("RPC frame exceeded the transport limit."));
            }
            let value: Value = serde_json::from_slice(line)?;
            self.push(value)
        })();
        if result.is_err() {
            self.failed = true;
            self.pending = None;
        }
        result
    }
    fn push(&mut self, value: Value) -> Result<Option<Value>> {
        if value["type"] == "rpc_frame_error" {
            return Err(conflict("OMP could not deliver a complete RPC frame."));
        }
        if value["type"] != "rpc_chunk" {
            if self.pending.is_some() {
                return Err(conflict("RPC chunk sequence interrupted."));
            }
            if !value.is_object() {
                return Err(conflict("RPC frame must be an object."));
            }
            return Ok(Some(value));
        }
        let id = value["chunkId"]
            .as_str()
            .filter(|s| !s.is_empty() && s.encode_utf16().count() <= 128)
            .ok_or_else(|| conflict("Invalid RPC chunk metadata."))?;
        let index =
            integer(&value["index"]).ok_or_else(|| conflict("Invalid RPC chunk metadata."))?;
        let count = integer(&value["count"])
            .filter(|v| (2..=MAX_REASSEMBLED_BYTES / CHUNK_BYTES).contains(v))
            .ok_or_else(|| conflict("Invalid RPC chunk metadata."))?;
        let length = integer(&value["byteLength"])
            .filter(|v| (MAX_FRAME_BYTES..=MAX_REASSEMBLED_BYTES).contains(v))
            .ok_or_else(|| conflict("Invalid RPC chunk metadata."))?;
        if index >= count {
            return Err(conflict("Invalid RPC chunk metadata."));
        }
        let data = value["data"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= CHUNK_BYTES.div_ceil(3) * 4)
            .ok_or_else(|| conflict("Invalid RPC chunk data."))?;
        let bytes = STANDARD
            .decode(data)
            .map_err(|_| conflict("Invalid RPC chunk data."))?;
        if STANDARD.encode(&bytes) != data || bytes.len() > CHUNK_BYTES {
            return Err(conflict("Invalid RPC chunk data."));
        }
        if self.pending.is_none() {
            if index != 0 {
                return Err(conflict("RPC chunk sequence must start at index 0."));
            }
            self.pending = Some(Pending {
                id: id.into(),
                count,
                length,
                next: 0,
                bytes: Vec::with_capacity(length),
            });
        }
        let pending = self.pending.as_mut().unwrap();
        if pending.id != id
            || pending.count != count
            || pending.length != length
            || pending.next != index
        {
            return Err(conflict("RPC chunk sequence mismatch."));
        }
        if pending.bytes.len() + bytes.len() > length {
            return Err(conflict("RPC chunk sequence exceeds declared length."));
        }
        pending.bytes.extend(bytes);
        pending.next += 1;
        if pending.next < count {
            return Ok(None);
        }
        if pending.bytes.len() != length {
            return Err(conflict("RPC chunk sequence length mismatch."));
        }
        let pending = self.pending.take().unwrap();
        let value: Value = serde_json::from_slice(&pending.bytes)?;
        if !value.is_object() {
            return Err(conflict("RPC frame must be an object."));
        }
        Ok(Some(value))
    }
    pub fn finish(&self) -> Result<()> {
        if self.failed || self.pending.is_some() {
            Err(conflict(
                "RPC stream ended with an incomplete or invalid frame.",
            ))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn chunks(value: &Value) -> Vec<Value> {
        let bytes = serde_json::to_vec(value).unwrap();
        let count = bytes.len().div_ceil(CHUNK_BYTES);
        bytes.chunks(CHUNK_BYTES).enumerate().map(|(index,chunk)|json!({"type":"rpc_chunk","chunkId":"fixture","index":index,"count":count,"byteLength":bytes.len(),"data":STANDARD.encode(chunk)})).collect()
    }
    fn line(value: &Value) -> Vec<u8> {
        let mut bytes = serde_json::to_vec(value).unwrap();
        bytes.push(b'\n');
        bytes
    }
    #[test]
    fn large_tool_arguments_and_responses_are_complete_across_utf8_boundaries() {
        for size in [MAX_FRAME_BYTES, MAX_FRAME_BYTES + 1, 2 * MAX_FRAME_BYTES] {
            let value =
                json!({"type":"host_tool_call","arguments":{"content":"🦀".repeat(size/4)}});
            let mut decoder = Decoder::default();
            let frames = chunks(&value);
            let mut received = Vec::new();
            for frame in frames {
                if let Some(v) = decoder.line(&line(&frame)).unwrap() {
                    received.push(v);
                }
            }
            assert_eq!(received, vec![value]);
            decoder.finish().unwrap();
        }
    }
    #[test]
    fn corrupt_incomplete_and_interleaved_chunks_never_dispatch() {
        let value =
            json!({"type":"host_tool_call","arguments":{"content":"x".repeat(MAX_FRAME_BYTES)}});
        let frames = chunks(&value);
        for malformed in [
            json!(null),
            json!({"type":"response"}),
            {
                let mut v = frames[1].clone();
                v["index"] = json!(0);
                v
            },
            {
                let mut v = frames[1].clone();
                v["chunkId"] = json!("other");
                v
            },
            {
                let mut v = frames[1].clone();
                v["byteLength"] = json!(MAX_REASSEMBLED_BYTES + 1);
                v
            },
            {
                let mut v = frames[1].clone();
                v["data"] = json!("!bad!");
                v
            },
        ] {
            let mut decoder = Decoder::default();
            assert!(decoder.line(&line(&frames[0])).unwrap().is_none());
            assert!(decoder.line(&line(&malformed)).is_err());
            assert!(decoder.finish().is_err());
            assert!(decoder.line(&line(&json!({"type":"response"}))).is_err());
        }
        let mut decoder = Decoder::default();
        decoder.line(&line(&frames[0])).unwrap();
        assert!(decoder.finish().is_err());
        assert!(
            Decoder::default()
                .line(&vec![b' '; MAX_FRAME_BYTES + 1])
                .is_err()
        );
        assert!(Decoder::default().line(b"not-json\n").is_err());
        assert!(
            Decoder::default()
                .line(&line(&json!({"type":"rpc_frame_error"})))
                .is_err()
        );
    }
}
