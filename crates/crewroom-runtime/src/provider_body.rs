//! Decode HTTP content codings before protocol inspection, as Node fetch does.
//! Keep the stream bounded and cancellation owned by the gateway's outer select.
use async_compression::tokio::bufread::{
    BrotliDecoder, DeflateDecoder, GzipDecoder, ZlibDecoder, ZstdDecoder,
};
use futures_util::{Stream, TryStreamExt};
use std::{io, pin::Pin};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio_util::io::{ReaderStream, StreamReader};
type Reader = Pin<Box<dyn AsyncRead + Send>>;
pub(crate) fn validate(response: &reqwest::Response) -> io::Result<()> {
    let count = response
        .headers()
        .get_all("content-encoding")
        .iter()
        .map(|v| v.as_bytes().split(|b| *b == b',').count())
        .sum::<usize>();
    if ![204, 205, 304].contains(&response.status().as_u16()) && count > 5 {
        return Err(io::Error::other("Too many provider content encodings."));
    }
    Ok(())
}
async fn decoded(response: reqwest::Response) -> io::Result<ReaderStream<Reader>> {
    let status = response.status().as_u16();
    let encoding = response
        .headers()
        .get_all("content-encoding")
        .iter()
        .map(|v| v.to_str().unwrap_or(""))
        .collect::<Vec<_>>()
        .join(",")
        .to_ascii_lowercase();
    let mut codings = if encoding.is_empty() || [204, 205, 304].contains(&status) {
        vec![]
    } else {
        encoding.split(',').map(str::trim).collect::<Vec<_>>()
    };
    if codings.len() > 5 {
        return Err(io::Error::other("Too many provider content encodings."));
    }
    if codings
        .iter()
        .any(|s| !["gzip", "x-gzip", "deflate", "br", "zstd"].contains(s))
    {
        codings.clear();
    }
    let mut reader: Reader = Box::pin(StreamReader::new(
        response.bytes_stream().map_err(io::Error::other),
    ));
    for coding in codings.into_iter().rev() {
        let mut buffered = BufReader::new(reader);
        if buffered.fill_buf().await?.is_empty() {
            reader = Box::pin(buffered);
            break;
        }
        reader = match coding {
            "gzip" | "x-gzip" => {
                let mut decoder = GzipDecoder::new(buffered);
                decoder.multiple_members(true);
                Box::pin(decoder)
            }
            // HTTP servers use both zlib-wrapped and raw DEFLATE. Node detects the
            // low nibble of the first byte; an empty encoded body stays empty.
            "deflate" => match buffered.fill_buf().await?.first() {
                Some(byte) if byte & 0x0f == 0x08 => Box::pin(ZlibDecoder::new(buffered)),
                Some(_) => Box::pin(DeflateDecoder::new(buffered)),
                None => Box::pin(buffered),
            },
            "br" => Box::pin(BrotliDecoder::new(buffered)),
            "zstd" => {
                let mut decoder = ZstdDecoder::new(buffered);
                decoder.multiple_members(true);
                Box::pin(decoder)
            }
            _ => unreachable!(),
        };
    }
    Ok(ReaderStream::with_capacity(reader, 64 * 1024))
}
pub(crate) fn stream(
    response: reqwest::Response,
) -> impl Stream<Item = io::Result<axum::body::Bytes>> + Send {
    futures_util::stream::once(decoded(response)).try_flatten()
}
