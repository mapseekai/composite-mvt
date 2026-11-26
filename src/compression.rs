use std::borrow::Cow;

use bytes::Bytes;

use crate::error::{BoxError, SourceErrorKind};
use crate::{Compression, SourceError};

pub(crate) fn detect_compression(input: &[u8]) -> Compression {
    let zstd_skippable = input.get(..4).is_some_and(|prefix| {
        (0x50..=0x5f).contains(&prefix[0]) && prefix[1..] == [0x2a, 0x4d, 0x18]
    });

    if input.starts_with(&[0x1f, 0x8b]) {
        Compression::Gzip
    } else if input.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) || zstd_skippable {
        Compression::Zstd
    } else {
        Compression::None
    }
}

pub(crate) const fn feature_enabled(compression: Compression) -> bool {
    match compression {
        Compression::None => true,
        Compression::Gzip => cfg!(feature = "gzip"),
        Compression::Zstd => cfg!(feature = "zstd"),
        Compression::Brotli => cfg!(feature = "brotli"),
        Compression::Other => false,
    }
}

pub(crate) fn decompress(
    compression: Compression,
    input: &[u8],
) -> Result<Cow<'_, [u8]>, SourceError> {
    decompress_limited(compression, input, usize::MAX).map_err(|error| match error {
        DecodeError::Source(source) => source,
        DecodeError::SizeLimit => decode_failure(
            compression,
            std::io::Error::other("decoded MVT size overflow"),
        ),
    })
}

pub(crate) enum DecodeError {
    Source(SourceError),
    SizeLimit,
}

impl From<SourceError> for DecodeError {
    fn from(source: SourceError) -> Self {
        Self::Source(source)
    }
}

#[inline]
pub(crate) fn decompress_limited(
    compression: Compression,
    input: &[u8],
    max: usize,
) -> Result<Cow<'_, [u8]>, DecodeError> {
    if input.is_empty() {
        return Ok(Cow::Borrowed(input));
    }

    let output = match compression {
        Compression::None => Cow::Borrowed(input),
        Compression::Gzip => Cow::Owned(decompress_gzip(input, max)?),
        Compression::Zstd => Cow::Owned(decompress_zstd(input, max)?),
        Compression::Brotli => return decompress_brotli(input, max).map(Cow::Owned),
        Compression::Other => {
            return Err(DecodeError::Source(
                SourceErrorKind::UnsupportedCompression { compression }.into(),
            ));
        }
    };
    if output.len() > max {
        return Err(DecodeError::SizeLimit);
    }
    Ok(output)
}

fn decode_failure(
    compression: Compression,
    cause: impl std::error::Error + Send + Sync + 'static,
) -> SourceError {
    SourceErrorKind::DecompressionFailed {
        compression,
        cause: Box::new(cause),
    }
    .into()
}

#[cfg(any(feature = "gzip", feature = "zstd"))]
fn read_decoded(
    reader: impl std::io::Read,
    max: usize,
    compression: Compression,
) -> Result<Vec<u8>, SourceError> {
    use std::io::Read as _;

    let mut output = Vec::new();
    // One more byte distinguishes an over-limit stream from a complete exact-size stream.
    reader
        .take((max as u64).saturating_add(1))
        .read_to_end(&mut output)
        .map_err(|error| decode_failure(compression, error))?;
    Ok(output)
}

#[cfg(feature = "gzip")]
fn decompress_gzip(input: &[u8], max: usize) -> Result<Vec<u8>, SourceError> {
    read_decoded(
        flate2::bufread::MultiGzDecoder::new(input),
        max,
        Compression::Gzip,
    )
}

#[cfg(not(feature = "gzip"))]
fn decompress_gzip(_: &[u8], _: usize) -> Result<Vec<u8>, SourceError> {
    Err(SourceErrorKind::CompressionFeatureDisabled {
        compression: Compression::Gzip,
    }
    .into())
}

#[cfg(feature = "zstd")]
fn decompress_zstd(input: &[u8], max: usize) -> Result<Vec<u8>, SourceError> {
    let decoder = zstd::stream::Decoder::with_buffer(input)
        .map_err(|error| decode_failure(Compression::Zstd, error))?;
    read_decoded(decoder, max, Compression::Zstd)
}

#[cfg(not(feature = "zstd"))]
fn decompress_zstd(_: &[u8], _: usize) -> Result<Vec<u8>, SourceError> {
    Err(SourceErrorKind::CompressionFeatureDisabled {
        compression: Compression::Zstd,
    }
    .into())
}

#[cfg(feature = "brotli")]
struct LimitedOutput {
    bytes: Vec<u8>,
    max: usize,
}

#[cfg(feature = "brotli")]
impl std::io::Write for LimitedOutput {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.max - self.bytes.len() {
            return Err(std::io::ErrorKind::FileTooLarge.into());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(feature = "brotli")]
fn decompress_brotli(input: &[u8], max: usize) -> Result<Vec<u8>, DecodeError> {
    use std::io::{ErrorKind, Write as _};

    let mut output = LimitedOutput {
        bytes: Vec::new(),
        max,
    };
    let result = {
        let mut decoder = brotli::DecompressorWriter::new(&mut output, 4096);
        // write_all rejects unconsumed trailing bytes; close rejects incomplete streams.
        decoder.write_all(input).and_then(|()| decoder.close())
    };
    match result {
        Ok(()) => Ok(output.bytes),
        Err(error) if error.kind() == ErrorKind::FileTooLarge => Err(DecodeError::SizeLimit),
        Err(error) => Err(decode_failure(Compression::Brotli, error).into()),
    }
}

#[cfg(not(feature = "brotli"))]
fn decompress_brotli(_: &[u8], _: usize) -> Result<Vec<u8>, DecodeError> {
    Err(DecodeError::Source(
        SourceErrorKind::CompressionFeatureDisabled {
            compression: Compression::Brotli,
        }
        .into(),
    ))
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum OutputCompression {
    Raw,
    #[cfg(feature = "gzip")]
    Gzip,
    #[cfg(feature = "zstd")]
    Zstd,
    #[cfg(feature = "brotli")]
    Brotli,
}

impl OutputCompression {
    pub(crate) fn from_compression(compression: Compression) -> Option<Self> {
        match compression {
            Compression::None => Some(Self::Raw),
            #[cfg(feature = "gzip")]
            Compression::Gzip => Some(Self::Gzip),
            #[cfg(feature = "zstd")]
            Compression::Zstd => Some(Self::Zstd),
            #[cfg(feature = "brotli")]
            Compression::Brotli => Some(Self::Brotli),
            _ => None,
        }
    }

    pub(crate) const fn compression(self) -> Compression {
        match self {
            Self::Raw => Compression::None,
            #[cfg(feature = "gzip")]
            Self::Gzip => Compression::Gzip,
            #[cfg(feature = "zstd")]
            Self::Zstd => Compression::Zstd,
            #[cfg(feature = "brotli")]
            Self::Brotli => Compression::Brotli,
        }
    }

    pub(crate) fn encode(self, input: Vec<u8>) -> Result<Bytes, BoxError> {
        match self {
            Self::Raw => Ok(Bytes::from(input)),
            #[cfg(feature = "gzip")]
            Self::Gzip => compress_gzip(&input).map(Bytes::from),
            #[cfg(feature = "zstd")]
            Self::Zstd => compress_zstd(&input).map(Bytes::from),
            #[cfg(feature = "brotli")]
            Self::Brotli => compress_brotli(&input).map(Bytes::from),
        }
    }
}

#[cfg(feature = "gzip")]
fn compress_gzip(input: &[u8]) -> Result<Vec<u8>, BoxError> {
    use std::io::Write as _;

    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(input)?;
    Ok(encoder.finish()?)
}

#[cfg(feature = "zstd")]
fn compress_zstd(input: &[u8]) -> Result<Vec<u8>, BoxError> {
    Ok(zstd::stream::encode_all(input, 0)?)
}

#[cfg(feature = "brotli")]
fn compress_brotli(input: &[u8]) -> Result<Vec<u8>, BoxError> {
    let mut output = Vec::new();
    let params = brotli::enc::BrotliEncoderParams::default();
    brotli::BrotliCompress(&mut std::io::Cursor::new(input), &mut output, &params)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;

    #[cfg(any(feature = "gzip", feature = "zstd"))]
    #[test]
    fn bounded_reader_leaves_bytes_after_the_probe_unread() {
        let mut reader = b"tiles".as_slice();
        read_decoded(&mut reader, 2, Compression::None).unwrap();
        assert_eq!(reader, b"es");
    }

    #[cfg(feature = "brotli")]
    #[test]
    fn brotli_writer_stops_at_the_remaining_budget() {
        use std::io::{ErrorKind, Write as _};

        let mut output = LimitedOutput {
            bytes: Vec::new(),
            max: 4,
        };
        output.write_all(b"ab").unwrap();
        let error = output.write_all(b"cde").unwrap_err();
        assert_eq!(error.kind(), ErrorKind::FileTooLarge);
        assert!(output.bytes.len() <= output.max);
    }

    #[test]
    fn detects_known_frame_signatures() {
        assert_eq!(detect_compression(&[0x1f, 0x8b, 0x08]), Compression::Gzip);
        assert_eq!(
            detect_compression(&[0x28, 0xb5, 0x2f, 0xfd]),
            Compression::Zstd
        );
        assert_eq!(
            detect_compression(&[0x50, 0x2a, 0x4d, 0x18]),
            Compression::Zstd
        );
        assert_eq!(
            detect_compression(&[0x5f, 0x2a, 0x4d, 0x18]),
            Compression::Zstd
        );
        assert_eq!(
            detect_compression(&[0x60, 0x2a, 0x4d, 0x18]),
            Compression::None
        );
        assert_eq!(detect_compression(&[0x1a, 0x00]), Compression::None);
    }

    #[test]
    fn none_decompression_borrows_input() {
        let input = b"raw";
        let output = decompress(Compression::None, input).unwrap();
        assert!(matches!(output, Cow::Borrowed(_)));
        assert_eq!(output.as_ref(), input);
    }

    #[test]
    fn reports_feature_availability() {
        assert!(feature_enabled(Compression::None));
        assert!(!feature_enabled(Compression::Other));
        assert_eq!(feature_enabled(Compression::Gzip), cfg!(feature = "gzip"));
        assert_eq!(feature_enabled(Compression::Zstd), cfg!(feature = "zstd"));
        assert_eq!(
            feature_enabled(Compression::Brotli),
            cfg!(feature = "brotli")
        );
    }

    #[test]
    fn raw_output_takes_ownership_of_the_input() {
        let input = b"raw".to_vec();
        let allocation = input.as_ptr();
        let output = OutputCompression::Raw.encode(input).unwrap();
        assert_eq!(output.as_ref(), b"raw");
        assert_eq!(output.as_ptr(), allocation);
    }

    #[test]
    fn other_decompression_returns_typed_unsupported_error() {
        let error = decompress(Compression::Other, b"raw").unwrap_err();
        assert!(error.is_unsupported_compression());
        assert_eq!(error.compression(), Some(Compression::Other));
    }

    #[test]
    fn only_enabled_encodings_can_be_selected_for_output() {
        for compression in [
            Compression::None,
            Compression::Gzip,
            Compression::Zstd,
            Compression::Brotli,
            Compression::Other,
        ] {
            assert_eq!(
                OutputCompression::from_compression(compression)
                    .map(OutputCompression::compression),
                feature_enabled(compression).then_some(compression)
            );
        }
    }

    #[cfg(not(feature = "gzip"))]
    #[test]
    fn disabled_gzip_decompression_returns_typed_error() {
        let error = decompress(Compression::Gzip, b"raw").unwrap_err();
        assert!(error.is_compression_feature_disabled());
        assert_eq!(error.compression(), Some(Compression::Gzip));
    }

    #[cfg(not(feature = "zstd"))]
    #[test]
    fn disabled_zstd_decompression_returns_typed_error() {
        let error = decompress(Compression::Zstd, b"raw").unwrap_err();
        assert!(error.is_compression_feature_disabled());
        assert_eq!(error.compression(), Some(Compression::Zstd));
    }

    #[cfg(not(feature = "brotli"))]
    #[test]
    fn disabled_brotli_decompression_returns_typed_error() {
        let error = decompress(Compression::Brotli, b"raw").unwrap_err();
        assert!(error.is_compression_feature_disabled());
        assert_eq!(error.compression(), Some(Compression::Brotli));
    }

    #[cfg(feature = "gzip")]
    #[test]
    fn gzip_round_trip() {
        let encoded = OutputCompression::Gzip.encode(b"tile".to_vec()).unwrap();
        assert!(encoded.starts_with(&[0x1f, 0x8b]));
        assert_eq!(
            decompress(Compression::Gzip, &encoded).unwrap().as_ref(),
            b"tile"
        );
    }

    #[cfg(feature = "zstd")]
    #[test]
    fn zstd_round_trip() {
        let encoded = OutputCompression::Zstd.encode(b"tile".to_vec()).unwrap();
        assert!(encoded.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]));
        assert_eq!(
            decompress(Compression::Zstd, &encoded).unwrap().as_ref(),
            b"tile"
        );
    }

    #[cfg(feature = "brotli")]
    #[test]
    fn brotli_round_trip() {
        let encoded = OutputCompression::Brotli.encode(b"tile".to_vec()).unwrap();
        assert_eq!(
            decompress(Compression::Brotli, &encoded).unwrap().as_ref(),
            b"tile"
        );
    }

    #[cfg(feature = "gzip")]
    #[test]
    fn invalid_gzip_input_returns_decompression_error() {
        let error = decompress(Compression::Gzip, b"raw").unwrap_err();
        assert!(error.is_decompression_failed());
        assert_eq!(error.compression(), Some(Compression::Gzip));
    }

    #[cfg(feature = "zstd")]
    #[test]
    fn invalid_zstd_input_returns_decompression_error() {
        let error = decompress(Compression::Zstd, b"raw").unwrap_err();
        assert!(error.is_decompression_failed());
        assert_eq!(error.compression(), Some(Compression::Zstd));
    }

    #[cfg(feature = "brotli")]
    #[test]
    fn invalid_brotli_input_returns_decompression_error() {
        let error = decompress(Compression::Brotli, b"raw").unwrap_err();
        assert!(error.is_decompression_failed());
        assert_eq!(error.compression(), Some(Compression::Brotli));
    }
}
