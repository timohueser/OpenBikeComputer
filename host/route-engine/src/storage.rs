use flate2::{read::ZlibDecoder, write::ZlibEncoder, Compression};
use serde::{de::DeserializeOwned, Serialize};
use std::io::{Read, Write};

pub const MAX_PAGE_BYTES: usize = 8 * 1024 * 1024;

pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    let raw = postcard::to_allocvec(value).map_err(|e| e.to_string())?;
    if raw.len() > MAX_PAGE_BYTES {
        return Err("Page exceeds encode budget".into());
    }
    let mut writer = ZlibEncoder::new(Vec::new(), Compression::fast());
    writer.write_all(&raw).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())
}

pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    let mut raw = Vec::new();
    ZlibDecoder::new(bytes).take((MAX_PAGE_BYTES + 1) as u64).read_to_end(&mut raw).map_err(|e| e.to_string())?;
    if raw.len() > MAX_PAGE_BYTES {
        return Err("Page exceeds decode budget".into());
    }
    postcard::from_bytes(&raw).map_err(|e| e.to_string())
}
