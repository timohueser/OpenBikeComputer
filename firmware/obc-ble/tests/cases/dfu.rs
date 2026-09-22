//! The live DFU command/result seam.

use obc_ble::{CommandStatus, CMD_INSTALL_FW};

#[test]
fn install_fw_reply_matrix() {
    assert_eq!(obc_ble::install_fw_reply(false), CommandStatus::Ok);
    assert_eq!(obc_ble::install_fw_reply(true), CommandStatus::Busy);
}

#[test]
fn install_fw_command_byte_is_three() {
    assert_eq!(CMD_INSTALL_FW, 3);
}
