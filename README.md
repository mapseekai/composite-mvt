# composite-mvt

[简体中文](README_CN.md)

`composite-mvt` composes a fixed set of Mapbox Vector Tile (MVT) sources into one response body.
Configure source metadata once, then supply one input per source for every request. The composer
works at the byte level and can resolve overlapping layer names against the layers present in each
tile.

## Quick start

```rust
use composite_mvt::{Compression, DuplicateLayer, MvtComposer, MvtSource};

# fn run(roads: &[u8], buildings_gzip: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
let composer = MvtComposer::builder()
    .duplicate_layer(DuplicateLayer::Error)
    .output_compression(Compression::Gzip)
    .add_source(MvtSource::new("roads").with_layers(["roads"]))
    .add_source(
        MvtSource::new("buildings")
            .with_compression(Compression::Gzip)
            .with_layers(["building"]),
    )
    .build()?;

let response_body = composer.compose(&[roads, buildings_gzip])?;
assert_eq!(composer.output_compression().content_encoding(), Some("gzip"));
# let _ = response_body;
# Ok(())
# }
```

The source order is fixed by `add_source`. `compose(&inputs)` requires exactly that many inputs,
and `inputs[n]` always belongs to source `n`.

Supply input tiles with the same projection, geographic bounds, and coordinate orientation. Check
the tile scheme as well as the XYZ values when combining tiles from different sources.

## Compression model

`compose` uses the encoding configured on each `MvtSource`. When the encoding can vary by request,
call `compose_with_encodings(&[(bytes, encoding), …])` in the same source order. Each pair selects the
decoder for that input only. The source IDs and declared layers still come from the built composer.
Both methods borrow raw inputs, decode compressed inputs, and combine the resulting MVT bytes in
source order under the configured duplicate-layer policy. A zero-byte input contributes an empty
tile and requires no codec support. When every input is zero bytes, both methods return empty bytes
without allocating or using codecs.

The composer then returns the raw composite unchanged or applies its fixed output compression once
to the **complete** composite MVT. It never emits one compressed stream per source. Encoder settings
are not part of the public API: enabled codecs use their default parameters, and the output format
cannot be overridden per request. A nonempty compressed input that decodes to zero bytes still
follows the normal output encoding path.

The complete uncompressed result must be smaller than 2 GiB, even when compressed output is
requested. See the [Protobuf message size limit](https://protobuf.dev/programming-guides/encoding/#encoded-proto-size-limitations).

With flate2 1.1.9, the optional file name and comment in each gzip input member are each limited to
65,535 bytes, excluding their terminating NUL bytes.

For source metadata, `MvtSource::from_mvt` and `from_mvts` auto-detect only gzip and Zstandard
frames; other samples are treated as raw MVT. Use `from_mvt_with_compression` or
`from_mvts_with_compression` with `Compression::Brotli` for Brotli samples. Sample constructors
reject empty bytes.

When returning compressed bytes through HTTP, set the response header from
`composer.output_compression().content_encoding()`. Omit `Content-Encoding` for the empty result
from all zero-byte inputs.

| Output | `Content-Encoding` |
| --- | --- |
| `Compression::None` | omit the header |
| `Compression::Gzip` | `gzip` |
| `Compression::Zstd` | `zstd` |
| `Compression::Brotli` | `br` |

Gzip output is a single gzip member around the complete composite MVT. This crate creates bytes,
not HTTP responses, so the calling server owns headers and cache policy.

## Features

| Feature | Default | Effect |
| --- | --- | --- |
| `gzip` | yes | Enables gzip source decoding and complete-output encoding. |
| `zstd` | no | Enables Zstandard source decoding and complete-output encoding. |
| `brotli` | no | Enables explicitly configured Brotli source decoding and complete-output encoding. |

Selecting an unavailable codec is rejected while building the composer. `Compression::Other` is an
unsupported marker and is never a valid source or output configuration. A nonempty input passed
to `compose_with_encodings` with a disabled or unsupported encoding returns a source error.

## Validation and errors

`MvtComposerBuilder::validate_duplicate_layers()` can be called independently and does not consume
or modify the builder. The builder rejects repeated names in a single source's declarations.
Between distinct sources, `DuplicateLayer::Error` (the default) rejects the repeat and `DuplicateLayer::Allow`
accepts and preserves repeated layers. Opting in may produce non-conformant output because MVT 2.1
requires layer names within one tile to be byte-for-byte unique. See the [MVT 2.1
specification](https://github.com/mapbox/vector-tile-spec/blob/master/2.1/README.md#41-layers).
`build()` runs the same duplicate validation along with source ID, layer, and feature checks.

`DuplicateLayer::KeepFirst` and `DuplicateLayer::KeepLast` accept overlapping declarations and keep
the first or last occurrence of each layer in source order for the current tile. For example,
`KeepLast` keeps the earlier source's `roads` when the later tile contains only `water` or is empty.
Declare every layer a source can emit. Each input tile may contain any subset of those layers
and must have unique layer names. For sources with overlapping declarations,
the keep policies read layer names and check protobuf framing, including embedded feature and
value fields. The composer copies retained fields byte-for-byte in their original order, including
unknown fields. Sources with disjoint declarations bypass the scan.

Sample construction and source decoding return `SourceError`. Configuration returns `BuildError`,
and request composition returns `ComposeError`. Use `BuildError::duplicate_layer_name()` to inspect
repeated declarations and `ComposeError::source_id()` to identify a source that failed decoding or
scanning. Use `SourceError::is_missing_layer_name()` for omitted or empty names in MVT data and
`BuildError::is_empty_layer_name()` for empty names in the configuration. Each error exposes a
`backtrace()` and any underlying failures through `std::error::Error::source()`.
Composition returns a complete result or an error.

Exceeding the sample reader's element memory limit produces a resource error. Use a smaller
representative sample that covers the same layers or declare all source layers explicitly.

## Concurrency and memory

After `build()`, `MvtComposer` is immutable and contains no mutex, cache, or request-level mutable
state. Share it with `Arc<MvtComposer>` across threads without library-managed locking. The composer
borrows raw inputs and owns the buffers decoded from nonempty compressed inputs. It builds the raw
composite in a separate buffer and uses another buffer when encoding the output. Layer scans use
request-local indexes of borrowed names and fields.

Set `MvtComposerBuilder::max_decoded_bytes(max)` to cap the combined size of the decoded inputs,
including raw bytes, in each call to either composition method. The default is `usize::MAX`.
Totals equal to the limit are allowed, including zero. The composer enforces the limit during
decoding. Use `ComposeError::is_size_overflow()` to identify a limit error. The limit covers the
decoded payload, including layers discarded by the duplicate-layer policy. Codec working memory
and encoded output use additional memory. Standalone `MvtSource::decompress` and the sample
constructors run outside the composer's budget.

## Example

Run the observable mixed-input example (enabled by the default `gzip` feature):

```text
cargo run --example mixed_sources
```

It prints `compression=gzip` and `layers=roads,pipeline,valve,building`, after reading the final
output back as MVT.

## MapLibre browser example

The browser example is one plain HTML page. It lets you add any number of vector-tile URL templates,
the source-layer names, input compression, render type, and color. The Rust backend saves that source
configuration, fetches the matching tiles, combines them with `MvtComposer`, and returns one MVT.

Run the example from a checkout of the Git repository, which contains the browser assets and
offline tiles:

```text
cargo run --example maplibre_server --features gzip
```

Open `http://127.0.0.1:3010`, add or remove tile sources, then select **应用并显示**. Each row is one
MVT source and accepts comma-separated source-layer names; point, line, and polygon features are
styled automatically. The defaults merge bundled Europe tiles at z=2 through z=5 from the MapLibre
demo source containing `geolines`, `centroids`, and `countries`, with OpenFreeMap tiles containing
`landuse`, so the example works offline across those zoom levels. The example uses an asynchronous
Reqwest client to fetch configured tile sources concurrently and includes MapLibre GL JS 5.24.0 locally.

## Testing

```text
cargo test --all-targets --all-features
cargo bench --locked --bench compose_raw
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option.
