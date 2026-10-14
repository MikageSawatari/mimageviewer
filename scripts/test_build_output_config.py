"""Fixed Cargo output regression tests, using only fake config and environment."""
from pathlib import Path
import shutil
import unittest
import uuid
from check_build_output_config import OutputOverride, check_output, OUTPUT_ENV


class FixedOutputTests(unittest.TestCase):
    def setUp(self):
        # Python 3.13's private mkdtemp ACL excludes a Windows sandbox SID.
        # Inherit the workspace ACL for these disposable, non-sensitive files.
        parent = Path(__file__).resolve().parents[1] / "target"
        self.root = parent / ("miv-output-test-" + uuid.uuid4().hex)
        self.root.mkdir(parents=True)
        self.assertEqual(self.root.resolve().parent, parent.resolve())
        self.addCleanup(shutil.rmtree, self.root)
        self.start = self.root / "project" / "nested"
        self.start.mkdir(parents=True)
        self.env = {"CARGO_HOME": str(self.root / "cargo-home")}

    def config(self, directory, text, name="config.toml"):
        directory.mkdir(parents=True, exist_ok=True)
        path = directory / name
        path.write_text(text, encoding="utf-8")
        return path

    def reject(self, env=None, args=()):
        with self.assertRaises(OutputOverride):
            check_output(self.start, self.env if env is None else env, args)

    def test_environment_overrides_even_same_directory(self):
        for key in OUTPUT_ENV:
            for value in ("elsewhere", "target", ""):
                with self.subTest(key=key, value=value):
                    self.reject({**self.env, key: value})

    def test_cli_output_overrides(self):
        for option in ("--target-dir", "--build-dir", "--target", "--profile", "--artifact-dir", "--out-dir", "--manifest-path", "-C"):
            for args in ([option, "elsewhere"], [option + "=elsewhere"]):
                with self.subTest(args=args):
                    self.reject(args=args)
        self.reject(args=["-Zbuild-dir"])

    def test_build_keys_all_discovery_locations_and_forms(self):
        locations = [self.start / ".cargo", self.root / "project" / ".cargo", Path(self.env["CARGO_HOME"])]
        for directory in locations:
            for name in ("config.toml", "config"):
                for key in ("target-dir", "build-dir", "target", "artifact-dir", "out-dir"):
                    for text in (f'[build]\n{key}="other"', f'build."{key}"="other"', f'build={{"{key}"="other"}}'):
                        with self.subTest(directory=directory, name=name, text=text):
                            path = self.config(directory, text, name)
                            self.reject()
                            path.unlink()

    def test_quoted_and_escaped_keys(self):
        self.config(self.start / ".cargo", '["build"]\n"target\\u002ddir"="other"')
        self.reject()

    def test_env_table_output_override(self):
        for key in OUTPUT_ENV:
            path = self.config(self.start / ".cargo", f'[env]\n{key}={{value="other", force=true}}')
            self.reject()
            path.unlink()

    def test_cli_config_inline_and_file(self):
        self.reject(args=["--config", 'build.target-dir="other"'])
        self.reject(args=["--config=build.target=\"x86_64-pc-windows-msvc\""])
        path = self.config(self.root / "extra", '[build]\nbuild-dir="other"')
        self.reject(args=["--config", str(path)])

    def test_includes_recursive_and_optional(self):
        directory = self.start / ".cargo"
        self.config(directory, 'include=[{path="missing.toml",optional=true}, "one.toml"]')
        self.config(directory, 'include=["two.toml"]', "one.toml")
        self.config(directory, '[build]\ntarget-dir="other"', "two.toml")
        self.reject()

    def test_legacy_config_precedence(self):
        self.config(self.start / ".cargo", '[build]\ntarget-dir="other"')
        self.config(self.start / ".cargo", '[build]\njobs=1', "config")
        check_output(self.start, self.env, [])

    def test_output_rustflags_environment_and_config(self):
        for flags in ("--out-dir other", "-o other", "-oother", "-C extra-filename=other", "--emit=link=other.exe", "--emit link=other.exe"):
            for key in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS", "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS"):
                self.reject({**self.env, key: flags})
            path = self.config(self.start / ".cargo", f'[build]\nrustflags="{flags}"')
            self.reject()
            path.unlink()
        self.reject(args=["--config", 'target.x86_64-pc-windows-msvc.rustflags=["--out-dir", "other"]'])

    def test_normal_linker_flags_and_arguments(self):
        self.config(self.start / ".cargo", '[target.x86_64-pc-windows-msvc]\nlinker="rust-lld.exe"\nrustflags=["-C", "target-feature=+crt-static"]\n[env]\nFFMPEG_DIR={value="vendor/ffmpeg",relative=true}\n[build]\njobs=2')
        check_output(self.start, self.env, ["--features", "foo", "--locked", "--config", "build.jobs=1"])


if __name__ == "__main__":
    unittest.main()
