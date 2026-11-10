use parity_scale_codec::{Decode, Encode};

/// An NFT purse identifier; the maximum value is reserved for the main purse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub struct PurseId(pub u32);

impl PurseId {
    /// The built-in purse, excluded from allocated identifiers.
    pub const MAIN: Self = Self(u32::MAX);
}

/// Application metadata matched as opaque bytes.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct PurseTag(pub Vec<u8>);

/// A purse returned by the shared operations.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct PurseRecord {
    /// Stable database identifier.
    pub id: PurseId,
    /// Caller-provided display name.
    pub name: String,
    /// Opaque grouping metadata.
    pub tag: PurseTag,
}
