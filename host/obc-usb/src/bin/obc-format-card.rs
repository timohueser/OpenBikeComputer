//! Destructively initialize an attached device card through protocol-v4 FORMAT.
//!
//! This is intentionally a thin maintenance client: the same native USB byte transport as the
//! desktop app, plus the two control exchanges needed to discover the current StoreId and replace
//! it. FORMAT itself remains transport-neutral; this command uses USB because `obc` is a host CLI.

use std::io::{self, Write};

use nusb::DeviceInfo;
use obc_link::flat::{padded_record_len, Reassembler, RecordFault, RECORD_PREFIX_LEN};
use obc_usb::{OpenLink, PRODUCT_ID, VENDOR_ID};

const MAGIC: [u8; 4] = *b"OBC4";
const WIRE_MAJOR: u8 = 4;
const HEADER_LEN: usize = 16;
/// Host → device control ceiling (§5.2). The widest request is PUT; LIST and FORMAT are smaller.
const MAX_OUT_CONTROL_RECORD: usize = 256;
/// Device → host ceiling on either endpoint pair (§5.2). LIST can fill the whole control ceiling.
const MAX_IN_RECORD: usize = 8_208;
const EXCHANGE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

const LIST: u8 = 0x01;
const FORMAT: u8 = 0x08;
const RESPONSE: u16 = 1 << 0;
const ERROR: u16 = 1 << 1;
const MORE: u16 = 1 << 2;

const READ_ONLY: u16 = 14;
const CATALOG_UNREADABLE: u16 = 1;
const UNFORMATTED: u16 = 3;
const ZERO_STORE: [u8; 16] = [0; 16];

#[derive(Debug, Default, PartialEq, Eq)]
struct Args {
    yes: bool,
    serial: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Refusal {
    code: u16,
    detail: u16,
    context: u64,
}

#[derive(Debug, PartialEq, Eq)]
struct ResponseFrame<'a> {
    opcode: u8,
    flags: u16,
    request: u32,
    body: &'a [u8],
}

#[tokio::main]
async fn main() {
    match run().await {
        Ok(()) => {}
        Err(message) => {
            eprintln!("error: {message}");
            std::process::exit(1);
        }
    }
}

async fn run() -> Result<(), String> {
    let args = parse_args(std::env::args().skip(1))?;
    let info = choose_device(args.serial.as_deref()).await?;
    let label = device_label(&info);
    let device_id = format!("{:?}", info.id());
    let (link, _) = obc_usb::open(&info, device_id).await.map_err(|error| error.to_string())?;

    let list = exchange(&link, &list_request(1)).await?;
    let expected = store_from_list(&list, 1)?;
    let state = if expected == ZERO_STORE {
        "unformatted or catalog-unreadable".to_owned()
    } else {
        format!("flat store {}", hex(&expected))
    };

    eprintln!("Device: {label}");
    eprintln!("Card:   {state}");
    eprintln!();
    eprintln!("This permanently deletes the map, routes, trips, rides, weather and update packages.");
    if !args.yes {
        eprint!("Type FORMAT to erase and initialize this card: ");
        io::stderr().flush().map_err(|error| format!("could not show the confirmation prompt: {error}"))?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer).map_err(|error| format!("could not read confirmation: {error}"))?;
        if answer.trim() != "FORMAT" {
            return Err("confirmation did not match; card left unchanged".into());
        }
    }

    let replacement = mint_store_id(expected)?;
    let response = exchange(&link, &format_request(2, expected, replacement)).await?;
    let formatted = store_from_format(&response, 2)?;
    if formatted != replacement {
        return Err(format!("device reported replacement StoreId {}, expected {}", hex(&formatted), hex(&replacement)));
    }
    println!(
        "Card formatted as empty store {}. The device is restarting; reconnect and upload a map.",
        hex(&formatted)
    );
    Ok(())
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut args = args.into_iter();
    let mut parsed = Args::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--yes" | "-y" => parsed.yes = true,
            "--serial" => {
                let value = args.next().ok_or_else(|| "--serial needs a value".to_owned())?;
                if value.is_empty() {
                    return Err("--serial needs a non-empty value".into());
                }
                parsed.serial = Some(value);
            }
            "--help" | "-h" => {
                println!("usage: obc format-card [--serial DEVICE_SERIAL] [--yes]");
                println!();
                println!("Formats the card inside one USB-connected OpenBikeComputer as an empty flat store.");
                println!("Without --yes, the command requires typing FORMAT before it erases anything.");
                std::process::exit(0);
            }
            _ => {
                return Err(format!("unknown option `{arg}`; usage: obc format-card [--serial DEVICE_SERIAL] [--yes]"))
            }
        }
    }
    Ok(parsed)
}

async fn choose_device(serial: Option<&str>) -> Result<DeviceInfo, String> {
    let devices = nusb::list_devices().await.map_err(|error| format!("USB devices could not be listed: {error}"))?;
    let mut matches: Vec<DeviceInfo> = devices
        .filter(|info| info.vendor_id() == VENDOR_ID && info.product_id() == PRODUCT_ID)
        .filter(|info| serial.is_none_or(|wanted| info.serial_number() == Some(wanted)))
        .collect();
    match matches.len() {
        0 if serial.is_some() => {
            Err(format!("no attached OpenBikeComputer has serial {}", serial.expect("serial is present")))
        }
        0 => Err("no OpenBikeComputer is attached over USB".into()),
        1 => Ok(matches.remove(0)),
        _ => {
            let choices = matches.iter().map(device_label).collect::<Vec<_>>().join(", ");
            Err(format!("more than one device is attached ({choices}); choose one with --serial DEVICE_SERIAL"))
        }
    }
}

fn device_label(info: &DeviceInfo) -> String {
    match (info.product_string(), info.serial_number()) {
        (Some(product), Some(serial)) => format!("{product} ({serial})"),
        (Some(product), None) => product.to_owned(),
        (None, Some(serial)) => format!("OpenBikeComputer ({serial})"),
        (None, None) => "OpenBikeComputer".into(),
    }
}

async fn exchange(link: &OpenLink, frame: &[u8]) -> Result<Vec<u8>, String> {
    tokio::time::timeout(EXCHANGE_TIMEOUT, async {
        let framed = frame_record(frame)?;
        link.control.write(&framed).await.map_err(|error| error.to_string())?;
        read_record(link).await
    })
    .await
    .map_err(|_| format!("device did not answer within {} seconds", EXCHANGE_TIMEOUT.as_secs()))?
}

async fn read_record(link: &OpenLink) -> Result<Vec<u8>, String> {
    let mut pending = Vec::new();
    loop {
        let bytes = link.control.read().await.map_err(|error| error.to_string())?;
        pending.extend_from_slice(&bytes);
        if let Some(frame) = decode_record(&pending)? {
            return Ok(frame.to_vec());
        }
    }
}

fn frame_record(frame: &[u8]) -> Result<Vec<u8>, String> {
    if frame.is_empty() {
        return Err("control frame is empty".into());
    }
    if frame.len() > MAX_OUT_CONTROL_RECORD {
        return Err(format!(
            "control frame is {} bytes; USB binding v5 permits at most {MAX_OUT_CONTROL_RECORD}",
            frame.len()
        ));
    }
    let length = u32::try_from(frame.len()).map_err(|_| "control frame length does not fit binding v5".to_owned())?;
    let mut record = Vec::with_capacity(RECORD_PREFIX_LEN + padded_record_len(frame.len()));
    record.extend_from_slice(&length.to_le_bytes());
    record.extend_from_slice(frame);
    record.resize(RECORD_PREFIX_LEN + padded_record_len(frame.len()), 0);
    Ok(record)
}

/// Decode the single response record accumulated by this request/response maintenance client.
/// `None` means a prefix, frame, or padding is still split across reads. A second record is an
/// error because §3.1 has no unsolicited control frames and each request receives one answer.
fn decode_record(pending: &[u8]) -> Result<Option<&[u8]>, String> {
    let mut decoder = Reassembler::new(MAX_IN_RECORD);
    decoder.filled(pending.len());
    match decoder.take(pending).map_err(record_fault)? {
        None => Ok(None),
        Some((start, length)) => {
            let wire_len = RECORD_PREFIX_LEN + padded_record_len(length);
            if pending.len() != wire_len {
                return Err("device sent more than one response to a single maintenance request".into());
            }
            Ok(Some(&pending[start..start + length]))
        }
    }
}

fn record_fault(fault: RecordFault) -> String {
    match fault {
        RecordFault::ZeroLength => "device announced an empty control record".into(),
        RecordFault::OverCeiling { declared, ceiling } => {
            format!("device announced a {declared}-byte control record; binding v5 permits at most {ceiling}")
        }
        RecordFault::NonZeroPadding => "device sent non-zero USB record alignment padding".into(),
    }
}

fn list_request(request: u32) -> Vec<u8> {
    control_request(LIST, request, &[0; 32])
}

fn format_request(request: u32, expected: [u8; 16], replacement: [u8; 16]) -> Vec<u8> {
    let mut body = [0u8; 32];
    body[..16].copy_from_slice(&expected);
    body[16..].copy_from_slice(&replacement);
    control_request(FORMAT, request, &body)
}

fn control_request(opcode: u8, request: u32, body: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; HEADER_LEN + body.len()];
    out[..4].copy_from_slice(&MAGIC);
    out[4] = WIRE_MAJOR;
    out[5] = opcode;
    out[8..10].copy_from_slice(&(body.len() as u16).to_le_bytes());
    out[12..16].copy_from_slice(&request.to_le_bytes());
    out[HEADER_LEN..].copy_from_slice(body);
    out
}

fn store_from_list(record: &[u8], request: u32) -> Result<[u8; 16], String> {
    let response = response(record, LIST, request)?;
    if response.flags & ERROR != 0 {
        let refusal = refusal(response.body)?;
        if refusal.code == READ_ONLY && matches!(refusal.detail, CATALOG_UNREADABLE | UNFORMATTED) {
            return Ok(ZERO_STORE);
        }
        return Err(refusal_message("LIST", refusal));
    }
    if response.body.len() < 24 || (response.body.len() - 24) % 88 != 0 {
        return Err(format!("LIST response has invalid {}-byte body", response.body.len()));
    }
    let mut store = [0u8; 16];
    store.copy_from_slice(&response.body[..16]);
    if store == ZERO_STORE {
        return Err("LIST response names the reserved zero StoreId".into());
    }
    Ok(store)
}

fn store_from_format(record: &[u8], request: u32) -> Result<[u8; 16], String> {
    let response = response(record, FORMAT, request)?;
    if response.flags & ERROR != 0 {
        return Err(refusal_message("FORMAT", refusal(response.body)?));
    }
    if response.body.len() != 16 {
        return Err(format!("FORMAT response has invalid {}-byte body", response.body.len()));
    }
    let mut store = [0u8; 16];
    store.copy_from_slice(response.body);
    if store == ZERO_STORE {
        return Err("FORMAT response names the reserved zero StoreId".into());
    }
    Ok(store)
}

fn response(record: &[u8], opcode: u8, request: u32) -> Result<ResponseFrame<'_>, String> {
    if record.len() < HEADER_LEN {
        return Err("device response is shorter than the control header".into());
    }
    if record[..4] != MAGIC || record[4] != WIRE_MAJOR {
        return Err("device response does not speak protocol v4".into());
    }
    let actual_opcode = record[5];
    let flags = u16_at(record, 6);
    let declared = u16_at(record, 8) as usize;
    let reserved = u16_at(record, 10);
    let actual_request = u32_at(record, 12);
    if actual_opcode != opcode || actual_request != request {
        return Err(format!(
            "device answered opcode {actual_opcode:#04x}/request {actual_request}, expected {opcode:#04x}/{request}"
        ));
    }
    if reserved != 0 || flags & RESPONSE == 0 || flags & !(RESPONSE | ERROR | MORE) != 0 {
        return Err(format!("device response carries invalid flags/reserved fields ({flags:#06x}/{reserved:#06x})"));
    }
    if flags & MORE != 0 && opcode != LIST {
        return Err("device set LIST's more flag on a non-LIST response".into());
    }
    if flags & ERROR != 0 && flags & MORE != 0 {
        return Err("device set both error and more on one response".into());
    }
    if record.len() != HEADER_LEN + declared {
        return Err(format!(
            "device response declares {declared} body bytes but carries {}",
            record.len() - HEADER_LEN
        ));
    }
    Ok(ResponseFrame { opcode: actual_opcode, flags, request: actual_request, body: &record[HEADER_LEN..] })
}

fn refusal(body: &[u8]) -> Result<Refusal, String> {
    if body.len() != 16 {
        return Err(format!("device refusal has invalid {}-byte body", body.len()));
    }
    let code = u16_at(body, 0);
    if code == 0 || u32_at(body, 4) != 0 {
        return Err("device refusal has invalid code/reserved fields".into());
    }
    Ok(Refusal { code, detail: u16_at(body, 2), context: u64_at(body, 8) })
}

fn refusal_message(operation: &str, refusal: Refusal) -> String {
    format!("device refused {operation}: code {}, detail {}, context {}", refusal.code, refusal.detail, refusal.context)
}

fn mint_store_id(avoid: [u8; 16]) -> Result<[u8; 16], String> {
    loop {
        let mut id = [0u8; 16];
        getrandom::fill(&mut id).map_err(|error| format!("could not generate a replacement StoreId: {error}"))?;
        if id != ZERO_STORE && id != avoid {
            return Ok(id);
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four-byte field"))
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().expect("eight-byte field"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn success(opcode: u8, request: u32, body: &[u8], more: bool) -> Vec<u8> {
        let mut out = control_request(opcode, request, body);
        out[6..8].copy_from_slice(&(RESPONSE | if more { MORE } else { 0 }).to_le_bytes());
        out
    }

    fn error(opcode: u8, request: u32, code: u16, detail: u16) -> Vec<u8> {
        let mut body = [0u8; 16];
        body[..2].copy_from_slice(&code.to_le_bytes());
        body[2..4].copy_from_slice(&detail.to_le_bytes());
        let mut out = control_request(opcode, request, &body);
        out[6..8].copy_from_slice(&(RESPONSE | ERROR).to_le_bytes());
        out
    }

    #[test]
    fn arguments_keep_confirmation_on_by_default() {
        assert_eq!(parse_args(Vec::<String>::new()).unwrap(), Args::default());
        assert_eq!(
            parse_args(["--serial".into(), "abc".into(), "--yes".into()]).unwrap(),
            Args { yes: true, serial: Some("abc".into()) }
        );
        assert!(parse_args(["--serial".into()]).unwrap_err().contains("needs a value"));
    }

    #[test]
    fn format_request_is_the_frozen_v4_shape() {
        let frame = format_request(0x0403_0201, [0x11; 16], [0x22; 16]);
        assert_eq!(frame.len(), 48);
        assert_eq!(&frame[..16], &[0x4f, 0x42, 0x43, 0x34, 4, 8, 0, 0, 32, 0, 0, 0, 1, 2, 3, 4]);
        assert_eq!(&frame[16..32], &[0x11; 16]);
        assert_eq!(&frame[32..], &[0x22; 16]);
        let record = frame_record(&frame).unwrap();
        assert_eq!(&record[..4], &[48, 0, 0, 0]);
        assert_eq!(&record[4..], frame);
    }

    #[test]
    fn v5_encoder_zero_pads_to_the_next_word() {
        let record = frame_record(&[1, 2, 3, 4, 5]).unwrap();
        assert_eq!(&record[..4], &[5, 0, 0, 0]);
        assert_eq!(&record[4..9], &[1, 2, 3, 4, 5]);
        assert_eq!(&record[9..], &[0, 0, 0]);
        assert_eq!(record.len() % 4, 0);
    }

    #[test]
    fn v5_decoder_waits_for_every_split_prefix_frame_and_padding_byte() {
        let record = frame_record(&[1, 2, 3, 4, 5]).unwrap();
        for split in 0..record.len() {
            assert_eq!(decode_record(&record[..split]).unwrap(), None, "split at {split} completed early");
        }
        assert_eq!(decode_record(&record).unwrap(), Some(&[1, 2, 3, 4, 5][..]));
    }

    #[test]
    fn v5_decoder_reads_a_u32_length_and_rejects_bad_padding_or_an_extra_record() {
        let oversized_u32 = 65_552u32.to_le_bytes();
        assert!(decode_record(&oversized_u32).unwrap_err().contains("65552-byte"));

        let mut bad_padding = frame_record(&[0xaa]).unwrap();
        *bad_padding.last_mut().unwrap() = 1;
        assert!(decode_record(&bad_padding).unwrap_err().contains("non-zero"));

        let mut two = frame_record(&[1]).unwrap();
        two.extend_from_slice(&frame_record(&[2]).unwrap());
        assert!(decode_record(&two).unwrap_err().contains("more than one response"));
    }

    #[test]
    fn list_selects_a_readable_identity_or_the_two_format_recovery_states() {
        let mut body = [0u8; 24];
        body[..16].copy_from_slice(&[0x55; 16]);
        body[16..24].copy_from_slice(&17u64.to_le_bytes());
        assert_eq!(store_from_list(&success(LIST, 1, &body, false), 1).unwrap(), [0x55; 16]);
        for detail in [CATALOG_UNREADABLE, UNFORMATTED] {
            assert_eq!(store_from_list(&error(LIST, 1, READ_ONLY, detail), 1).unwrap(), ZERO_STORE);
        }
        assert!(store_from_list(&error(LIST, 1, 6, 2), 1).unwrap_err().contains("code 6"));
    }

    #[test]
    fn format_requires_the_correlated_nonzero_identity() {
        assert_eq!(store_from_format(&success(FORMAT, 2, &[0x66; 16], false), 2).unwrap(), [0x66; 16]);
        assert!(store_from_format(&success(FORMAT, 3, &[0x66; 16], false), 2).is_err());
        assert!(store_from_format(&success(FORMAT, 2, &[0; 16], false), 2).is_err());
    }
}
