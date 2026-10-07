//! Initialize an attached device card through the shared native store client.

use nusb::DeviceInfo;
use obc_link::flat::{
    client::{Error as ClientError, Options, Outcome},
    wire::{detail, FormatRequest, ListRequest, Request},
    ErrorCode, StoreId,
};
use obc_usb::{
    client::{Error, NativeClient},
    PRODUCT_ID, VENDOR_ID,
};
use std::io::{self, Write};

const ZERO_STORE: [u8; 16] = [0; 16];

#[derive(Debug, Default, PartialEq, Eq)]
struct Args {
    yes: bool,
    serial: Option<String>,
}

#[tokio::main]
async fn main() {
    if let Err(message) = run().await {
        eprintln!("error: {message}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let args = parse_args(std::env::args().skip(1))?;
    let info = choose_device(args.serial.as_deref()).await?;
    let label = device_label(&info);
    let mut client = NativeClient::open(&info, Options::default()).await.map_err(|e| e.to_string())?;
    let (_, cancel) = tokio::sync::watch::channel(false);
    let result = async {
        let list = client
            .execute(Request::List(ListRequest { kind: None, cursor: None }), None, None, None, &cancel, |_, _| {})
            .await;
        let expected = store_from_list(list)?;
        let state = if expected == ZERO_STORE {
            "unformatted or catalog-unreadable".to_owned()
        } else {
            format!("flat store {}", hex(&expected))
        };
        eprintln!("Device: {label}");
        eprintln!("Card:   {state}");
        eprintln!();
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
        let response = client
            .execute(
                Request::Format(FormatRequest { expected: StoreId(expected), replacement: StoreId(replacement) }),
                None,
                None,
                None,
                &cancel,
                |_, _| {},
            )
            .await;
        let formatted = store_from_format(response, replacement)?;
        println!(
            "Card formatted as empty store {}. The device is restarting; reconnect and upload a map.",
            hex(&formatted)
        );
        Ok(())
    }
    .await;
    client.close().await;
    result
}

fn store_from_list(result: Result<Outcome, Error>) -> Result<[u8; 16], String> {
    match result {
        Ok(Outcome::Catalog { store, .. }) if store.0 != ZERO_STORE => Ok(store.0),
        Err(Error::Client(ClientError::Remote(refusal)))
            if refusal.code == ErrorCode::ReadOnly
                && matches!(refusal.detail, detail::read_only::CATALOG_UNREADABLE | detail::read_only::UNFORMATTED) =>
        {
            Ok(ZERO_STORE)
        }
        Err(Error::Client(ClientError::Remote(refusal))) => Err(refusal_message("LIST", refusal)),
        Err(error) => Err(error.to_string()),
        _ => Err("LIST did not return a nonzero card identity".into()),
    }
}

fn store_from_format(result: Result<Outcome, Error>, expected: [u8; 16]) -> Result<[u8; 16], String> {
    match result {
        Ok(Outcome::Format(store)) if store.0 == expected && expected != ZERO_STORE => Ok(store.0),
        Err(Error::Client(ClientError::Remote(refusal))) => Err(refusal_message("FORMAT", refusal)),
        Err(error) => Err(error.to_string()),
        _ => Err("FORMAT did not return the confirmed replacement card identity".into()),
    }
}

fn refusal_message(operation: &str, refusal: obc_link::flat::Refusal) -> String {
    format!(
        "device refused {operation}: code {}, detail {}, context {}",
        refusal.code.value(),
        refusal.detail,
        refusal.context
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use obc_link::flat::Refusal;

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
        use obc_link::flat::wire::{encode_request, RequestId};
        let mut frame = [0u8; 100];
        let len = encode_request(
            &mut frame,
            RequestId(0x0403_0201),
            Request::Format(FormatRequest { expected: StoreId([0x11; 16]), replacement: StoreId([0x22; 16]) }),
        )
        .unwrap();
        assert_eq!(len, 48);
        assert_eq!(&frame[..16], &[0x4f, 0x42, 0x43, 0x34, 4, 8, 0, 0, 32, 0, 0, 0, 1, 2, 3, 4]);
        assert_eq!(&frame[16..32], &[0x11; 16]);
        assert_eq!(&frame[32..48], &[0x22; 16]);
    }

    #[test]
    fn list_selects_a_readable_identity_or_the_two_format_recovery_states() {
        let list = Ok(Outcome::Catalog { store: StoreId([0x55; 16]), sequence: 17, entries: Vec::new() });
        assert_eq!(store_from_list(list).unwrap(), [0x55; 16]);
        for detail in [detail::read_only::CATALOG_UNREADABLE, detail::read_only::UNFORMATTED] {
            assert_eq!(
                store_from_list(Err(Error::Client(ClientError::Remote(Refusal {
                    code: ErrorCode::ReadOnly,
                    detail,
                    context: 0
                }))))
                .unwrap(),
                ZERO_STORE
            );
        }
        assert!(store_from_list(Err(Error::Client(ClientError::Remote(Refusal {
            code: ErrorCode::NoSpace,
            detail: 2,
            context: 0
        }))))
        .unwrap_err()
        .contains("code 6"));
    }

    #[test]
    fn format_requires_the_correlated_nonzero_identity() {
        assert_eq!(store_from_format(Ok(Outcome::Format(StoreId([0x66; 16]))), [0x66; 16]).unwrap(), [0x66; 16]);
        assert!(store_from_format(Ok(Outcome::Format(StoreId([0x66; 16]))), [0x77; 16]).is_err());
        assert!(store_from_format(Ok(Outcome::Format(StoreId(ZERO_STORE))), ZERO_STORE).is_err());
    }
}
