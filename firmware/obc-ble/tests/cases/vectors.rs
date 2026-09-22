//! The production codecs against the live shared `specs/vectors/` fixtures.

use obc_ble::{CommandResult, Config, Crc32, StatusMessage};

fn fixture(name: &str) -> Vec<u8> {
    let path = obc_vectors::dir().join(name);
    std::fs::read(&path).unwrap_or_else(|e| {
        panic!("fixture {name} unreadable ({e}) — run `cargo run -p obc-vectors --example regenerate --locked`")
    })
}

fn decoded_command_result(bytes: &[u8]) -> CommandResult {
    match StatusMessage::decode(bytes).unwrap().expect("known discriminator") {
        StatusMessage::CommandResult(result) => result,
    }
}

#[test]
fn production_crc_matches_reference() {
    let route = fixture("route-waypoints.obcr");
    assert_eq!(Crc32::checksum(&route), obc_vectors::crc32(&route));
}

#[test]
fn command_result_vector() {
    use obc_ble::{CommandStatus, CMD_INSTALL_FW};

    let result_bytes = fixture("status-command-result.bin");
    let result = decoded_command_result(&result_bytes);
    assert_eq!((result.command, result.status, result.detail), (CMD_INSTALL_FW, CommandStatus::Ok, 0));
    let (buf, len) = StatusMessage::CommandResult(CommandResult::new(CMD_INSTALL_FW, CommandStatus::Ok)).encode();
    assert_eq!(&buf[..len], &result_bytes[..]);
}

#[test]
fn command_forget_bond_round_trip() {
    use obc_ble::{CommandStatus, CMD_FORGET_BOND};

    assert_eq!(CMD_FORGET_BOND, 4);
    let (buf, len) = StatusMessage::CommandResult(CommandResult::new(CMD_FORGET_BOND, CommandStatus::Ok)).encode();
    let result = decoded_command_result(&buf[..len]);
    assert_eq!((result.command, result.status, result.detail), (CMD_FORGET_BOND, CommandStatus::Ok, 0));
}

#[test]
fn command_set_clock_vector() {
    use obc_ble::{CommandStatus, SetClock, CMD_SET_CLOCK};

    assert_eq!(CMD_SET_CLOCK, 5);
    let bytes = fixture("command-set-clock.bin");
    assert_eq!(bytes.len(), SetClock::ENCODED_LEN);
    let clock = SetClock::decode(&bytes).expect("valid setClock");
    assert_eq!(clock.utc, 1_783_598_400);
    assert_eq!(clock.offset_min, 120);

    let mut out = [0u8; SetClock::ENCODED_LEN];
    let len = SetClock::encode(clock.utc, clock.offset_min, &mut out).unwrap();
    assert_eq!(&out[..len], &bytes[..]);

    let (buf, len) = StatusMessage::CommandResult(CommandResult::new(CMD_SET_CLOCK, CommandStatus::Ok)).encode();
    let result = decoded_command_result(&buf[..len]);
    assert_eq!((result.command, result.status, result.detail), (CMD_SET_CLOCK, CommandStatus::Ok, 0));
}

#[test]
fn set_clock_decode_edges() {
    use obc_ble::{SetClock, SET_CLOCK_MAX_OFFSET_MIN, SET_CLOCK_MIN_UTC};

    let valid = |utc: u32, off: i16| {
        let mut bytes = [0u8; SetClock::ENCODED_LEN];
        SetClock::encode(utc, off, &mut bytes).unwrap();
        bytes
    };

    assert!(SetClock::decode(&[5, 0, 0, 0, 0, 0]).is_err());
    assert!(SetClock::decode(&[5, 0, 0, 0, 0, 0, 0, 0]).is_err());
    assert!(SetClock::decode(&valid_cmd(4, SET_CLOCK_MIN_UTC, 0)).is_err());
    assert!(SetClock::decode(&valid(SET_CLOCK_MIN_UTC - 1, 0)).is_err());
    assert!(SetClock::decode(&valid(0, 0)).is_err());
    assert!(SetClock::decode(&valid(SET_CLOCK_MIN_UTC, SET_CLOCK_MAX_OFFSET_MIN + 1)).is_err());
    assert!(SetClock::decode(&valid(SET_CLOCK_MIN_UTC, -SET_CLOCK_MAX_OFFSET_MIN - 1)).is_err());
    assert!(SetClock::decode(&valid(SET_CLOCK_MIN_UTC, SET_CLOCK_MAX_OFFSET_MIN)).is_ok());
    assert!(SetClock::decode(&valid(SET_CLOCK_MIN_UTC, -SET_CLOCK_MAX_OFFSET_MIN)).is_ok());
    assert!(SetClock::decode(&valid(SET_CLOCK_MIN_UTC, 0)).is_ok());
}

fn valid_cmd(cmd: u8, utc: u32, offset_min: i16) -> [u8; 7] {
    let mut bytes = [0u8; 7];
    bytes[0] = cmd;
    bytes[1..5].copy_from_slice(&utc.to_le_bytes());
    bytes[5..7].copy_from_slice(&offset_min.to_le_bytes());
    bytes
}

#[test]
fn config_vector() {
    let bytes = fixture("config-v1.bin");
    let config = Config::decode(&bytes).expect("valid config");
    assert_eq!(config.name, b"OBC Tourer");
    assert_eq!(config.units, 0);

    let mut out = [0u8; Config::MAX_ENCODED];
    let len = Config::encode(&config, &mut out).unwrap();
    assert_eq!(&out[..len], &bytes[..]);
}

#[test]
fn unknown_status_discriminator_is_ignored() {
    assert_eq!(StatusMessage::decode(&[0xEE, 0, 0, 0]), Ok(None));
}
