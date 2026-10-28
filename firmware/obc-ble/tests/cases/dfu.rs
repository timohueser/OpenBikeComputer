//! The live DFU command/result seam.

use obc_ble::CommandStatus;

#[test]
fn install_fw_reply_matrix() {
    assert_eq!(obc_ble::install_fw_reply(false), CommandStatus::Ok);
    assert_eq!(obc_ble::install_fw_reply(true), CommandStatus::Busy);
}
