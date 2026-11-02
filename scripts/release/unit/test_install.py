"""Exercise the downloadable installer without building the workspace."""
from pathlib import Path
import os
import shutil
import subprocess
import tempfile
import unittest

INSTALLER = Path(__file__).resolve().parents[1] / "install.sh"


class InstallTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bundle = self.root / "release with spaces"
        self.plugins = self.bundle / "plugins/builtins"
        self.plugins.mkdir(parents=True)
        (self.plugins / "nested").mkdir()
        (self.plugins / "nested/plugin.scm").write_text("new plugin")
        shutil.copy2(INSTALLER, self.bundle / "install.sh")
        self.binary = self.bundle / "yinz"
        self.binary.write_text("#!/bin/sh\necho 'yinz fixture'\n")
        self.binary.chmod(0o755)
        self.home = self.root / "home"
        self.home.mkdir()
        self.env = dict(os.environ, HOME=str(self.home))
        self.env.pop("XDG_DATA_HOME", None)
        self.bin_dir = self.root / "bin with spaces"
        self.data_dir = self.root / "data with spaces"

    def install(self, *args, success=True):
        result = subprocess.run([str(self.bundle / "install.sh"), *args],
                                env=self.env, text=True, capture_output=True)
        self.assertEqual(result.returncode == 0, success, result.stderr)
        return result

    def explicit(self, success=True):
        return self.install("--bin-dir", str(self.bin_dir), "--data-dir", str(self.data_dir),
                            success=success)

    def test_reinstall_preserves_unrelated_files(self):
        self.explicit()
        plugin = self.data_dir / "plugins/builtins/nested/plugin.scm"
        plugin.write_text("old plugin")
        unrelated = plugin.parent / "user.scm"
        unrelated.write_text("keep me")
        self.explicit()
        self.assertEqual(plugin.read_text(), "new plugin")
        self.assertEqual(unrelated.read_text(), "keep me")
        self.assertEqual((self.bin_dir / "yinz").read_bytes(), self.binary.read_bytes())
        self.assertTrue(os.access(self.bin_dir / "yinz", os.X_OK))

    def test_missing_plugins_does_not_replace_binary(self):
        self.explicit()
        self.binary.write_text("replacement")
        shutil.rmtree(self.plugins)
        self.explicit(success=False)
        self.assertNotEqual((self.bin_dir / "yinz").read_bytes(), self.binary.read_bytes())

    def test_rejects_symlinked_builtin_directory(self):
        outside = self.root / "outside"
        outside.mkdir()
        (self.data_dir / "plugins").mkdir(parents=True)
        (self.data_dir / "plugins/builtins").symlink_to(outside, target_is_directory=True)
        self.explicit(success=False)
        self.assertEqual(list(outside.iterdir()), [])

    def test_platform_defaults(self):
        # Mock uname to check both platforms on either native test runner.
        commands = self.root / "commands"
        commands.mkdir()
        uname = commands / "uname"
        self.env["PATH"] = str(commands) + os.pathsep + self.env["PATH"]
        for system, suffix in [
            ("Linux", ".local/share/yinz"),
            ("Darwin", "Library/Application Support/org.Yinz-Contributors.Yinz"),
        ]:
            with self.subTest(system=system):
                uname.write_text(f"#!/bin/sh\necho {system}\n")
                uname.chmod(0o755)
                self.install()
                self.assertTrue((self.home / suffix / "plugins/builtins/nested/plugin.scm").exists())
        uname.write_text("#!/bin/sh\necho Linux\n")
        self.env["XDG_DATA_HOME"] = str(self.root / "xdg")
        self.install()
        self.assertTrue((self.root / "xdg/yinz/plugins/builtins/nested/plugin.scm").exists())

    def test_argument_errors(self):
        self.install("--bin-dir", success=False)
        self.install("--data-dir", "", success=False)
        self.install("--unknown", success=False)


if __name__ == "__main__":
    unittest.main()
