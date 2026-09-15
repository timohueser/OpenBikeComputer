#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataError {
    Unsupported,
    WriteFailed,
    Busy,
    RemountRequired,
    Stale,
}
