use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;

use fast_mvt::{MvtError, MvtReaderRef};

use crate::SourceError;
use crate::compression;
use crate::error::SourceErrorKind;

macro_rules! string_newtype {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name(Box<str>);

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.into())
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value.into_boxed_str())
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

string_newtype!(SourceId, "An owned identifier for a configured MVT source.");
string_newtype!(
    LayerName,
    "An owned name of a layer declared by an MVT source."
);

/// Compression associated with source input or the complete output.
///
/// `Gzip`, `Zstd`, and `Brotli` require Cargo features of the same lowercase names. `gzip` is
/// enabled by default; `zstd` and `brotli` are optional. [`Self::Other`] is a marker for an
/// unsupported external format and cannot be used to build a composer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    /// Raw, uncompressed bytes.
    None,
    /// RFC 1952 gzip bytes, enabled by the `gzip` feature.
    ///
    /// With `flate2` 1.1.9, each input member's optional file name and comment fields are
    /// limited to 65,535 bytes each, excluding their terminating NUL bytes.
    Gzip,
    /// Zstandard bytes, enabled by the `zstd` feature.
    Zstd,
    /// Brotli bytes, enabled by the `brotli` feature.
    Brotli,
    /// An external encoding unsupported by this crate.
    Other,
}

impl Compression {
    /// Returns the HTTP `Content-Encoding` token for a supported compressed output.
    ///
    /// The mappings are gzip to `gzip`, Zstandard to `zstd`, and Brotli to `br`. Raw and unknown
    /// encodings return `None`, so callers should omit `Content-Encoding` for them.
    #[must_use]
    pub const fn content_encoding(self) -> Option<&'static str> {
        match self {
            Self::None | Self::Other => None,
            Self::Gzip => Some("gzip"),
            Self::Zstd => Some("zstd"),
            Self::Brotli => Some("br"),
        }
    }
}

impl fmt::Display for Compression {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::None => "none",
            Self::Gzip => "gzip",
            Self::Zstd => "zstd",
            Self::Brotli => "brotli",
            Self::Other => "other",
        })
    }
}

/// Fixed metadata for one source in an [`crate::MvtComposer`].
///
/// The source's compression setting selects the decoder for [`crate::MvtComposer::compose`].
/// Use [`crate::MvtComposer::compose_with_encodings`] to supply an encoding for each input.
#[derive(Debug, Clone)]
pub struct MvtSource {
    id: SourceId,
    compression: Compression,
    layers: Box<[LayerName]>,
}

impl MvtSource {
    /// Creates a raw source with no declared layers.
    ///
    /// Add layers with [`Self::with_layers`] before using this source in a composer.
    #[must_use]
    pub fn new(id: impl Into<SourceId>) -> Self {
        Self {
            id: id.into(),
            compression: Compression::None,
            layers: Box::new([]),
        }
    }

    /// Sets the compression format used by [`crate::MvtComposer::compose`] for this source.
    ///
    /// Brotli input must be declared explicitly. The automatic sample constructors deliberately do
    /// not detect Brotli because it has no reliable signature.
    #[must_use]
    pub fn with_compression(mut self, compression: Compression) -> Self {
        self.compression = compression;
        self
    }

    /// Replaces the source's declared layers.
    ///
    /// Declare every layer this source can emit. Each tile may contain a subset of these layers.
    #[must_use]
    pub fn with_layers<I, S>(mut self, layers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<LayerName>,
    {
        self.layers = layers.into_iter().map(Into::into).collect();
        self
    }

    /// Reads source metadata from one representative MVT sample.
    ///
    /// This constructor detects gzip and Zstandard signatures, otherwise treating bytes as raw
    /// MVT. It deliberately does not auto-detect Brotli; use
    /// [`Self::from_mvt_with_compression`] with [`Compression::Brotli`] instead.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::from_mvt_with_compression`].
    pub fn from_mvt(id: impl Into<SourceId>, bytes: &[u8]) -> Result<Self, SourceError> {
        Self::from_mvt_with_compression(id, bytes, compression::detect_compression(bytes))
    }

    /// Reads source metadata from one sample using an explicitly declared compression format.
    ///
    /// Use this constructor for Brotli and whenever the input encoding is known externally.
    /// If a sample exceeds the MVT reader's element memory limit, use a smaller sample that
    /// covers the same layers or declare all source layers with [`Self::with_layers`].
    ///
    /// # Errors
    ///
    /// Returns a [`SourceError`] when the sample cannot be decoded, is not a usable MVT, or
    /// exceeds the reader's element memory limit.
    pub fn from_mvt_with_compression(
        id: impl Into<SourceId>,
        bytes: &[u8],
        compression: Compression,
    ) -> Result<Self, SourceError> {
        if bytes.is_empty() {
            return Err(SourceErrorKind::EmptyBytes.into());
        }

        let raw = compression::decompress(compression, bytes)?;
        Ok(Self {
            id: id.into(),
            compression,
            layers: read_layers(raw.as_ref())?,
        })
    }

    /// Reads layer names from multiple representative samples with auto-detected encoding.
    ///
    /// All samples must detect as the same raw, gzip, or Zstandard format. Use
    /// [`Self::from_mvts_with_compression`] for Brotli samples and for details on layer order and
    /// repeated names.
    ///
    /// # Errors
    ///
    /// Returns a [`SourceError`] for no samples, inconsistent detected encodings, or any sample
    /// rejected by [`Self::from_mvts_with_compression`].
    pub fn from_mvts<I, B>(id: impl Into<SourceId>, inputs: I) -> Result<Self, SourceError>
    where
        I: IntoIterator<Item = B>,
        B: AsRef<[u8]>,
    {
        let inputs: Vec<B> = inputs.into_iter().collect();
        let (first, rest) = inputs.split_first().ok_or(SourceErrorKind::NoSamples)?;
        let expected = compression::detect_compression(first.as_ref());

        for input in rest {
            let actual = compression::detect_compression(input.as_ref());
            if actual != expected {
                return Err(
                    SourceErrorKind::InconsistentSampleCompression { expected, actual }.into(),
                );
            }
        }

        Self::from_mvts_with_compression(id, inputs, expected)
    }

    /// Reads layer names from multiple samples using an explicit compression format.
    ///
    /// Collects layer names in first-seen order across samples. Additional occurrences of a name
    /// within a single sample are retained so the composer builder can reject the duplicate.
    /// Each sample has the same parsing limits as [`Self::from_mvt_with_compression`].
    ///
    /// # Errors
    ///
    /// Returns a [`SourceError`] if no samples are supplied or a sample is rejected by the
    /// same rules as [`Self::from_mvt_with_compression`].
    pub fn from_mvts_with_compression<I, B>(
        id: impl Into<SourceId>,
        inputs: I,
        compression: Compression,
    ) -> Result<Self, SourceError>
    where
        I: IntoIterator<Item = B>,
        B: AsRef<[u8]>,
    {
        let mut found_sample = false;
        let mut seen = HashSet::new();
        let mut layers = Vec::new();

        for input in inputs {
            found_sample = true;
            if input.as_ref().is_empty() {
                return Err(SourceErrorKind::EmptyBytes.into());
            }

            let raw = compression::decompress(compression, input.as_ref())?;
            let mut sample_seen = HashSet::new();
            for layer in read_layers(raw.as_ref())? {
                if !sample_seen.insert(layer.clone()) || !seen.contains(&layer) {
                    layers.push(layer);
                }
            }
            seen.extend(sample_seen);
        }

        if !found_sample {
            return Err(SourceErrorKind::NoSamples.into());
        }

        Ok(Self {
            id: id.into(),
            compression,
            layers: layers.into_boxed_slice(),
        })
    }

    /// Decodes request bytes according to this source's fixed compression.
    ///
    /// Raw and zero-byte inputs are returned as [`Cow::Borrowed`]. A zero-byte input requires no
    /// codec support. Nonempty compressed input is returned as owned decoded bytes.
    ///
    /// # Errors
    ///
    /// Returns a [`SourceError`] when a nonempty input uses a disabled or unsupported codec,
    /// or when decoding fails.
    pub fn decompress<'a>(&self, input: &'a [u8]) -> Result<Cow<'a, [u8]>, SourceError> {
        compression::decompress(self.compression, input)
    }

    /// Returns this source's stable identifier.
    #[must_use]
    pub fn id(&self) -> &SourceId {
        &self.id
    }

    /// Returns the configured compression used by [`crate::MvtComposer::compose`].
    #[must_use]
    pub const fn compression(&self) -> Compression {
        self.compression
    }

    /// Returns the fixed layer names in declaration order.
    #[must_use]
    pub fn layers(&self) -> &[LayerName] {
        &self.layers
    }
}

fn read_layers(bytes: &[u8]) -> Result<Box<[LayerName]>, SourceError> {
    let reader = MvtReaderRef::new(bytes).map_err(sample_parse_error)?;
    let layers: Box<[LayerName]> = reader.layers().map(|layer| layer.name().into()).collect();

    if layers.is_empty() {
        return Err(SourceErrorKind::NoLayers.into());
    }

    Ok(layers)
}

fn sample_parse_error(error: MvtError) -> SourceError {
    match error {
        MvtError::MissingLayerName => SourceErrorKind::MissingLayerName.into(),
        error @ MvtError::Decode(buffa::DecodeError::ElementMemoryLimitExceeded) => {
            SourceErrorKind::SampleElementMemoryLimit {
                cause: Box::new(error),
            }
            .into()
        }
        error => SourceError::invalid_mvt(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    #[test]
    fn sample_element_limit_is_reported_separately_from_invalid_mvt() {
        let error = sample_parse_error(MvtError::Decode(
            buffa::DecodeError::ElementMemoryLimitExceeded,
        ));

        assert!(!error.is_invalid_mvt());
        assert!(
            error
                .to_string()
                .starts_with("MVT sample exceeds the parser's element memory limit")
        );
        let cause = error
            .source()
            .and_then(|cause| cause.downcast_ref::<MvtError>());
        assert!(matches!(
            cause,
            Some(MvtError::Decode(
                buffa::DecodeError::ElementMemoryLimitExceeded
            ))
        ));
    }

    #[test]
    fn strong_names_convert_from_strings_and_display() {
        let source = SourceId::from("roads");
        let layer = LayerName::from(String::from("road_labels"));

        assert_eq!(source.as_ref(), "roads");
        assert_eq!(source.to_string(), "roads");
        assert_eq!(layer.as_ref(), "road_labels");
        assert_eq!(layer.to_string(), "road_labels");
    }

    #[test]
    fn compression_names_match_content_encodings() {
        assert_eq!(Compression::None.content_encoding(), None);
        assert_eq!(Compression::Gzip.content_encoding(), Some("gzip"));
        assert_eq!(Compression::Zstd.content_encoding(), Some("zstd"));
        assert_eq!(Compression::Brotli.content_encoding(), Some("br"));
        assert_eq!(Compression::Other.content_encoding(), None);
    }
}
