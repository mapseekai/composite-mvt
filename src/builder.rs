use std::collections::{HashMap, HashSet};

use crate::compression::{self, OutputCompression};
use crate::error::BuildErrorKind;
use crate::layer_filter::{LayerFilter, Precedence};
use crate::{BuildError, Compression, DuplicateLayer, LayerName, MvtComposer, MvtSource};

/// Configures an immutable [`MvtComposer`].
///
/// Source configuration is fixed when [`Self::build`] succeeds. Request bytes are supplied later
/// in the same order as sources are added here.
#[derive(Debug)]
pub struct MvtComposerBuilder {
    sources: Vec<MvtSource>,
    duplicate_layer: DuplicateLayer,
    output_compression: Compression,
    max_decoded_bytes: usize,
}

impl Default for MvtComposerBuilder {
    fn default() -> Self {
        Self {
            sources: Vec::new(),
            duplicate_layer: DuplicateLayer::Error,
            output_compression: Compression::None,
            max_decoded_bytes: usize::MAX,
        }
    }
}

impl MvtComposerBuilder {
    /// Sets how equal layer names in separate sources are handled.
    ///
    /// [`DuplicateLayer::Error`] is the default. Each source must declare each layer name once.
    #[must_use]
    pub fn duplicate_layer(mut self, behavior: DuplicateLayer) -> Self {
        self.duplicate_layer = behavior;
        self
    }

    /// Sets the encoding applied once to the complete composite output.
    ///
    /// This setting is fixed in the built composer; it is not selected per request. The required
    /// Cargo feature must be enabled for a non-raw encoding.
    #[must_use]
    pub fn output_compression(mut self, compression: Compression) -> Self {
        self.output_compression = compression;
        self
    }

    /// Sets the inclusive limit on total decoded bytes across all inputs in each composition.
    ///
    /// Raw inputs and discarded layers count toward the same limit. The default is `usize::MAX`.
    /// A limit of zero allows only inputs that contribute zero decoded bytes. Exceeding the limit
    /// returns a [`crate::ComposeError`] that can be identified with
    /// [`crate::ComposeError::is_size_overflow`].
    #[must_use]
    pub fn max_decoded_bytes(mut self, max: usize) -> Self {
        self.max_decoded_bytes = max;
        self
    }

    /// Adds a source to the fixed input order.
    ///
    /// [`MvtComposer::compose`] and [`MvtComposer::compose_with_encodings`] each require exactly one
    /// input per source, in this order.
    #[must_use]
    pub fn add_source(mut self, source: MvtSource) -> Self {
        self.sources.push(source);
        self
    }

    /// Validates duplicate layer names without consuming or changing this builder.
    ///
    /// Repeating a name within one source always produces an error. Sharing a name across sources
    /// produces an error under [`DuplicateLayer::Error`]. Use [`BuildError::duplicate_layer_name`]
    /// to inspect the conflicting declarations. [`Self::build`] reuses this validation.
    pub fn validate_duplicate_layers(&self) -> Result<(), BuildError> {
        self.validate_layers(None).map(drop)
    }

    fn validate_layers(
        &self,
        precedence: Option<Precedence>,
    ) -> Result<Option<LayerFilter>, BuildError> {
        let mut first_sources: HashMap<&LayerName, usize> = HashMap::new();
        let mut scan_sources = Vec::new();
        let reject_overlap = self.duplicate_layer == DuplicateLayer::Error;

        for (index, source) in self.sources.iter().enumerate() {
            let mut local = HashSet::new();
            for layer in source.layers() {
                if !reject_overlap && !local.insert(layer) {
                    return Err(BuildErrorKind::DuplicateLayerName {
                        layer: layer.clone(),
                        first_source: source.id().clone(),
                        second_source: source.id().clone(),
                    }
                    .into());
                }

                if !reject_overlap && precedence.is_none() {
                    continue;
                }
                if let Some(&first_index) = first_sources.get(layer) {
                    if reject_overlap {
                        return Err(BuildErrorKind::DuplicateLayerName {
                            layer: layer.clone(),
                            first_source: self.sources[first_index].id().clone(),
                            second_source: source.id().clone(),
                        }
                        .into());
                    }
                    scan_sources.resize(self.sources.len(), false);
                    scan_sources[first_index] = true;
                    scan_sources[index] = true;
                } else {
                    first_sources.insert(layer, index);
                }
            }
        }

        Ok(precedence
            .filter(|_| !scan_sources.is_empty())
            .map(|precedence| LayerFilter {
                precedence,
                scan_sources: scan_sources.into_boxed_slice(),
            }))
    }

    /// Validates all configuration and constructs an immutable, lock-free composer.
    ///
    /// In addition to duplicate-layer validation, this checks source IDs, required source layers,
    /// and feature support for source and output encodings.
    pub fn build(self) -> Result<MvtComposer, BuildError> {
        let layer_filter = self.validate()?;
        let output_compression = validate_output_compression(self.output_compression)?;

        Ok(MvtComposer {
            sources: self.sources.into_boxed_slice(),
            output_compression,
            max_decoded_bytes: self.max_decoded_bytes,
            layer_filter,
        })
    }

    fn validate(&self) -> Result<Option<LayerFilter>, BuildError> {
        if self.sources.is_empty() {
            return Err(BuildErrorKind::NoSources.into());
        }

        let mut ids = HashSet::new();
        for source in &self.sources {
            if !ids.insert(source.id()) {
                return Err(BuildErrorKind::DuplicateSourceId {
                    id: source.id().clone(),
                }
                .into());
            }
        }

        for source in &self.sources {
            if source.layers().is_empty() {
                return Err(BuildErrorKind::NoLayers {
                    source_id: source.id().clone(),
                }
                .into());
            }
        }

        for source in &self.sources {
            if source
                .layers()
                .iter()
                .any(|layer| layer.as_ref().is_empty())
            {
                return Err(BuildErrorKind::EmptyLayerName {
                    source_id: source.id().clone(),
                }
                .into());
            }
        }

        let precedence = match self.duplicate_layer {
            DuplicateLayer::KeepFirst => Some(Precedence::First),
            DuplicateLayer::KeepLast => Some(Precedence::Last),
            DuplicateLayer::Error | DuplicateLayer::Allow => None,
        };
        let layer_filter = self.validate_layers(precedence)?;

        for source in &self.sources {
            validate_source_compression(source)?;
        }

        Ok(layer_filter)
    }
}

fn validate_source_compression(source: &MvtSource) -> Result<(), BuildError> {
    let compression = source.compression();
    if compression == Compression::Other {
        return Err(BuildErrorKind::UnsupportedCompression {
            source_id: source.id().clone(),
            compression,
        }
        .into());
    }
    if !compression::feature_enabled(compression) {
        return Err(BuildErrorKind::CompressionFeatureDisabled {
            source_id: source.id().clone(),
            compression,
        }
        .into());
    }

    Ok(())
}

fn validate_output_compression(compression: Compression) -> Result<OutputCompression, BuildError> {
    OutputCompression::from_compression(compression).ok_or_else(|| {
        let kind = if compression == Compression::Other {
            BuildErrorKind::UnsupportedOutputCompression { compression }
        } else {
            BuildErrorKind::OutputCompressionFeatureDisabled { compression }
        };
        kind.into()
    })
}
