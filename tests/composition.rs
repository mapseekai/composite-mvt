#[cfg(any(feature = "gzip", feature = "zstd", feature = "brotli"))]
mod common;
#[path = "composition/concurrency.rs"]
mod concurrency;
#[path = "composition/duplicate_layers.rs"]
mod duplicate_layers;

#[cfg(any(feature = "gzip", feature = "zstd", feature = "brotli"))]
#[path = "composition/fixtures.rs"]
mod fixtures;

use composite_mvt::{Compression, MvtComposer, MvtSource};

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

    let missing = composer.compose(&[b"only"]).unwrap_err();
    let extra = composer
        .compose(&[&b"one"[..], &b"two"[..], &b"three"[..]])
        .unwrap_err();
    assert_eq!(missing.input_count_mismatch(), Some((2, 1)));
    assert_eq!(extra.input_count_mismatch(), Some((2, 3)));
    for inputs in [
        &[(b"invalid", Compression::Other)][..],
        &[(b"invalid", Compression::Other); 3][..],
    ] {
        let error = composer.compose_with_encodings(inputs).unwrap_err();
        assert_eq!(error.input_count_mismatch(), Some((2, inputs.len())));
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
    assert!(composer.compose(&[b"x"]).unwrap_err().is_size_overflow());
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
            let error = composer
                .compose_with_encodings(&[(b"x", encoding)])
                .unwrap_err();
            assert!(error.is_source_decompression());
            assert_eq!(error.source_id().map(AsRef::as_ref), Some("roads"));
            let source = error.source_error().unwrap();
            assert!(source.is_compression_feature_disabled());
            assert_eq!(source.compression(), Some(encoding));
        }
    }
    let error = composer
        .compose_with_encodings(&[(b"x", Compression::Other)])
        .unwrap_err();
    assert!(error.is_source_decompression());
    assert_eq!(error.source_id().map(AsRef::as_ref), Some("roads"));
    let source = error.source_error().unwrap();
    assert!(source.is_unsupported_compression());
    assert_eq!(source.compression(), Some(Compression::Other));
}

#[test]
fn decoded_budget_counts_all_raw_sources_and_resets_per_call() {
    let composer = MvtComposer::builder()
        .add_source(MvtSource::new("roads").with_layers(["roads"]))
        .add_source(MvtSource::new("water").with_layers(["water"]))
        .max_decoded_bytes(4)
        .build()
        .unwrap();
    assert!(
        composer
            .compose(&[b"abc".as_slice(), b"de"])
            .unwrap_err()
            .is_size_overflow()
    );
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
        assert!(
            composer
                .compose(&[prefix, &over])
                .unwrap_err()
                .is_size_overflow()
        );
        assert!(
            composer
                .compose_with_encodings(&[(prefix, Compression::None), (&over, encoding)])
                .unwrap_err()
                .is_size_overflow()
        );
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
            assert!(
                composer
                    .compose(&[prefix, &multistream])
                    .unwrap_err()
                    .is_size_overflow()
            );
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
    use std::error::Error;

    let composer = MvtComposer::builder()
        .add_source(
            MvtSource::new("roads")
                .with_compression(composite_mvt::Compression::Gzip)
                .with_layers(["roads"]),
        )
        .build()
        .unwrap();

    let error = composer.compose(&[b"not-gzip"]).unwrap_err();

    let _: &(dyn Error + Send + Sync) = &error;
    let _: &std::backtrace::Backtrace = error.backtrace();
    assert!(error.is_source_decompression());
    assert_eq!(error.source_id().map(AsRef::as_ref), Some("roads"));
    let source = error.source_error().unwrap();
    assert!(source.is_decompression_failed());
    assert_eq!(source.compression(), Some(Compression::Gzip));
    assert!(error.source().unwrap().is::<composite_mvt::SourceError>());
    assert!(source.source().unwrap().is::<std::io::Error>());
}

#[cfg(feature = "gzip")]
#[test]
fn compresses_the_complete_output_with_dependency_defaults() {
    assert_complete_output_encoding(Compression::Gzip, common::gzip);
}

#[cfg(feature = "zstd")]
#[test]
fn compresses_the_complete_output_with_zstd_defaults() {
    assert_complete_output_encoding(Compression::Zstd, fixtures::zstd_encode);
}

#[cfg(feature = "brotli")]
#[test]
fn compresses_the_complete_output_with_brotli_defaults() {
    assert_complete_output_encoding(Compression::Brotli, fixtures::brotli_encode);
}

#[cfg(any(feature = "gzip", feature = "zstd", feature = "brotli"))]
fn assert_complete_output_encoding(compression: Compression, encode: fn(&[u8]) -> Vec<u8>) {
    let first = common::tile_with_layers(&["roads"]);
    let second = common::tile_with_layers(&["water"]);
    let expected_raw: Vec<u8> = first.iter().chain(&second).copied().collect();
    let expected = encode(&expected_raw);
    let composer = MvtComposer::builder()
        .add_source(MvtSource::new("roads").with_layers(["roads"]))
        .add_source(MvtSource::new("water").with_layers(["water"]))
        .output_compression(compression)
        .build()
        .unwrap();

    let output = composer.compose(&[first, second]).unwrap();

    assert_eq!(output.as_ref(), expected);
}
