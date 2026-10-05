use super::*;
use sha2::{Digest, Sha256};

fn geom_digest(geom: &Geom, hash: &mut Sha256) {
    fn points(points: &[(f64, f64)], hash: &mut Sha256) {
        hash.update((points.len() as u64).to_le_bytes());
        // Native hypot can differ in the last bit. Picodegrees are one million times
        // finer than the stored microdegree coordinates.
        for &(x, y) in points {
            assert!(x.is_finite() && y.is_finite());
            hash.update(((x * 1e12).round_ties_even() as i64).to_le_bytes());
            hash.update(((y * 1e12).round_ties_even() as i64).to_le_bytes());
        }
    }
    match geom {
        Geom::Empty => hash.update([0]),
        Geom::Line(line) => {
            hash.update([1]);
            points(line, hash);
        }
        Geom::Polygon { exterior, interiors } => {
            hash.update([2]);
            points(exterior, hash);
            hash.update((interiors.len() as u64).to_le_bytes());
            for ring in interiors {
                points(ring, hash);
            }
        }
        Geom::Multi(parts) => {
            hash.update([3]);
            hash.update((parts.len() as u64).to_le_bytes());
            for part in parts {
                geom_digest(part, hash);
            }
        }
    }
}

fn digest(features: &[(u8, Geom)], labels: &[u8], grid: (f64, f64, usize, usize, f64)) -> Vec<u8> {
    let mut hash = Sha256::new();
    for value in [grid.0.to_bits(), grid.1.to_bits(), grid.2 as u64, grid.3 as u64, grid.4.to_bits()] {
        hash.update(value.to_le_bytes());
    }
    hash.update((features.len() as u64).to_le_bytes());
    for (style, geom) in features {
        hash.update([*style]);
        geom_digest(geom, &mut hash);
    }
    hash.update(labels);
    hash.finalize().to_vec()
}

fn compare(
    features: &[(u8, Geom)],
    scheme: &SemanticScheme,
    bbox: (i64, i64, i64, i64),
    scales: &[f64],
    expected: &[&str],
) {
    assert_eq!(scales.len(), expected.len());
    let mut prior = None;
    for (&mpp, expected) in scales.iter().zip(expected) {
        let level = build_semantic_lod(features, scheme, bbox, mpp, prior.as_ref(), &Progress::silent()).unwrap();
        let grid = &level.labels.grid;
        let actual =
            digest(&level.features, &level.labels.labels, (grid.left, grid.bottom, grid.cols, grid.rows, grid.cell_m));
        let actual: String = actual.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(actual, *expected, "semantic rung {mpp}");
        prior = Some(level.labels);
    }
}

#[test]
fn semantic_ladders_preserve_geometry_and_labels() {
    let mut scheme = SemanticScheme::new();
    for (style, class) in [
        (1, SemanticClass::Farmland),
        (2, SemanticClass::Forest),
        (3, SemanticClass::Urban),
        (4, SemanticClass::Water),
        (5, SemanticClass::Grass),
        (6, SemanticClass::Rock),
        (7, SemanticClass::Ice),
    ] {
        scheme.insert(style, class);
    }
    let square = |x: f64, y: f64, size: f64| vec![(x, y), (x + size, y), (x + size, y + size), (x, y + size), (x, y)];
    let inputs = vec![
        (1, Geom::Polygon { exterior: square(0.001, 0.001, 0.006), interiors: vec![] }),
        (2, Geom::Polygon { exterior: square(0.003, 0.003, 0.007), interiors: vec![square(0.005, 0.005, 0.001)] }),
        (3, Geom::Polygon { exterior: square(0.005, 0.005, 0.001), interiors: vec![] }),
        (4, Geom::Polygon { exterior: square(0.008, 0.001, 0.004), interiors: vec![square(0.009, 0.002, 0.001)] }),
        (5, Geom::Polygon { exterior: square(0.043, 0.045, 0.013), interiors: vec![] }),
        (6, Geom::Polygon { exterior: square(0.053, 0.045, 0.013), interiors: vec![] }),
        (7, Geom::Polygon { exterior: square(0.063, 0.045, 0.013), interiors: vec![] }),
    ];
    compare(
        &inputs,
        &scheme,
        (0, 0, 15_000, 15_000),
        &[20.0, 35.0, 50.0, 90.0, 130.0],
        &[
            "1842b065a9b8449a5b7f1984e5b7bab36ef45f6059f425b30d454124ff6ffd5e",
            "63de17805b625ea151d31374d706762acb51e2758e8ddaa13889d1523b896a42",
            "591cd83f02d53dd696550d5539638aeec9975a08404dc04fbace114723ee50cd",
            "46c2d3b016afbbfea011e1ce8b834ecb2c12f537593d8124328da75ef823ce8e",
            "e8edf1baa3d24a674d1b166fef96c78d2486466a71e804beda7ff30f370e127d",
        ],
    );
    compare(
        &inputs,
        &scheme,
        (0, 0, 120_000, 120_000),
        &[20.0, 35.0, 50.0],
        &[
            "ec90ebe61fe74cf6d2476c5f512d78a1c2850201012377cd35e5da491e23cee5",
            "6f3f82f298d5dd99255dc025a803bcb23150212bb176fcb00f2b190425ff8356",
            "3b6121ed9967c9da3756afb62358f151ef3effccfc41c8bed97f36443a57e34c",
        ],
    );
    compare(
        &[],
        &scheme,
        (0, 0, 15_000, 15_000),
        &[20.0, 35.0],
        &[
            "758071e4f1662b75b18894a6c04f0c3dde8315552acc0d514516ee0fdf5acb99",
            "0c33bcad355783a3b1a558a459c18a54026b2a770dbc162c15f2f6276d4b108e",
        ],
    );
}
