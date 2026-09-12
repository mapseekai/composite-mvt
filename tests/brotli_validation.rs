#![cfg(feature = "brotli")]

use composite_mvt::{ComposeError, Compression, MvtComposer, MvtSource, SourceError};

fn encode(raw: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    brotli::BrotliCompress(&mut &raw[..], &mut output, &Default::default()).unwrap();
    output
}

#[test]
fn rejects_trailing_brotli_data_in_public_decoding_paths() {
    // A version-2 MVT with one layer named "foo".
    let raw = [0x1a, 0x07, 0x0a, 0x03, b'f', b'o', b'o', 0x78, 0x02];
    let encoded = encode(&raw);
    let source = MvtSource::new("brotli-source")
        .with_layers(["foo"])
        .with_compression(Compression::Brotli);
    let composer = MvtComposer::builder()
        .add_source(source.clone())
        .build()
        .unwrap();

    for suffix in [b"junk".as_slice(), encoded.as_slice()] {
        let input = [encoded.as_slice(), suffix].concat();
        assert!(matches!(
            source.decompress(&input),
            Err(SourceError::DecompressionFailed { .. })
        ));
        assert!(matches!(
            MvtSource::from_mvt_with_compression("sample", &input, Compression::Brotli),
            Err(SourceError::DecompressionFailed { .. })
        ));
        assert!(matches!(
            composer.compose(&[input]),
            Err(ComposeError::SourceDecompression { source_id, source: SourceError::DecompressionFailed { .. } })
                if source_id.as_ref() == "brotli-source"
        ));
    }
}

#[test]
fn validates_brotli_completion_across_buffer_boundaries() {
    // Deterministic noise makes both encoded and decoded data exceed the codec buffer.
    let mut seed = 1_u32;
    let raw: Vec<u8> = (0..16384)
        .map(|_| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 24) as u8
        })
        .collect();
    let source = MvtSource::new("source").with_compression(Compression::Brotli);
    let encoded = encode(&raw);
    assert!(encoded.len() > 4096);
    assert_eq!(source.decompress(&encoded).unwrap().as_ref(), raw);
    assert!(source.decompress(&encoded[..encoded.len() - 1]).is_err());
    assert!(
        source
            .decompress(&[encoded.as_slice(), b"junk"].concat())
            .is_err()
    );

    let small = encode(b"tile");
    for length in 0..small.len() {
        assert!(
            source.decompress(&small[..length]).is_err(),
            "accepted prefix {length}"
        );
    }
    assert!(source.decompress(&encode(b"")).unwrap().is_empty());
}
