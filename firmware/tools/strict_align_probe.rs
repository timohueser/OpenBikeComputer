#![no_std]

// Deliberately align-1 input: without +strict-align LLVM combines these byte reads into one `ldr`
// on thumbv8m; with the flag it must emit four `ldrb`s. `resource_guard.py strict-align` compiles
// both forms and checks that the probe still discriminates on the selected toolchain.
#[no_mangle]
pub unsafe extern "C" fn decode_u32(bytes: *const u8) -> u32 {
    u32::from_le_bytes([unsafe { *bytes }, unsafe { *bytes.add(1) }, unsafe { *bytes.add(2) }, unsafe {
        *bytes.add(3)
    }])
}
