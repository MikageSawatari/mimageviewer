"""Small structural checks; no downloaded model or GPU is needed."""
import hashlib
import json
import shutil
import unittest
import uuid
from contextlib import contextmanager
from pathlib import Path

import onnx
from onnx import TensorProto, helper

from build_depth_addon import INPUT_SHAPES, MODEL_NAME, build


@contextmanager
def workspace_temp():
    # Python 3.13's Windows mode-0700 tempfile ACL excludes the managed runner.
    # An atomically created UUID directory inherits this workspace's normal ACL.
    test_root = (Path(__file__).parents[2] / "target/depth-builder-tests").resolve()
    test_root.mkdir(parents=True, exist_ok=True)
    root = test_root / uuid.uuid4().hex
    root.mkdir()
    try:
        yield root
    finally:
        if root.resolve().parent != test_root or root.is_symlink():
            raise RuntimeError("Unexpected temporary test directory")
        shutil.rmtree(root)


class BuildDepthAddonTests(unittest.TestCase):
    def test_fixed_fp32_io_embedded_fp16_and_hashed_assets(self):
        with workspace_temp() as root:
            inputs = [helper.make_tensor_value_info(name, TensorProto.FLOAT,
                      ["num_views" if i == 1 else "height" if i == 3 else "width" if i == 4 else size
                       for i, size in enumerate(shape)])
                      for name, shape in INPUT_SHAPES.items()]
            output = helper.make_tensor_value_info("depth", TensorProto.FLOAT, [1, "num_views", 3, "height", "width"])
            weight = helper.make_tensor("weight", TensorProto.FLOAT, [1], [0.5])
            graph = helper.make_graph([helper.make_node("Add", ["image", "weight"], ["depth"])],
                                      "test", inputs, [output], [weight])
            source = root / "source.onnx"
            onnx.save(helper.make_model(graph, opset_imports=[helper.make_opsetid("", 17)]), source)
            index = build(source, root / "out")
            model = onnx.load(root / "out" / MODEL_NAME)
            onnx.checker.check_model(model)
            for value in model.graph.input:
                self.assertEqual(value.type.tensor_type.elem_type, TensorProto.FLOAT)
                self.assertEqual(tuple(d.dim_value for d in value.type.tensor_type.shape.dim), INPUT_SHAPES[value.name])
                self.assertTrue(all(not d.dim_param for d in value.type.tensor_type.shape.dim))
            self.assertEqual(model.graph.output[0].type.tensor_type.elem_type, TensorProto.FLOAT)
            self.assertTrue(any(t.data_type == TensorProto.FLOAT16 for t in model.graph.initializer))
            self.assertTrue(all(not t.external_data for t in model.graph.initializer))
            self.assertEqual(json.loads((root / "out/depth-model-index.json").read_text()), index)
            for entry in index["files"]:
                content = (root / "out" / entry["name"]).read_bytes()
                self.assertEqual(len(content), entry["bytes"])
                self.assertEqual(hashlib.sha256(content).hexdigest(), entry["sha256"])
                self.assertEqual(entry["license"], "Apache-2.0")
            self.assertIn("fp16", (root / "out/NOTICE").read_text())
            self.assertIn("504 x 504", (root / "out/NOTICE").read_text())
            self.assertGreater((root / "out/LICENSE").stat().st_size, 10000)

    def test_other_export_is_rejected(self):
        with workspace_temp() as root:
            graph = helper.make_graph([], "wrong-model", [], [])
            source = root / "wrong.onnx"
            onnx.save(helper.make_model(graph), source)
            with self.assertRaisesRegex(ValueError, "SpatialHub"):
                build(source, root / "out")


if __name__ == "__main__":
    unittest.main()
