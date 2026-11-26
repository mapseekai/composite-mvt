use bytes::Bytes;

use crate::compression::{self, DecodeError, OutputCompression};
use crate::error::ComposeErrorKind;
use crate::layer_filter::LayerFilter;
use crate::{ComposeError, Compression, MvtComposerBuilder, MvtSource};

/// An immutable composition plan for fixed MVT sources.
///
/// A composer contains only owned configuration and can be shared as `Arc<MvtComposer>` without
/// library-managed locks. Requests borrow raw inputs and own any decompressed and output buffers.
#[derive(Debug)]
pub struct MvtComposer {
    pub(crate) sources: Box<[MvtSource]>,
    pub(crate) output_compression: OutputCompression,
    pub(crate) max_decoded_bytes: usize,
    pub(crate) layer_filter: Option<LayerFilter>,
}

impl MvtComposer {
    /// Starts configuring a composer.
    #[must_use]
    pub fn builder() -> MvtComposerBuilder {
        MvtComposerBuilder::default()
    }

    /// Returns the fixed sources in the input order used by both composition methods.
    #[must_use]
    pub fn sources(&self) -> &[MvtSource] {
        &self.sources
    }

    /// Returns the fixed output encoding selected during building.
    #[must_use]
    pub const fn output_compression(&self) -> Compression {
        self.output_compression.compression()
    }

    /// Composes one input per configured source, preserving source order.
    ///
    /// Supply input tiles with the same projection, geographic bounds, and coordinate orientation.
    /// Check the tile scheme as well as the XYZ values when combining tiles from different sources.
    ///
    /// `inputs[n]` is decoded according to `self.sources()[n].compression()`. A zero-byte input
    /// contributes an empty tile regardless of the encoding. When every input is zero bytes,
    /// this method returns empty bytes without allocating or using codecs. Otherwise the
    /// composite is returned raw or encoded once using [`Self::output_compression`].
    ///
    /// With [`crate::DuplicateLayer::KeepFirst`] or [`crate::DuplicateLayer::KeepLast`], only
    /// sources with overlapping declarations are scanned. Every valid tile must use unique
    /// layer names from its source's declared layers. Surviving layers retain their input order.
    ///
    /// A gzip output is one gzip member containing the complete composite MVT, suitable for an
    /// HTTP response with `Content-Encoding: gzip`. This crate returns bytes only; callers set HTTP
    /// headers themselves via [`Compression::content_encoding`]. Omit `Content-Encoding` for
    /// the empty result from all zero-byte inputs.
    ///
    /// # Errors
    ///
    /// Returns [`ComposeError`] for an input count mismatch, a failure to decode or scan an input,
    /// an exceeded decoded size limit, or an output encoding failure. Use
    /// [`ComposeError::source_id`] to identify a source that failed decoding or scanning.
    pub fn compose<B>(&self, inputs: &[B]) -> Result<Bytes, ComposeError>
    where
        B: AsRef<[u8]>,
    {
        self.compose_inputs(inputs, |source, input| {
            (input.as_ref(), source.compression())
        })
    }

    /// Composes one input per configured source using the encoding supplied for each input.
    ///
    /// Each `(bytes, encoding)` pair corresponds to the source at the same index. The per-input
    /// encodings apply only to this call. Source metadata and [`Self::output_compression`] come
    /// from the built composer. Input tiles follow the spatial alignment and empty-tile rules of
    /// [`Self::compose`].
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::compose`]. A nonempty input with a disabled or
    /// unsupported codec reports a [source decompression error](ComposeError::is_source_decompression)
    /// with the configured source ID.
    pub fn compose_with_encodings<B: AsRef<[u8]>>(
        &self,
        inputs: &[(B, Compression)],
    ) -> Result<Bytes, ComposeError> {
        self.compose_inputs(inputs, |_, (input, encoding)| (input.as_ref(), *encoding))
    }

    fn compose_inputs<'a, B>(
        &self,
        inputs: &'a [B],
        resolve: impl Fn(&MvtSource, &'a B) -> (&'a [u8], Compression),
    ) -> Result<Bytes, ComposeError> {
        if inputs.len() != self.sources.len() {
            return Err(ComposeErrorKind::InputCountMismatch {
                expected: self.sources.len(),
                actual: inputs.len(),
            }
            .into());
        }

        let mut remaining = self.max_decoded_bytes;
        let mut inline_inputs = [const { None }; 4];
        let mut inline_slots = inline_inputs.iter_mut();
        let mut extra_inputs = Vec::new();
        for (index, (source, input)) in self.sources.iter().zip(inputs).enumerate() {
            let (bytes, encoding) = resolve(source, input);
            if bytes.is_empty() {
                continue;
            }
            let raw =
                compression::decompress_limited(encoding, bytes, remaining).map_err(|error| {
                    match error {
                        DecodeError::Source(source_error) => {
                            ComposeErrorKind::SourceDecompression {
                                source_id: source.id().clone(),
                                cause: Box::new(source_error),
                            }
                        }
                        DecodeError::SizeLimit => ComposeErrorKind::SizeOverflow,
                    }
                })?;
            remaining -= raw.len();
            if let Some(slot) = inline_slots.next() {
                *slot = Some((index, raw));
            } else {
                if extra_inputs.is_empty() {
                    extra_inputs.reserve_exact(inputs.len() - index);
                }
                extra_inputs.push((index, raw));
            }
        }

        if inline_inputs[0].is_none() {
            return Ok(Bytes::new());
        }
        let raw_inputs = inline_inputs
            .iter()
            .flatten()
            .chain(&extra_inputs)
            .map(|(index, raw)| (*index, raw.as_ref()));
        let output = if let Some(filter) = &self.layer_filter {
            filter.compose(&self.sources, raw_inputs)?
        } else {
            let mut output = Vec::with_capacity(self.max_decoded_bytes - remaining);
            for (_, input) in raw_inputs {
                output.extend_from_slice(input);
            }
            output
        };

        self.output_compression.encode(output).map_err(|cause| {
            ComposeErrorKind::OutputCompression {
                compression: self.output_compression(),
                cause,
            }
            .into()
        })
    }
}
