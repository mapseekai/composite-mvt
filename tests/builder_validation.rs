use composite_mvt::{BuildError, Compression, DuplicateLayer, MvtComposer, MvtSource};

fn source(id: &str, layers: &[&str]) -> MvtSource {
    MvtSource::new(id).with_layers(layers.iter().copied())
}

#[track_caller]
fn assert_duplicate_layer(error: &BuildError, expected: (&str, &str, &str)) {
    let (layer, first, second) = error
        .duplicate_layer_name()
        .expect("expected a duplicate layer");
    assert_eq!((layer.as_ref(), first.as_ref(), second.as_ref()), expected);
}

#[test]
fn rejects_no_sources_and_duplicate_ids() {
    let builder = MvtComposer::builder();
    let _: &dyn std::fmt::Debug = &builder;
    let error = builder.build().unwrap_err();
    let _: &(dyn std::error::Error + Send + Sync) = &error;
    let _: &std::backtrace::Backtrace = error.backtrace();
    assert!(error.is_no_sources());

    let error = MvtComposer::builder()
        .add_source(source("roads", &["roads"]))
        .add_source(source("roads", &["labels"]))
        .build()
        .unwrap_err();
    assert!(error.is_duplicate_source_id());
    assert_eq!(error.source_id().map(AsRef::as_ref), Some("roads"));
}

#[test]
fn explicit_duplicate_validation_matches_build() {
    let builder = MvtComposer::builder()
        .duplicate_layer(DuplicateLayer::Error)
        .add_source(source("a", &["shared"]))
        .add_source(source("b", &["shared"]));

    for result in [
        builder.validate_duplicate_layers(),
        builder.build().map(drop),
    ] {
        assert_duplicate_layer(&result.unwrap_err(), ("shared", "a", "b"));
    }
}

#[test]
fn allows_cross_source_duplicates_when_configured() {
    for policy in [
        DuplicateLayer::Allow,
        DuplicateLayer::KeepFirst,
        DuplicateLayer::KeepLast,
    ] {
        let builder = MvtComposer::builder()
            .duplicate_layer(policy)
            .add_source(source("a", &["shared"]))
            .add_source(source("b", &["shared"]));
        assert!(builder.validate_duplicate_layers().is_ok());
        let composer = builder.build().unwrap();

        assert_eq!(composer.sources().len(), 2);
        assert_eq!(composer.output_compression(), Compression::None);
    }
}

#[test]
fn rejects_missing_and_empty_layers() {
    let no_layers = MvtComposer::builder()
        .add_source(MvtSource::new("empty"))
        .build()
        .unwrap_err();
    assert!(no_layers.is_no_layers());

    let empty_name = MvtComposer::builder()
        .add_source(source("empty", &[""]))
        .build()
        .unwrap_err();
    assert!(empty_name.is_empty_layer_name());
}

#[test]
fn same_source_duplicates_are_always_invalid() {
    for policy in [
        DuplicateLayer::Error,
        DuplicateLayer::Allow,
        DuplicateLayer::KeepFirst,
        DuplicateLayer::KeepLast,
    ] {
        let builder = MvtComposer::builder()
            .duplicate_layer(policy)
            .add_source(source("roads", &["roads", "roads"]));

        for result in [
            builder.validate_duplicate_layers(),
            builder.build().map(drop),
        ] {
            assert_duplicate_layer(&result.unwrap_err(), ("roads", "roads", "roads"));
        }
    }
}

#[test]
fn duplicate_source_ids_precede_per_source_errors() {
    // Given: the first source is incomplete and a later source repeats its ID.
    let builder = MvtComposer::builder()
        .add_source(source("same", &[""]))
        .add_source(MvtSource::new("same"));

    // When: the complete builder is validated.
    let error = builder.build().unwrap_err();

    // Then: the global source-ID pass wins over source-local errors.
    assert!(error.is_duplicate_source_id());
    assert_eq!(error.source_id().map(AsRef::as_ref), Some("same"));
}

#[test]
fn missing_layers_across_all_sources_precede_earlier_empty_names() {
    // Given: an early source has an empty name and a later source has no layers.
    let builder = MvtComposer::builder()
        .add_source(source("early", &[""]))
        .add_source(MvtSource::new("later"));

    // When: the complete builder is validated.
    let error = builder.build().unwrap_err();

    // Then: the all-sources no-layer pass completes before empty names are checked.
    assert!(error.is_no_layers());
    assert_eq!(error.source_id().map(AsRef::as_ref), Some("later"));
}

#[test]
fn duplicate_layers_precede_unsupported_source_compression() {
    // Given: the second source duplicates a layer and uses an unsupported codec.
    let builder = MvtComposer::builder()
        .add_source(source("first", &["shared"]))
        .add_source(source("second", &["shared"]).with_compression(Compression::Other));

    // When: the complete builder is validated.
    let error = builder.build().unwrap_err();

    // Then: duplicate validation runs before source compression validation.
    assert_duplicate_layer(&error, ("shared", "first", "second"));
}

#[cfg(not(feature = "zstd"))]
#[test]
fn duplicate_layers_precede_disabled_source_compression() {
    // Given: the second source duplicates a layer and requires disabled zstd support.
    let builder = MvtComposer::builder()
        .add_source(source("first", &["shared"]))
        .add_source(source("second", &["shared"]).with_compression(Compression::Zstd));

    // When: the complete builder is validated.
    let error = builder.build().unwrap_err();

    // Then: duplicate validation runs before feature validation.
    assert_duplicate_layer(&error, ("shared", "first", "second"));
}

#[test]
fn rejects_other_for_input_and_output() {
    let input = MvtComposer::builder()
        .add_source(source("roads", &["roads"]).with_compression(Compression::Other))
        .build()
        .unwrap_err();
    assert!(input.is_unsupported_compression());
    assert_eq!(input.source_id().map(AsRef::as_ref), Some("roads"));
    assert_eq!(input.compression(), Some(Compression::Other));

    let output = MvtComposer::builder()
        .output_compression(Compression::Other)
        .add_source(source("roads", &["roads"]))
        .build()
        .unwrap_err();
    assert!(output.is_unsupported_output_compression());
    assert_eq!(output.compression(), Some(Compression::Other));
}

#[cfg(not(feature = "zstd"))]
#[test]
fn rejects_disabled_input_and_output_features() {
    let input = MvtComposer::builder()
        .add_source(source("roads", &["roads"]).with_compression(Compression::Zstd))
        .build()
        .unwrap_err();
    assert!(input.is_compression_feature_disabled());
    assert_eq!(input.compression(), Some(Compression::Zstd));

    let output = MvtComposer::builder()
        .output_compression(Compression::Zstd)
        .add_source(source("roads", &["roads"]))
        .build()
        .unwrap_err();
    assert!(output.is_output_compression_feature_disabled());
    assert_eq!(output.compression(), Some(Compression::Zstd));
}

#[test]
fn preserves_source_order() {
    let composer = MvtComposer::builder()
        .add_source(source("roads", &["roads"]))
        .add_source(source("buildings", &["building"]))
        .build()
        .unwrap();

    assert_eq!(
        composer
            .sources()
            .iter()
            .map(|source| source.id().as_ref())
            .collect::<Vec<_>>(),
        ["roads", "buildings"]
    );
}
