//! The board's fixed output transport layout (128-byte header plus append stage).
use obc_formats::io::{ByteSink, Error};
pub struct NavStageSink<'a> {
    pub stage: &'a mut [u8; crate::arena::NAV_OUTPUT_STAGE_BYTES],
    pub appended: usize,
    pub patch_len: usize,
}
impl ByteSink for NavStageSink<'_> {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let start = 128 + self.appended;
        let end = start.checked_add(bytes.len()).ok_or(Error::TooLarge)?;
        self.stage.get_mut(start..end).ok_or(Error::TooLarge)?.copy_from_slice(bytes);
        self.appended += bytes.len();
        Ok(())
    }
    fn patch_at(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Error> {
        if offset != 0 || bytes.len() > 128 {
            return Err(Error::BadOffset);
        }
        self.stage[..bytes.len()].copy_from_slice(bytes);
        self.patch_len = bytes.len();
        Ok(())
    }
}
