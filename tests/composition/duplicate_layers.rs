use composite_mvt::{
    ComposeError, Compression, DuplicateLayer, MvtComposer, MvtSource, SourceError,
};
use fast_mvt::{MvtCoord, MvtGeomType, MvtReaderRef, MvtTileBuilder, MvtValueRef};

const KEEP_POLICIES: [DuplicateLayer; 2] = [DuplicateLayer::KeepFirst, DuplicateLayer::KeepLast];

fn layer(name: &str, id: u64) -> Vec<u8> {
    let mut feature = MvtTileBuilder::new()
        .layer(name)
        .unwrap()
        .feature_of(MvtGeomType::Point);
    feature.id(Some(id)).tag_uint("source", id).unwrap();
    feature.points([MvtCoord { x: id as i32, y: 7 }]).unwrap();
    feature.end().end().encode()
}

fn builder(policy: DuplicateLayer) -> composite_mvt::MvtComposerBuilder {
    MvtComposer::builder()
        .duplicate_layer(policy)
        .add_source(MvtSource::new("a").with_layers(["shared"]))
        .add_source(MvtSource::new("b").with_layers(["shared"]))
}

#[test]
fn each_tile_selects_its_own_winners_in_source_and_field_order() {
    let sources = ["a", "b", "c"];
    let names = [
        ["shared", "alpha", "other"],
        ["beta", "other", "shared"],
        ["shared", "gamma", "third"],
    ];
    let orders = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];

    for (policy, order) in KEEP_POLICIES
        .into_iter()
        .flat_map(|policy| orders.map(|order| (policy, order)))
    {
        let names = order.map(|source| names[source]);
        let ids = |index| (order[index / 3] * 3 + index % 3 + 1) as u64;
        let fields: [Vec<u8>; 9] =
            std::array::from_fn(|index| layer(names.as_flattened()[index], ids(index)));
        let composer = order
            .map(|source| sources[source])
            .into_iter()
            .zip(names)
            .fold(
                MvtComposer::builder().duplicate_layer(policy),
                |builder, (id, names)| builder.add_source(MvtSource::new(id).with_layers(names)),
            )
            .build()
            .unwrap();

        for mask in 0..(1 << fields.len()) {
            let selected: Vec<_> = (0..fields.len())
                .filter(|index| mask & (1 << index) != 0)
                .collect();
            let inputs: [Vec<u8>; 3] = std::array::from_fn(|source| {
                selected
                    .iter()
                    .copied()
                    .filter(|index| index / 3 == source)
                    .flat_map(|index| fields[index].iter().copied())
                    .collect()
            });
            let expected: Vec<_> = selected
                .iter()
                .copied()
                .filter(|&index| {
                    !selected.iter().copied().any(|other| {
                        let preferred = match policy {
                            DuplicateLayer::KeepFirst => other < index,
                            DuplicateLayer::KeepLast => other > index,
                            _ => unreachable!(),
                        };
                        preferred && names.as_flattened()[index] == names.as_flattened()[other]
                    })
                })
                .collect();
            let output = composer.compose(&inputs).unwrap();
            let expected_bytes: Vec<_> = expected
                .iter()
                .flat_map(|&index| fields[index].iter().copied())
                .collect();
            assert_eq!(
                output, expected_bytes,
                "{policy:?} in {order:?} at {mask:#05x}"
            );

            let decoded = MvtReaderRef::new(&output).unwrap();
            assert_eq!(decoded.layers().count(), expected.len());
            for (layer, index) in decoded.layers().zip(expected) {
                assert_eq!(layer.name(), names.as_flattened()[index]);
                let mut features = layer.features();
                let feature = features.next().unwrap();
                let id = ids(index);
                assert_eq!(feature.id(), Some(id));
                assert_eq!(
                    feature.properties_vec().unwrap(),
                    [("source", MvtValueRef::UInt(id))]
                );
                assert_eq!(feature.geometry_commands(), [9, 2 * id as u32, 14]);
                assert!(features.next().is_none());
            }
        }
    }
}

#[test]
fn selects_shared_layers_among_many_sparse_sources() {
    let names = ["shared", "b", "c", "d", "e", "f", "g", "shared"];
    let inputs: [Vec<u8>; 8] = std::array::from_fn(|i| match i {
        1 | 5 => Vec::new(),
        _ => layer(names[i], i as u64),
    });
    for (policy, keep) in [
        (DuplicateLayer::KeepFirst, [0, 2, 3, 4, 6]),
        (DuplicateLayer::KeepLast, [2, 3, 4, 6, 7]),
    ] {
        let mut builder = MvtComposer::builder().duplicate_layer(policy);
        for (i, name) in names.iter().enumerate() {
            builder = builder.add_source(MvtSource::new(i.to_string()).with_layers([*name]));
        }
        let expected = keep.map(|i| inputs[i].as_slice()).concat();
        assert_eq!(builder.build().unwrap().compose(&inputs).unwrap(), expected);
    }
}

#[test]
fn scanned_sources_preserve_unknown_fields_and_exact_winner_bytes() {
    let first = b"\x1a\x0b\x78\x02\x0a\x01x\x0a\x01s\x42\x01a";
    let last = b"\x1a\x08\x78\x02\x0a\x01s\x42\x01b";
    let unknown = b"\x2b\x30\x81\x00\x2c";
    let first_input = [unknown.as_slice(), first.as_slice()].concat();
    let last_input = [unknown.as_slice(), last.as_slice()].concat();

    for (policy, expected) in [
        (
            DuplicateLayer::KeepFirst,
            [unknown.as_slice(), first, unknown].concat(),
        ),
        (
            DuplicateLayer::KeepLast,
            [unknown.as_slice(), unknown, last].concat(),
        ),
    ] {
        let composer = MvtComposer::builder()
            .duplicate_layer(policy)
            .add_source(MvtSource::new("a").with_layers(["s"]))
            .add_source(MvtSource::new("b").with_layers(["s"]))
            .build()
            .unwrap();
        assert_eq!(
            composer.compose(&[&first_input, &last_input]).unwrap(),
            expected
        );
    }
}

#[test]
fn allow_and_disjoint_sources_keep_opaque_bytes() {
    let first = layer("shared", 1);
    let last = layer("shared", 2);
    let allow = builder(DuplicateLayer::Allow).build().unwrap();
    let inputs = [first.as_slice(), last.as_slice()];
    assert_eq!(allow.compose(&inputs).unwrap(), inputs.concat());

    for policy in KEEP_POLICIES {
        let disjoint = MvtComposer::builder()
            .duplicate_layer(policy)
            .add_source(MvtSource::new("a").with_layers(["a"]))
            .add_source(MvtSource::new("b").with_layers(["b"]))
            .build()
            .unwrap();
        let opaque = b"\xff";
        assert_eq!(
            disjoint.compose(&[opaque, opaque]).unwrap(),
            b"\xff\xff"[..]
        );

        let partially_disjoint = builder(policy)
            .add_source(MvtSource::new("c").with_layers(["c"]))
            .build()
            .unwrap();
        let winner = if policy == DuplicateLayer::KeepFirst {
            &first
        } else {
            &last
        };
        assert_eq!(
            partially_disjoint
                .compose(&[first.as_slice(), last.as_slice(), opaque])
                .unwrap(),
            [winner.as_slice(), opaque].concat(),
        );
    }
}

#[test]
fn scanned_sources_report_malformed_wire_data_with_source_id() {
    let first = layer("shared", 1);
    let invalid: &[&[u8]] = &[
        b"\x00",
        b"\x18\x00",
        b"\x80\x80\x80\x80\x10",
        b"\x1a\xff\xff\xff\xff\xff\xff\xff\xff\xff\x02",
        b"\x1a\x02\x00",
        b"\x21\x00",
        b"\x2b\x34",
        b"\x1a\x02\x08\x00",
        b"\x1a\x03\x0a\x01\xff",
        b"\x1a\x04\x0a\x01a\x4e",
        b"\x1a\x06\x0a\x01a\x12\x01\x80",
        b"\x1a\x08\x0a\x01a\x12\x03\x22\x01\x80",
        b"\x1a\x06\x0a\x01a\x22\x01\x80",
    ];
    for policy in KEEP_POLICIES {
        let composer = builder(policy).build().unwrap();
        for &raw in invalid {
            assert!(
                matches!(
                    composer.compose(&[first.as_slice(), raw]),
                    Err(ComposeError::SourceParsing { source_id, source: SourceError::InvalidMvt })
                        if source_id.as_ref() == "b"
                ),
                "{raw:?}"
            );
        }
        for raw in [b"\x1a\x00", b"\x1a\x02\x0a\x00".as_slice()] {
            assert!(matches!(
                composer.compose_with_encodings(&[(raw, Compression::None), (b"", Compression::Other)]),
                Err(ComposeError::SourceParsing { source_id, source: SourceError::MissingLayerName })
                    if source_id.as_ref() == "a"
            ));
        }
    }
}

#[cfg(any(feature = "gzip", feature = "zstd", feature = "brotli"))]
#[test]
fn checks_the_decoded_budget_before_discarding_layers() {
    let first = layer("shared", 1);
    let last = layer("shared", 2);
    for (&(encoding, encode), policy) in super::fixtures::CODECS
        .iter()
        .flat_map(|codec| KEEP_POLICIES.map(|policy| (codec, policy)))
    {
        let encoded_first = encode(&first);
        let encoded_last = encode(&last);
        let builder = |budget| {
            MvtComposer::builder()
                .duplicate_layer(policy)
                .add_source(MvtSource::new("a").with_layers(["shared"]))
                .add_source(
                    MvtSource::new("b")
                        .with_layers(["shared"])
                        .with_compression(encoding),
                )
                .output_compression(encoding)
                .max_decoded_bytes(budget)
                .build()
                .unwrap()
        };
        let inputs = [
            (encoded_first.as_slice(), encoding),
            (last.as_slice(), Compression::None),
        ];
        let composer = builder(first.len() + last.len());
        let winner = if policy == DuplicateLayer::KeepFirst {
            &first
        } else {
            &last
        };
        let output = encode(winner);
        assert_eq!(composer.compose(&[&first, &encoded_last]).unwrap(), output);
        assert_eq!(composer.compose_with_encodings(&inputs).unwrap(), output);
        assert_eq!(
            composer.compose(&[&first, &encode(b"")]).unwrap(),
            encode(&first)
        );
        assert!(matches!(
            builder(first.len() + last.len() - 1).compose_with_encodings(&inputs),
            Err(ComposeError::SizeOverflow)
        ));
    }
}
