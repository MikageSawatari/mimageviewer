//! サイドカー内 Shape の互換・永続化の統合テスト。

use base64::Engine;
use mimageviewer::mask_db::{LineKind, MaskDb, Shape, ShapeOp, compress_mask};
use mimageviewer::sidecar::{
    SIDECAR_FILENAME, SidecarFile, SidecarMask, import_to_dbs, reconstruct_image_key,
};

fn sample_shape(op: ShapeOp) -> Shape {
    Shape::Rect {
        op,
        center: (4.0, 4.0),
        half_w: 2.0,
        half_h: 1.0,
        rotation_rad: 0.0,
    }
}

#[test]
fn sidecar_legacy_lineobject_roundtrip() {
    let temp = tempfile::tempdir().unwrap();
    let raw = compress_mask(&vec![false; 16]);
    let encoded = base64::engine::general_purpose::STANDARD.encode(raw);
    let sidecar_json = serde_json::json!({
        "version": 1,
        "items": {
            "photo.jpg": {
                "mask": {
                    "w": 4,
                    "h": 4,
                    "data": encoded,
                    "vectors": [
                        {
                            "kind": "diag",
                            "p0": [0.0, 0.0],
                            "p1": [4.0, 0.0],
                            "thickness": 2.0
                        }
                    ]
                }
            }
        }
    });
    std::fs::write(
        temp.path().join(SIDECAR_FILENAME),
        serde_json::to_string_pretty(&sidecar_json).unwrap(),
    )
    .unwrap();

    let loaded = SidecarFile::load(temp.path());
    let entry = loaded.items().get("photo.jpg").unwrap();
    let vectors = &entry.mask.as_ref().unwrap().vectors;
    assert_eq!(vectors.len(), 1);
    assert_eq!(vectors[0].op(), ShapeOp::Add);
    match vectors[0] {
        Shape::Line {
            kind: LineKind::Diagonal,
            thickness,
            ..
        } => assert_eq!(thickness, 2.0),
        other => panic!("expected legacy line, got {other:?}"),
    }
}

#[test]
fn sidecar_new_shape_roundtrip() {
    let temp = tempfile::tempdir().unwrap();
    let shape = sample_shape(ShapeOp::Add);
    let raw = compress_mask(&vec![true; 16]);

    let mut sidecar = SidecarFile::new(temp.path().to_path_buf());
    sidecar.set_mask("photo.jpg", SidecarMask::from_raw(&raw, &[shape], 4, 4));
    sidecar.flush();

    let loaded = SidecarFile::load(temp.path());
    let vectors = &loaded
        .items()
        .get("photo.jpg")
        .unwrap()
        .mask
        .as_ref()
        .unwrap()
        .vectors;
    assert_eq!(vectors, &vec![shape]);
}

#[test]
fn sidecar_subtract_persists_through_import() {
    let temp = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let mask_db = MaskDb::open_at(&db_dir.path().join("mask.db")).unwrap();
    let shape = sample_shape(ShapeOp::Subtract);
    let raw = compress_mask(&vec![false; 16]);

    let mut sidecar = SidecarFile::new(temp.path().to_path_buf());
    sidecar.set_mask("photo.jpg", SidecarMask::from_raw(&raw, &[shape], 4, 4));
    sidecar.flush();

    let loaded = SidecarFile::load(temp.path());
    let stats = import_to_dbs(temp.path(), &loaded, None, Some(&mask_db), None, None, None);
    assert_eq!(stats.imported_mask, 1);

    let key = reconstruct_image_key(temp.path(), "photo.jpg");
    let (_, shapes) = mask_db.get_full(&key, 4, 4).unwrap();
    assert_eq!(shapes, vec![shape]);
}
