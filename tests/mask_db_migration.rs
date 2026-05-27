//! mask.db の Shape 互換・合成順序の統合テスト。

use mimageviewer::mask_db::{
    LineKind, MaskDb, Shape, ShapeOp, rasterize_shapes_into, shapes_from_json,
};

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
fn legacy_line_object_json_reads_as_add_shape() {
    let shapes =
        shapes_from_json(r#"[{"kind":"diag","p0":[0.0,1.0],"p1":[4.0,1.0],"thickness":2.0}]"#);
    assert_eq!(shapes.len(), 1);
    assert_eq!(shapes[0].op(), ShapeOp::Add);
    match shapes[0] {
        Shape::Line {
            kind: LineKind::Diagonal,
            p0,
            p1,
            thickness,
            ..
        } => {
            assert_eq!(p0, (0.0, 1.0));
            assert_eq!(p1, (4.0, 1.0));
            assert_eq!(thickness, 2.0);
        }
        other => panic!("expected legacy line, got {other:?}"),
    }
}

#[test]
fn legacy_op_missing_reads_as_add() {
    let shapes = shapes_from_json(
        r#"[{"type":"rect","center":[4.0,4.0],"half_w":2.0,"half_h":1.0,"rotation_rad":0.0}]"#,
    );
    assert_eq!(shapes.len(), 1);
    assert_eq!(shapes[0].op(), ShapeOp::Add);
}

#[test]
fn subtract_roundtrip_through_json() {
    let src = rect(ShapeOp::Subtract, (3.0, 3.0), 2.0, 2.0);
    let json = serde_json::to_string(&vec![src]).unwrap();
    let got = shapes_from_json(&json);
    assert_eq!(got, vec![src]);
}

#[test]
fn mixed_legacy_new_array_reads_all_shapes() {
    let shapes = shapes_from_json(
        r#"[
          {"kind":"diag","p0":[0.0,0.0],"p1":[4.0,0.0],"thickness":1.0},
          {"type":"rect","op":"subtract","center":[2.0,2.0],"half_w":1.0,"half_h":1.0,"rotation_rad":0.0}
        ]"#,
    );
    assert_eq!(shapes.len(), 2);
    assert_eq!(shapes[0].op(), ShapeOp::Add);
    assert_eq!(shapes[1].op(), ShapeOp::Subtract);
}

#[test]
fn mask_db_set_get_with_subtract() {
    let temp = tempfile::tempdir().unwrap();
    let db = MaskDb::open_at(&temp.path().join("mask.db")).unwrap();
    let mask = vec![false; 16];
    let shapes = vec![rect(ShapeOp::Subtract, (2.0, 2.0), 1.0, 1.0)];

    db.set("image.png", &mask, &shapes, 4, 4).unwrap();
    let (got_mask, got_shapes) = db.get_full("image.png", 4, 4).unwrap();

    assert!(!got_mask.iter().any(|&v| v));
    assert_eq!(got_shapes, shapes);
}

#[test]
fn mask_db_delete_when_empty() {
    let temp = tempfile::tempdir().unwrap();
    let db = MaskDb::open_at(&temp.path().join("mask.db")).unwrap();
    let mask = vec![true; 16];

    db.set("image.png", &mask, &[], 4, 4).unwrap();
    assert!(db.get_full("image.png", 4, 4).is_some());

    let empty = vec![false; 16];
    db.set("image.png", &empty, &[], 4, 4).unwrap();
    assert!(db.get_full("image.png", 4, 4).is_none());
}

#[test]
fn rasterize_shapes_apply_op_order() {
    let w = 10;
    let h = 10;
    let mut mask = vec![false; w * h];
    let shapes = vec![
        rect(ShapeOp::Add, (5.0, 5.0), 5.0, 5.0),
        rect(ShapeOp::Subtract, (5.0, 5.0), 2.0, 2.0),
        rect(ShapeOp::Add, (5.0, 5.0), 0.75, 0.75),
    ];

    rasterize_shapes_into(&mut mask, &shapes, w, h);

    assert!(mask[0], "first Add should paint the outer area");
    assert!(
        !mask[3 + 4 * w],
        "Subtract should remove the inner area after the first Add"
    );
    assert!(
        mask[2 + 2 * w],
        "final Add should paint back over the subtraction"
    );
}
