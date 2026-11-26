use std::backtrace::{Backtrace, BacktraceStatus};
use std::error::Error;
use std::fmt;

use crate::{Compression, LayerName, SourceId};

pub(crate) type BoxError = Box<dyn Error + Send + Sync + 'static>;

/// An error reading [`crate::MvtSource`] metadata or processing source inputs.
#[derive(Debug)]
#[must_use]
pub struct SourceError {
    kind: SourceErrorKind,
    backtrace: Backtrace,
}

impl SourceError {
    /// Returns whether the source sample was empty.
    #[must_use]
    pub const fn is_empty_bytes(&self) -> bool {
        matches!(self.kind, SourceErrorKind::EmptyBytes)
    }

    /// Returns whether a multi-sample constructor received no samples.
    #[must_use]
    pub const fn is_no_samples(&self) -> bool {
        matches!(self.kind, SourceErrorKind::NoSamples)
    }

    /// Returns whether Cargo's feature for the input compression was disabled.
    #[must_use]
    pub const fn is_compression_feature_disabled(&self) -> bool {
        #[cfg(not(all(feature = "gzip", feature = "zstd", feature = "brotli")))]
        {
            matches!(
                self.kind,
                SourceErrorKind::CompressionFeatureDisabled { .. }
            )
        }
        #[cfg(all(feature = "gzip", feature = "zstd", feature = "brotli"))]
        {
            false
        }
    }

    /// Returns whether the requested input compression is unsupported.
    #[must_use]
    pub const fn is_unsupported_compression(&self) -> bool {
        matches!(self.kind, SourceErrorKind::UnsupportedCompression { .. })
    }

    /// Returns whether the input decoder rejected the bytes.
    #[must_use]
    pub const fn is_decompression_failed(&self) -> bool {
        matches!(self.kind, SourceErrorKind::DecompressionFailed { .. })
    }

    /// Returns the input encoding for feature, support, and decompression errors.
    #[must_use]
    pub const fn compression(&self) -> Option<Compression> {
        match self.kind {
            #[cfg(not(all(feature = "gzip", feature = "zstd", feature = "brotli")))]
            SourceErrorKind::CompressionFeatureDisabled { compression } => Some(compression),
            SourceErrorKind::UnsupportedCompression { compression }
            | SourceErrorKind::DecompressionFailed { compression, .. } => Some(compression),
            _ => None,
        }
    }

    /// Returns whether the decoded bytes failed MVT parsing.
    #[must_use]
    pub const fn is_invalid_mvt(&self) -> bool {
        matches!(self.kind, SourceErrorKind::InvalidMvt { .. })
    }

    /// Returns whether a layer name was missing or empty.
    #[must_use]
    pub const fn is_missing_layer_name(&self) -> bool {
        matches!(self.kind, SourceErrorKind::MissingLayerName)
    }

    /// Returns whether the sample contained no layers.
    #[must_use]
    pub const fn is_no_layers(&self) -> bool {
        matches!(self.kind, SourceErrorKind::NoLayers)
    }

    /// Returns the expected and actual encodings when sample detection disagreed.
    #[must_use]
    pub const fn inconsistent_sample_compression(&self) -> Option<(Compression, Compression)> {
        match self.kind {
            SourceErrorKind::InconsistentSampleCompression { expected, actual } => {
                Some((expected, actual))
            }
            _ => None,
        }
    }

    /// Returns the backtrace captured when the error was created.
    pub const fn backtrace(&self) -> &Backtrace {
        &self.backtrace
    }

    pub(crate) fn invalid_mvt(cause: impl Error + Send + Sync + 'static) -> Self {
        SourceErrorKind::InvalidMvt {
            cause: Box::new(cause),
        }
        .into()
    }
}

impl From<SourceErrorKind> for SourceError {
    fn from(kind: SourceErrorKind) -> Self {
        Self {
            kind,
            backtrace: Backtrace::capture(),
        }
    }
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_error(&self.kind, &self.backtrace, self.source(), f)
    }
}

impl Error for SourceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.kind {
            SourceErrorKind::DecompressionFailed { cause, .. }
            | SourceErrorKind::InvalidMvt { cause } => Some(cause.as_ref()),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum SourceErrorKind {
    #[error("MVT input is empty")]
    EmptyBytes,
    #[error("no MVT samples were supplied")]
    NoSamples,
    #[cfg(not(all(feature = "gzip", feature = "zstd", feature = "brotli")))]
    #[error("the {compression} Cargo feature is disabled")]
    CompressionFeatureDisabled { compression: Compression },
    #[error("unsupported compression format: {compression}")]
    UnsupportedCompression { compression: Compression },
    #[error("failed to decompress {compression} input")]
    DecompressionFailed {
        compression: Compression,
        cause: BoxError,
    },
    #[error("input is not a valid MVT")]
    InvalidMvt { cause: BoxError },
    #[error("MVT layer name is missing or empty")]
    MissingLayerName,
    #[error("MVT contains no layers")]
    NoLayers,
    #[error("sample compression mismatch: expected {expected}, got {actual}")]
    InconsistentSampleCompression {
        expected: Compression,
        actual: Compression,
    },
}

/// An error validating a [`crate::MvtComposerBuilder`] configuration.
#[derive(Debug)]
#[must_use]
pub struct BuildError {
    kind: BuildErrorKind,
    backtrace: Backtrace,
}

impl BuildError {
    /// Returns whether the builder had no sources.
    #[must_use]
    pub const fn is_no_sources(&self) -> bool {
        matches!(self.kind, BuildErrorKind::NoSources)
    }

    /// Returns whether two configured sources used the same ID.
    #[must_use]
    pub const fn is_duplicate_source_id(&self) -> bool {
        matches!(self.kind, BuildErrorKind::DuplicateSourceId { .. })
    }

    /// Returns whether a source declared no layers.
    #[must_use]
    pub const fn is_no_layers(&self) -> bool {
        matches!(self.kind, BuildErrorKind::NoLayers { .. })
    }

    /// Returns whether a source declared an empty layer name.
    #[must_use]
    pub const fn is_empty_layer_name(&self) -> bool {
        matches!(self.kind, BuildErrorKind::EmptyLayerName { .. })
    }

    /// Returns the repeated layer name and its first and second source IDs.
    #[must_use]
    pub const fn duplicate_layer_name(&self) -> Option<(&LayerName, &SourceId, &SourceId)> {
        match &self.kind {
            BuildErrorKind::DuplicateLayerName {
                layer,
                first_source,
                second_source,
            } => Some((layer, first_source, second_source)),
            _ => None,
        }
    }

    /// Returns whether a configured source requires a disabled compression feature.
    #[must_use]
    pub const fn is_compression_feature_disabled(&self) -> bool {
        matches!(self.kind, BuildErrorKind::CompressionFeatureDisabled { .. })
    }

    /// Returns whether a configured source uses an unsupported encoding.
    #[must_use]
    pub const fn is_unsupported_compression(&self) -> bool {
        matches!(self.kind, BuildErrorKind::UnsupportedCompression { .. })
    }

    /// Returns whether the output encoding requires a disabled compression feature.
    #[must_use]
    pub const fn is_output_compression_feature_disabled(&self) -> bool {
        matches!(
            self.kind,
            BuildErrorKind::OutputCompressionFeatureDisabled { .. }
        )
    }

    /// Returns whether the output encoding is unsupported.
    #[must_use]
    pub const fn is_unsupported_output_compression(&self) -> bool {
        matches!(
            self.kind,
            BuildErrorKind::UnsupportedOutputCompression { .. }
        )
    }

    /// Returns the source ID for errors that identify a single source.
    #[must_use]
    pub const fn source_id(&self) -> Option<&SourceId> {
        match &self.kind {
            BuildErrorKind::DuplicateSourceId { id }
            | BuildErrorKind::NoLayers { source_id: id }
            | BuildErrorKind::EmptyLayerName { source_id: id }
            | BuildErrorKind::CompressionFeatureDisabled { source_id: id, .. }
            | BuildErrorKind::UnsupportedCompression { source_id: id, .. } => Some(id),
            _ => None,
        }
    }

    /// Returns the input or output encoding for a compression configuration error.
    #[must_use]
    pub const fn compression(&self) -> Option<Compression> {
        match self.kind {
            BuildErrorKind::CompressionFeatureDisabled { compression, .. }
            | BuildErrorKind::UnsupportedCompression { compression, .. }
            | BuildErrorKind::OutputCompressionFeatureDisabled { compression }
            | BuildErrorKind::UnsupportedOutputCompression { compression } => Some(compression),
            _ => None,
        }
    }

    /// Returns the backtrace captured when the error was created.
    pub const fn backtrace(&self) -> &Backtrace {
        &self.backtrace
    }
}

impl From<BuildErrorKind> for BuildError {
    fn from(kind: BuildErrorKind) -> Self {
        Self {
            kind,
            backtrace: Backtrace::capture(),
        }
    }
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_error(&self.kind, &self.backtrace, None, f)
    }
}

impl Error for BuildError {}

#[derive(Debug, thiserror::Error)]
pub(crate) enum BuildErrorKind {
    #[error("composer requires at least one source")]
    NoSources,
    #[error("duplicate source id: {id}")]
    DuplicateSourceId { id: SourceId },
    #[error("source `{source_id}` has no layers")]
    NoLayers { source_id: SourceId },
    #[error("source `{source_id}` contains an empty layer name")]
    EmptyLayerName { source_id: SourceId },
    #[error("layer `{layer}` is duplicated between `{first_source}` and `{second_source}`")]
    DuplicateLayerName {
        layer: LayerName,
        first_source: SourceId,
        second_source: SourceId,
    },
    #[error("source `{source_id}` requires disabled {compression} support")]
    CompressionFeatureDisabled {
        source_id: SourceId,
        compression: Compression,
    },
    #[error("source `{source_id}` uses unsupported compression {compression}")]
    UnsupportedCompression {
        source_id: SourceId,
        compression: Compression,
    },
    #[error("output requires disabled {compression} support")]
    OutputCompressionFeatureDisabled { compression: Compression },
    #[error("unsupported output compression: {compression}")]
    UnsupportedOutputCompression { compression: Compression },
}

/// An error composing configured source inputs.
#[derive(Debug)]
#[must_use]
pub struct ComposeError {
    kind: ComposeErrorKind,
    backtrace: Backtrace,
}

impl ComposeError {
    /// Returns the expected and actual counts when the input count differed.
    #[must_use]
    pub const fn input_count_mismatch(&self) -> Option<(usize, usize)> {
        match self.kind {
            ComposeErrorKind::InputCountMismatch { expected, actual } => Some((expected, actual)),
            _ => None,
        }
    }

    /// Returns whether one of the source inputs could not be decoded.
    #[must_use]
    pub const fn is_source_decompression(&self) -> bool {
        matches!(self.kind, ComposeErrorKind::SourceDecompression { .. })
    }

    /// Returns whether a source input failed the layer scan.
    #[must_use]
    pub const fn is_source_parsing(&self) -> bool {
        matches!(self.kind, ComposeErrorKind::SourceParsing { .. })
    }

    /// Returns the ID of the source that failed decoding or the layer scan.
    #[must_use]
    pub const fn source_id(&self) -> Option<&SourceId> {
        match &self.kind {
            ComposeErrorKind::SourceDecompression { source_id, .. }
            | ComposeErrorKind::SourceParsing { source_id, .. } => Some(source_id),
            _ => None,
        }
    }

    /// Returns the error from decoding or scanning an individual source.
    #[must_use]
    pub fn source_error(&self) -> Option<&SourceError> {
        match &self.kind {
            ComposeErrorKind::SourceDecompression { cause, .. }
            | ComposeErrorKind::SourceParsing { cause, .. } => Some(cause),
            _ => None,
        }
    }

    /// Returns the output encoding when encoding the composite failed.
    #[must_use]
    pub const fn output_compression(&self) -> Option<Compression> {
        match self.kind {
            ComposeErrorKind::OutputCompression { compression, .. } => Some(compression),
            _ => None,
        }
    }

    /// Returns whether the total decoded length exceeded the configured limit.
    #[must_use]
    pub const fn is_size_overflow(&self) -> bool {
        matches!(self.kind, ComposeErrorKind::SizeOverflow)
    }

    /// Returns the backtrace captured when the error was created.
    pub const fn backtrace(&self) -> &Backtrace {
        &self.backtrace
    }
}

impl From<ComposeErrorKind> for ComposeError {
    fn from(kind: ComposeErrorKind) -> Self {
        Self {
            kind,
            backtrace: Backtrace::capture(),
        }
    }
}

impl fmt::Display for ComposeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_error(&self.kind, &self.backtrace, self.source(), f)
    }
}

impl Error for ComposeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.kind {
            ComposeErrorKind::OutputCompression { cause, .. } => Some(cause.as_ref()),
            _ => self
                .source_error()
                .map(|cause| cause as &(dyn Error + 'static)),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ComposeErrorKind {
    #[error("input count mismatch: expected {expected}, got {actual}")]
    InputCountMismatch { expected: usize, actual: usize },
    #[error("failed to decompress source `{source_id}`")]
    SourceDecompression {
        source_id: SourceId,
        cause: Box<SourceError>,
    },
    #[error("failed to read layers from source `{source_id}`")]
    SourceParsing {
        source_id: SourceId,
        cause: Box<SourceError>,
    },
    #[error("failed to compress composite output as {compression}")]
    OutputCompression {
        compression: Compression,
        cause: BoxError,
    },
    #[error("decoded composite MVT exceeds the size limit")]
    SizeOverflow,
}

fn fmt_error(
    summary: &impl fmt::Display,
    backtrace: &Backtrace,
    cause: Option<&dyn Error>,
    f: &mut fmt::Formatter<'_>,
) -> fmt::Result {
    summary.fmt(f)?;
    let separator = if backtrace.status() == BacktraceStatus::Captured {
        write!(f, "\n{backtrace}")?;
        "\nCaused by: "
    } else {
        ": "
    };
    if let Some(cause) = cause {
        write!(f, "{separator}{cause}")?;
    }
    Ok(())
}
