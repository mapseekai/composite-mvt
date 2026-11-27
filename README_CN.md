# composite-mvt

[English](README.md)

`composite-mvt` 将一组固定的 Mapbox Vector Tile（MVT）源合成为一个响应体。
配置一次源元数据后，每个请求按顺序提供每个源的一段字节。合成器在字节层面处理数据，并可按当前瓦片实际包含的图层处理重名。

## 快速开始

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

`add_source` 的调用顺序固定了源顺序。`compose(&inputs)` 必须接收相同数量的输入，且 `inputs[n]` 始终对应第 `n` 个源。

调用方须确保各输入瓦片的投影、地理范围和坐标方向一致。合并不同来源的瓦片时，请同时核对瓦片方案和 XYZ 值。

## 压缩模型

`compose` 使用各个 `MvtSource` 配置的压缩格式。如果输入格式随请求变化，可以按相同的源顺序调用 `compose_with_encodings(&[(bytes, encoding), …])`。每个二元组为本次调用的对应输入指定解码格式，源 ID 与声明的图层仍取自已构建的合成器。两种方法均借用原始输入、解码压缩输入，再按源顺序和配置的重名策略合并原始 MVT 字节。零字节输入按空瓦片处理，无需启用对应编解码器。所有输入均为零字节时，两种方法均直接返回空字节，无需分配内存或调用编解码器。

其余情况下，合成器要么原样返回合并后的原始 MVT，要么对**完整**合成结果应用一次固定的输出压缩。编码器使用默认参数，输出格式在构建时确定。非空压缩输入即使解码为零字节，仍按配置编码输出。

无论输出是否压缩，完整瓦片的原始 Protobuf 长度都必须小于 2 GiB。[Protobuf 消息长度限制](https://protobuf.dev/programming-guides/encoding/#encoded-proto-size-limitations)

使用 flate2 1.1.9 时，每个 gzip 输入成员的可选文件名和注释分别最多为 65,535 字节，不计末尾的 NUL。

对于源元数据，`MvtSource::from_mvt` 与 `from_mvts` 只会自动识别 gzip 和 Zstandard 帧，其他样本按原始 MVT 处理。Brotli 样本须通过 `from_mvt_with_compression` 或 `from_mvts_with_compression` 显式指定 `Compression::Brotli`。样本构造方法会拒绝零字节输入。

通过 HTTP 返回压缩字节时，应使用 `composer.output_compression().content_encoding()` 设置响应头。所有输入均为零字节、结果为空时，省略 `Content-Encoding`。

| 输出 | `Content-Encoding` |
| --- | --- |
| `Compression::None` | 不设置该响应头 |
| `Compression::Gzip` | `gzip` |
| `Compression::Zstd` | `zstd` |
| `Compression::Brotli` | `br` |

Gzip 输出是包裹完整合成 MVT 的单个 gzip member。此 crate 只创建字节，不创建 HTTP 响应；响应头和缓存策略由调用方负责。

## 功能特性

| 功能 | 默认启用 | 作用 |
| --- | --- | --- |
| `gzip` | 是 | 启用 gzip 源解码与完整输出压缩。 |
| `zstd` | 否 | 启用 Zstandard 源解码与完整输出压缩。 |
| `brotli` | 否 | 启用显式配置的 Brotli 源解码与完整输出压缩。 |

构建合成器时，选择未启用的编解码器会被拒绝。`Compression::Other` 是不受支持的标记，不能作为有效的源或输出压缩格式。通过 `compose_with_encodings` 传入非空输入时，指定未启用或不支持的编码会返回源错误。

## 校验与错误

`MvtComposerBuilder::validate_duplicate_layers()` 可独立调用，不会消费或修改 builder。同一来源重复声明图层名时，构建器会报错。不同来源声明同名图层时，默认的 `DuplicateLayer::Error` 会报错。`DuplicateLayer::Allow` 保留所有图层，因此可能产生含重复图层名的非标准瓦片。[MVT 2.1 规范](https://github.com/mapbox/vector-tile-spec/blob/master/2.1/README.md#41-layers)

`DuplicateLayer::KeepFirst` 与 `DuplicateLayer::KeepLast` 接受不同源声明同名图层，并按源顺序保留当前瓦片中最先或最后出现的同名图层。例如，两个源都声明了 `roads`，后一个源在这张瓦片中只有 `water` 或传入空切片，`KeepLast` 会保留前一个源的 `roads`。

请声明每个源可能输出的所有图层，并确保同一输入瓦片内的图层名唯一。每张瓦片可以只包含声明中的一部分图层。使用 `KeepFirst` 或 `KeepLast` 时，合成器仅对声明重叠的源读取图层名，并检查 Protobuf 字段边界，包括嵌套的要素和属性值。保留的字段按原顺序逐字节复制，包括未知字段。

`build()` 会执行相同的重复图层校验，以及源 ID、图层和功能特性校验。样本构造与源解码返回 `SourceError`，配置返回 `BuildError`，请求合成返回 `ComposeError`。可用 `BuildError::duplicate_layer_name()` 查看重名图层及其两个来源，用 `ComposeError::source_id()` 找到解压或扫描失败的源。MVT 数据中的缺失或空图层名由 `SourceError::is_missing_layer_name()` 识别，显式配置的空名称由 `BuildError::is_empty_layer_name()` 识别。三种错误均可通过 `backtrace()` 查看回溯，并通过 `std::error::Error::source()` 查看已有的底层错误。合成返回完整结果或错误。

样本读取超出元素内存预算时会返回资源超限错误。可以使用覆盖相同图层的较小样本，或显式声明数据源的全部图层。

## 并发与内存

`build()` 完成后，`MvtComposer` 不可变，且不包含互斥锁、缓存或请求级可变状态。可将其放入 `Arc<MvtComposer>` 并在线程间共享，无需库管理的锁。合成器借用原始输入，并为非空压缩输入持有解码缓冲区。原始合成结果使用单独的缓冲区，输出压缩使用额外的编码缓冲区。图层扫描在请求内部为借用的图层名和字段建立索引。

通过 `MvtComposerBuilder::max_decoded_bytes(max)` 可限制任一合成方法单次调用的解码后总字节数，原始输入与重名策略丢弃的图层也计入上限。默认值为 `usize::MAX`，总数等于上限时允许通过，上限可设为零。合成器在解码过程中执行此限制，可用 `ComposeError::is_size_overflow()` 识别超限错误。编解码器的工作内存及编码后的输出另计。单独调用 `MvtSource::decompress` 或从样本构造源时，不使用合成器的上限。

## 示例

运行混合输入压缩示例；默认启用 `gzip`：

```text
cargo run --example mixed_sources
```

它会将原始和 gzip 输入合成，再把完整输出压缩为 gzip，并打印：

```text
compression=gzip
layers=roads,pipeline,valve,building
```

## MapLibre 浏览器示例

浏览器示例是单个纯 HTML 页面。可以添加任意数量的矢量瓦片 URL 模板、源图层名、输入压缩格式、渲染类型和颜色。Rust 后端保存这些源配置，获取对应瓦片，使用 `MvtComposer` 合成后返回一个 MVT。

请在 Git 仓库的检出目录中运行此示例，浏览器资源和离线瓦片随仓库提供：

```text
cargo run --example maplibre_server --features gzip
```

打开 `http://127.0.0.1:3010`，添加或删除瓦片源后，选择 **应用并显示**。每一行对应一个 MVT 源，并接受逗号分隔的源图层名；点、线、面要素会自动应用样式。默认配置会在 z=2 至 z=5 合并随包提供的 MapLibre 欧洲演示瓦片（包含 `geolines`、`centroids`、`countries`）和 OpenFreeMap 瓦片（包含 `landuse`），因此这些缩放级别可离线运行。示例使用异步 Reqwest 客户端并发获取已配置的瓦片源，并在本地包含 MapLibre GL JS 5.24.0。

## 测试

```text
cargo test --all-targets --all-features
cargo bench --locked --bench compose_raw
```

## 许可证

本项目可按你的选择，使用 [Apache License, Version 2.0](LICENSE-APACHE) 或 [MIT License](LICENSE-MIT) 授权。
