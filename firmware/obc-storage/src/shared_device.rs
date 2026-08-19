//! A block device borrowed by shared reference.
//!
//! `embedded-sdmmc::VolumeManager` takes its device by value. Tests and the retiring OBC2
//! compatibility benches still need to observe the same instrumented device after handing it to
//! the manager, so they pass this tiny forwarding handle instead. It is deliberately independent
//! of the deleted FAT extent-map implementation: sharing a test device is not an extent feature.

use embedded_sdmmc::{Block, BlockCount, BlockDevice, BlockIdx};

/// Forward [`BlockDevice`] calls to a borrowed device.
pub struct SharedBlockDevice<'a, D: BlockDevice>(pub &'a D);

impl<D: BlockDevice> BlockDevice for SharedBlockDevice<'_, D> {
    type Error = D::Error;

    fn read(&self, blocks: &mut [Block], start_block_idx: BlockIdx) -> Result<(), Self::Error> {
        self.0.read(blocks, start_block_idx)
    }

    fn write(&self, blocks: &[Block], start_block_idx: BlockIdx) -> Result<(), Self::Error> {
        self.0.write(blocks, start_block_idx)
    }

    fn num_blocks(&self) -> Result<BlockCount, Self::Error> {
        self.0.num_blocks()
    }
}
