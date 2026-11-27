# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- Reject composite tiles that exceed the Protobuf message size limit before allocating the output.
- Report the MVT sample reader's element memory limit as a resource error.
- Include the source repository in the crate metadata.
- Serve the bundled MapLibre GL JS license and show data attribution for the OpenFreeMap sources.
- Update the MapLibre example's HTTP/2 and TLS dependencies to patched versions.

## [0.3.0] - 2026-11-22

### Added

- Set an optional aggregate limit on decoded request inputs with `max_decoded_bytes`.
- Supply each request input's encoding with `compose_with_encodings`.
- Resolve duplicate layer names per tile with `DuplicateLayer::KeepFirst` and `KeepLast`.

### Upgrade notes

- Update exhaustive matches on `DuplicateLayer` for `KeepFirst` and `KeepLast`, and on
  `ComposeError` for `SourceParsing`.
- Omit `Content-Encoding` when every input is a zero-byte slice. The result is empty for all
  output encodings.

### Fixed

- Treat zero-byte request inputs as empty tiles for any encoding. Requests with only zero-byte
  inputs return empty bytes without allocating or applying output compression.

## [0.2.2] - 2026-11-11

### Fixed

- Check the crates.io sparse index for existing versions before publishing from GitHub Actions.

## [0.2.1] - 2026-11-11

### Changed

- Update `fast-mvt` to `0.7.2` while retaining Rust 1.87 compatibility.

### Fixed

- Keep the MapLibre demo and its offline tiles in the Git repository so the crates.io archive fits
  the registry upload limit.

## [0.2.0] - 2026-09-12

### Fixed

- Reject trailing or concatenated Brotli input instead of silently decoding only the first stream.
- Allow MapLibre example configuration updates to complete while older tile requests are in flight.
- Add regression coverage for Brotli stream boundaries and configuration snapshot isolation.

## [0.1.0] - 2026-07-18

### Added

- Source metadata parsing from representative raw, gzip, Zstandard, and explicitly configured
  Brotli MVT samples.
- Optional gzip (default), Zstandard, and Brotli source decoding and complete-output encoding.
- Immutable `MvtComposer` construction with source ID, layer, compression-feature, and output
  validation.
- Independent duplicate-layer validation with configurable `Allow` and `Error` policies.
- Lock-free sharing of a built composer and independent request results.
- One-pass, whole-composite output compression suitable for HTTP `Content-Encoding` delivery.
- Rustdoc, README, example, feature-matrix, and package verification for the first publishable
  crate, with a documented Rust 1.87 MSRV. Exact Rust 1.87 verification remains pending because
  the configured mirror returned HTTP 404 and the official-source cargo download stalled.
