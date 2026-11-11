# composite-mvt repository instructions

## Composition

- Preserve the source order established by `MvtComposerBuilder::add_source`. Each call to
  `MvtComposer::compose` takes one input per configured source in that order.
- Validate source metadata and duplicate layer names at build time. At request time, decode inputs
  with their configured codecs, concatenate raw MVT bytes, and apply the configured output codec
  once to the complete tile. See [the library contract](README.md#compression-model).

## Verification

- Verify compatibility with the minimum Rust version declared in [Cargo.toml](Cargo.toml). The
  [release workflow](.github/workflows/release.yml) runs
  `cargo +1.87.0 test --locked --all-targets --all-features`.
- For changes to codec or feature gating, also run `cargo test --locked --no-default-features`.
  The MapLibre example regression test needs permission to bind a loopback port.

## Packaging and release

- Keep the MapLibre browser and server demo, including offline tiles, available from a Git checkout.
  Keep it outside the package allowlist in [Cargo.toml](Cargo.toml). The crate includes the
  `mixed_sources` example.
- Before publishing, run `cargo package --locked` from the intended release commit. Measure the
  compressed `.crate` in `target/package/` and confirm it fits the
  [crates.io upload limit](https://doc.rust-lang.org/cargo/reference/publishing.html#packaging-a-crate)
  of 10 MiB.
- Verify the release tag points to the commit used for packaging and matches the manifest version.
  The [release workflow](.github/workflows/release.yml) checks out the tagged commit.
