use std::hint::black_box;
use std::time::Instant;

use composite_mvt::{DuplicateLayer, MvtComposer, MvtSource};
use fast_mvt::{MvtCoord, MvtGeomType, MvtTileBuilder};

fn tile(name: &str, points: i32) -> Vec<u8> {
    let mut layer = MvtTileBuilder::new().layer(name).unwrap();
    for x in 0..points {
        let mut feature = layer.feature_of(MvtGeomType::Point);
        feature.points([MvtCoord { x, y: 0 }]).unwrap();
        layer = feature.end();
    }
    layer.end().encode()
}

fn measure(policy: DuplicateLayer, sources: usize, points: i32, iterations: u32) {
    let mut builder = MvtComposer::builder().duplicate_layer(policy);
    let mut tiles = Vec::new();
    for index in 0..sources {
        let name = format!("layer-{index}");
        tiles.push(tile(&name, points));
        builder = builder.add_source(MvtSource::new(name.clone()).with_layers([name]));
    }
    let composer = builder.build().unwrap();
    let inputs: Vec<_> = tiles.iter().map(Vec::as_slice).collect();
    let expected = tiles.concat();
    assert_eq!(composer.compose(&inputs).unwrap().as_ref(), expected);

    let compose = || {
        black_box(composer.compose(black_box(&inputs)).unwrap());
    };
    for _ in 0..iterations / 10 {
        compose();
    }
    let start = Instant::now();
    for _ in 0..iterations {
        compose();
    }
    let ns_per_tile = start.elapsed().as_secs_f64() * 1e9 / f64::from(iterations);
    println!(
        "{policy:?}\t{sources}\t{points}\t{}\t{iterations}\t{ns_per_tile:.2}",
        expected.len()
    );
}

fn main() {
    if !std::env::args().any(|arg| arg == "--bench") {
        return;
    }
    println!("policy\tsources\tpoints/source\tbytes/tile\titerations\tns/tile");
    let filter = std::env::args().skip(1).find(|arg| !arg.starts_with('-'));
    for policy in [
        DuplicateLayer::Error,
        DuplicateLayer::KeepFirst,
        DuplicateLayer::KeepLast,
    ] {
        if filter
            .as_ref()
            .is_some_and(|name| name != &format!("{policy:?}"))
        {
            continue;
        }
        measure(policy, 2, 1, 1_000_000);
        measure(policy, 4, 32, 400_000);
        measure(policy, 4, 2048, 180_000);
        measure(policy, 12, 32, 350_000);
    }
}
