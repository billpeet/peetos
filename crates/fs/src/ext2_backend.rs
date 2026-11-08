pub type Ext2BlockReadFn = fn(ctx: u64, block_index: u32, out: &mut [u8]) -> Result<(), i64>;
pub type Ext2BlockWriteFn = fn(ctx: u64, block_index: u32, data: &[u8]) -> Result<(), i64>;
/// Write barrier: when it returns `Ok`, every block previously passed to
/// `write_block` is on stable storage (no write cache can still reorder or
/// drop it). The journal relies on this between log blocks and the commit
/// record, after the commit record, and before retiring checkpointed log
/// space.
pub type Ext2BlockFlushFn = fn(ctx: u64) -> Result<(), i64>;

#[derive(Clone, Copy)]
pub struct Ext2BlockBackend {
    pub ctx: u64,
    pub read_block: Ext2BlockReadFn,
    pub write_block: Ext2BlockWriteFn,
    /// `None` means writes are durable as soon as `write_block` returns.
    pub flush: Option<Ext2BlockFlushFn>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Ext2BackendKind {
    None = 0,
    BlockOps = 1,
}

#[derive(Clone, Copy)]
pub enum Ext2Backend {
    None,
    BlockOps(Ext2BlockBackend),
}

impl Ext2Backend {
    pub const fn none() -> Self {
        Self::None
    }

    pub const fn kind(self) -> Ext2BackendKind {
        match self {
            Self::None => Ext2BackendKind::None,
            Self::BlockOps(_) => Ext2BackendKind::BlockOps,
        }
    }
}
