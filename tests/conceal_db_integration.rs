//! conceal.db のマスク・スロット・リサイズの統合テスト。

use mimageviewer::conceal_db::ConcealDb;
use mimageviewer::mask_db::{Shape, ShapeOp};

fn rect(op: ShapeOp, center: (f32, f32), half_w: f32, half_h: f32) -> Shape {
    Shape::Rect {
        op,
        center,
        half_w,
        half_h,
        rotation_rad: 0.0,
    }
}

#[test]
fn conceal_db_roundtrip_basic() {
    let temp = tempfile::tempdir().unwrap();
    let db = ConcealDb::open_at(&temp.path().join("conceal.db")).unwrap();
    let mut mask = vec![false; 25];
    mask[12] = true;
    let shapes = vec![
        rect(ShapeOp::Add, (2.0, 2.0), 2.0, 2.0),
        rect(ShapeOp::Subtract, (2.0, 2.0), 0.5, 0.5),
    ];

    db.set("image.png", &mask, &shapes, 5, 5).unwrap();
    let (got_mask, got_shapes) = db.get_full("image.png", 5, 5).unwrap();

    assert_eq!(got_mask, mask);
    assert_eq!(got_shapes, shapes);
}

#[test]
fn conceal_db_mask_slot_save_load() {
    let temp = tempfile::tempdir().unwrap();
    let db = ConcealDb::open_at(&temp.path().join("conceal.db")).unwrap();
    let mask = vec![true, false, false, true];
    let shapes = vec![rect(ShapeOp::Add, (1.0, 1.0), 0.5, 0.5)];

    db.set_slot(1, &mask, &shapes, 2, 2).unwrap();
    assert_eq!(db.slot_size(1), Some((2, 2)));

    let (got_mask, got_shapes) = db.get_slot_full(1, 2, 2).unwrap();
    assert_eq!(got_mask, mask);
    assert_eq!(got_shapes, shapes);
}

#[test]
fn conceal_db_delete_on_empty() {
    let temp = tempfile::tempdir().unwrap();
    let db = ConcealDb::open_at(&temp.path().join("conceal.db")).unwrap();
    let full = vec![true; 16];
    db.set("image.png", &full, &[], 4, 4).unwrap();
    assert!(db.get_full("image.png", 4, 4).is_some());

    let empty = vec![false; 16];
    db.set("image.png", &empty, &[], 4, 4).unwrap();
    assert!(db.get_full("image.png", 4, 4).is_none());
}

#[test]
fn conceal_db_pdf_zoom_resize() {
    let temp = tempfile::tempdir().unwrap();
    let db = ConcealDb::open_at(&temp.path().join("conceal.db")).unwrap();
    let mut mask = vec![false; 4];
    mask[0] = true;
    let shapes = vec![rect(ShapeOp::Add, (1.0, 1.0), 0.5, 0.5)];

    db.set("doc.pdf::page_0", &mask, &shapes, 2, 2).unwrap();
    let (resized_mask, resized_shapes) = db.get_full("doc.pdf::page_0", 4, 4).unwrap();

    assert_eq!(resized_mask.len(), 16);
    assert!(
        resized_mask[0] && resized_mask[1] && resized_mask[4] && resized_mask[5],
        "nearest-neighbor resize should expand the top-left source pixel"
    );
    match resized_shapes[0] {
        Shape::Rect {
            center,
            half_w,
            half_h,
            ..
        } => {
            assert_eq!(center, (2.0, 2.0));
            assert_eq!(half_w, 1.0);
            assert_eq!(half_h, 1.0);
        }
        other => panic!("expected rect after resize, got {other:?}"),
    }
}
