#[cfg(any(feature = "gzip", feature = "zstd", feature = "brotli"))]
mod common;
#[path = "composition/concurrency.rs"]
mod concurrency;
#[path = "composition/duplicate_layers.rs"]
mod duplicate_layers;

#[cfg(any(feature = "gzip", feature = "zstd", feature = "brotli"))]
#[path = "composition/fixtures.rs"]
mod fixtures;

use composite_mvt::{ComposeError, Compression, MvtComposer, MvtSource, SourceError};

fn composer(ids: &[&str]) -> MvtComposer {
    ids.iter()
        .fold(MvtComposer::builder(), |builder, id| {
            builder.add_source(MvtSource::new(*id).with_layers([*id]))
        })
        .build()
        .unwrap()
}

#[test]
fn composes_raw_inputs_in_source_order() {
    let composer = composer(&["a", "b", "c"]);

    let output = composer
        .compose(&[&b"first"[..], &b"second"[..], &b"third"[..]])
        .unwrap();

    assert_eq!(output.as_ref(), b"firstsecondthird");
}

#[test]
fn rejects_wrong_input_count_before_composition() {
    let composer = composer(&["a", "b"]);

    let missing = composer.compose(&[b"only"]);
    let extra = composer.compose(&[&b"one"[..], &b"two"[..], &b"three"[..]]);

    assert!(matches!(
        missing,
        Err(ComposeError::InputCountMismatch {
            expected: 2,
            actual: 1
        })
    ));
    assert!(matches!(
        extra,
        Err(ComposeError::InputCountMismatch {
            expected: 2,
            actual: 3
        })
    ));
    for inputs in [
        &[(b"invalid", Compression::Other)][..],
        &[(b"invalid", Compression::Other); 3][..],
    ] {
        assert!(matches!(
            composer.compose_with_encodings(inputs),
            Err(ComposeError::InputCountMismatch { expected: 2, actual }) if actual == inputs.len()
        ));
    }
}

#[test]
fn empty_inputs_are_preserved_without_mutating_sources() {
    let composer = composer(&["a", "b"]);
    let first = Vec::<u8>::new();
    let second = b"second".to_vec();

    let output = composer.compose(&[&first, &second]).unwrap();

    assert_eq!(output.as_ref(), b"second");
    assert!(first.is_empty());
    assert_eq!(second, b"second");
}

#[test]
fn zero_byte_inputs_need_no_codec() {
    let composer = MvtComposer::builder()
        .add_source(MvtSource::new("roads").with_layers(["roads"]))
        .max_decoded_bytes(0)
        .build()
        .unwrap();

    for encoding in [
        Compression::None,
        Compression::Gzip,
        Compression::Zstd,
        Compression::Brotli,
        Compression::Other,
    ] {
        let source = MvtSource::new("roads").with_compression(encoding);
        assert!(matches!(
            source.decompress(b""),
            Ok(std::borrow::Cow::Borrowed([]))
        ));
        assert!(
            composer
                .compose_with_encodings(&[(b"", encoding)])
                .unwrap()
                .is_empty()
        );
    }
    assert!(matches!(
        composer.compose(&[b"x"]),
        Err(ComposeError::SizeOverflow)
    ));
    assert!(composer.compose(&[b""]).unwrap().is_empty());
}

#[test]
fn rejects_unavailable_request_encodings_for_nonempty_inputs() {
    let composer = composer(&["roads"]);
    for (encoding, enabled) in [
        (Compression::Gzip, cfg!(feature = "gzip")),
        (Compression::Zstd, cfg!(feature = "zstd")),
        (Compression::Brotli, cfg!(feature = "brotli")),
    ] {
        if !enabled {
            assert!(matches!(
                composer.compose_with_encodings(&[(b"x", encoding)]),
                Err(ComposeError::SourceDecompression {
                    source_id,
                    source: SourceError::CompressionFeatureDisabled { compression }
                }) if source_id.as_ref() == "roads" && compression == encoding
            ));
        }
    }
    assert!(matches!(
        composer.compose_with_encodings(&[(b"x", Compression::Other)]),
        Err(ComposeError::SourceDecompression {
            source_id,
            source: SourceError::UnsupportedCompression { compression: Compression::Other }
        }) if source_id.as_ref() == "roads"
    ));
}

#[test]
fn decoded_budget_counts_all_raw_sources_and_resets_per_call() {
    let composer = MvtComposer::builder()
        .add_source(MvtSource::new("roads").with_layers(["roads"]))
        .add_source(MvtSource::new("water").with_layers(["water"]))
        .max_decoded_bytes(4)
        .build()
        .unwrap();
    assert!(matches!(
        composer.compose(&[b"abc".as_slice(), b"de"]),
        Err(ComposeError::SizeOverflow)
    ));
    assert_eq!(composer.compose(&[b"ab", b"cd"]).unwrap(), b"abcd"[..]);
}

#[cfg(any(feature = "gzip", feature = "zstd", feature = "brotli"))]
#[test]
fn request_encodings_share_the_budget_and_keep_output_encoding_fixed() {
    for &(encoding, encode) in fixtures::CODECS {
        let source = MvtSource::new("water")
            .with_layers(["water"])
            .with_compression(encoding);
        let composer = MvtComposer::builder()
            .add_source(MvtSource::new("roads").with_layers(["roads"]))
            .add_source(source.clone())
            .output_compression(encoding)
            .max_decoded_bytes(4)
            .build()
            .unwrap();

        let prefix = b"ab".as_slice();
        let over = encode(b"cde");
        let exact = encode(b"cd");
        assert!(matches!(
            composer.compose(&[prefix, &over]),
            Err(ComposeError::SizeOverflow)
        ));
        assert!(matches!(
            composer.compose_with_encodings(&[(prefix, Compression::None), (&over, encoding)]),
            Err(ComposeError::SizeOverflow)
        ));
        assert_eq!(
            composer
                .compose_with_encodings(&[
                    (exact.clone(), encoding),
                    (prefix.to_vec(), Compression::None)
                ])
                .unwrap(),
            encode(b"cdab")
        );
        assert_eq!(
            composer.compose(&[prefix, &exact]).unwrap(),
            encode(b"abcd")
        );
        assert_eq!(composer.sources()[1].compression(), encoding);
        assert_eq!(composer.sources()[1].layers()[0].as_ref(), "water");

        assert!(composer.compose(&[b"", b""]).unwrap().is_empty());
        assert!(
            composer
                .compose_with_encodings(&[(b"", encoding), (b"", Compression::Other)])
                .unwrap()
                .is_empty()
        );
        let empty = encode(b"");
        assert!(!empty.is_empty());
        assert_eq!(composer.compose(&[b"".as_slice(), &empty]).unwrap(), empty);
        assert_eq!(
            composer
                .compose_with_encodings(&[(empty.as_slice(), encoding), (b"", Compression::Other)])
                .unwrap(),
            empty
        );
        assert_eq!(
            composer.compose(&[b"abcd".as_slice(), &empty]).unwrap(),
            encode(b"abcd")
        );

        if encoding != Compression::Brotli {
            let multistream = [exact.as_slice(), exact.as_slice()].concat();
            assert_eq!(
                composer.compose(&[b"".as_slice(), &multistream]).unwrap(),
                encode(b"cdcd")
            );
            assert!(matches!(
                composer.compose(&[prefix, &multistream]),
                Err(ComposeError::SizeOverflow)
            ));
        }

        let unlimited = MvtComposer::builder()
            .add_source(source)
            .max_decoded_bytes(usize::MAX)
            .build()
            .unwrap();
        assert_eq!(unlimited.compose(&[exact]).unwrap(), b"cd"[..]);
    }
}

#[cfg(feature = "gzip")]
#[test]
fn decompresses_each_source_before_composing() {
    let first = common::tile_with_layers(&["roads"]);
    let second = common::tile_with_layers(&["water"]);
    let encoded_first = common::gzip(&first);
    let composer = MvtComposer::builder()
        .add_source(
            MvtSource::new("roads")
                .with_compression(Compression::Gzip)
                .with_layers(["roads"]),
        )
        .add_source(MvtSource::new("water").with_layers(["water"]))
        .build()
        .unwrap();

    let output = composer
        .compose(&[encoded_first.as_slice(), second.as_slice()])
        .unwrap();
    let expected: Vec<u8> = first.into_iter().chain(second).collect();

    assert_eq!(output.as_ref(), expected);
}

#[cfg(feature = "gzip")]
#[test]
fn composes_all_layers_from_concatenated_gzip_members() {
    // Given: one gzip source declaring layers from two independent members.
    let first = common::gzip(&common::tile_with_layers(&["roads"]));
    let second = common::gzip(&common::tile_with_layers(&["buildings"]));
    let encoded: Vec<u8> = first.into_iter().chain(second).collect();
    let composer = MvtComposer::builder()
        .add_source(
            MvtSource::new("transport")
                .with_compression(Compression::Gzip)
                .with_layers(["roads", "buildings"]),
        )
        .build()
        .unwrap();

    // When: the composer receives the concatenated gzip input.
    let output = composer.compose(&[encoded.as_slice()]).unwrap();

    // Then: the composed tile contains all layers in declaration order.
    assert_eq!(
        fixtures::layer_names(&output),
        vec!["roads".to_owned(), "buildings".to_owned()]
    );
}

#[cfg(feature = "gzip")]
#[test]
fn reports_the_source_that_failed_decompression() {
    let composer = MvtComposer::builder()
        .add_source(
            MvtSource::new("roads")
                .with_compression(composite_mvt::Compression::Gzip)
                .with_layers(["roads"]),
        )
        .build()
        .unwrap();

    let error = composer.compose(&[b"not-gzip"]).unwrap_err();

    assert!(matches!(
        error,
        ComposeError::SourceDecompression { source_id, .. }
            if source_id.as_ref() == "roads"
    ));
}

#[cfg(feature = "gzip")]
#[test]
fn compresses_the_complete_output_with_dependency_defaults() {
    let first = common::tile_with_layers(&["roads"]);
    let second = common::tile_with_layers(&["water"]);
    let expected_raw: Vec<u8> = first.iter().chain(&second).copied().collect();
    let expected = common::gzip(&expected_raw);
    let composer = MvtComposer::builder()
        .add_source(MvtSource::new("roads").with_layers(["roads"]))
        .add_source(MvtSource::new("water").with_layers(["water"]))
        .output_compression(Compression::Gzip)
        .build()
        .unwrap();

    let output = composer.compose(&[first, second]).unwrap();

    assert_eq!(output.as_ref(), expected);
}

#[cfg(feature = "zstd")]
#[test]
fn compresses_the_complete_output_with_zstd_defaults() {
    let first = common::tile_with_layers(&["roads"]);
    let second = common::tile_with_layers(&["water"]);
    let expected_raw: Vec<u8> = first.iter().chain(&second).copied().collect();
    let expected = fixtures::zstd_encode(&expected_raw);
    let composer = MvtComposer::builder()
        .add_source(MvtSource::new("roads").with_layers(["roads"]))
        .add_source(MvtSource::new("water").with_layers(["water"]))
        .output_compression(Compression::Zstd)
        .build()
        .unwrap();

    let output = composer.compose(&[first, second]).unwrap();

    assert_eq!(output.as_ref(), expected);
}

#[cfg(feature = "brotli")]
#[test]
fn compresses_the_complete_output_with_brotli_defaults() {
    let first = common::tile_with_layers(&["roads"]);
    let second = common::tile_with_layers(&["water"]);
    let expected_raw: Vec<u8> = first.iter().chain(&second).copied().collect();
    let expected = fixtures::brotli_encode(&expected_raw);
    let composer = MvtComposer::builder()
        .add_source(MvtSource::new("roads").with_layers(["roads"]))
        .add_source(MvtSource::new("water").with_layers(["water"]))
        .output_compression(Compression::Brotli)
        .build()
        .unwrap();

    let output = composer.compose(&[first, second]).unwrap();

    assert_eq!(output.as_ref(), expected);
}
