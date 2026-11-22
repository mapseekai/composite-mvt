/// Policy for repeated layer names across distinct sources.
///
/// Each source must declare each layer name once. This policy controls names shared by sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DuplicateLayer {
    /// Allow distinct sources to declare the same layer name.
    ///
    /// This is an explicit opt-in that may produce output not conforming to MVT 2.1. Byte-for-byte
    /// identical layer names within one tile are forbidden by the
    /// [MVT 2.1 specification](https://github.com/mapbox/vector-tile-spec/blob/master/2.1/README.md#41-layers).
    Allow,
    /// Reject a layer name shared by distinct sources. This is the default policy.
    #[default]
    Error,
    /// Keep each shared layer from the earliest source that contains it in the current tile.
    KeepFirst,
    /// Keep each shared layer from the latest source that contains it in the current tile.
    KeepLast,
}
