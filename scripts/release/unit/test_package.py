"""Cross packaging must inspect the target without executing its binary."""
import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / "package.py"
spec = importlib.util.spec_from_file_location("release_package", SCRIPT)
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)


class CrossPackageTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        previous = Path.cwd()
        self.addCleanup(os.chdir, previous)
        self.root = Path(self.temp.name)
        os.chdir(self.root)
        self.target = "aarch64-unknown-linux-gnu"
        binary = self.root / "target" / self.target / "release/yinz"
        binary.parent.mkdir(parents=True)
        # Not executable by the host: packaging must never try running it.
        binary.write_bytes(b"target executable fixture")
        binary.chmod(0o755)
        plugins = self.root / "yinz-steel-builtins/assets/plugins"
        plugins.mkdir(parents=True)
        (plugins / "fixture.scm").write_text("(define source-commit 'fixture)")
        scripts = self.root / "scripts/release"
        scripts.mkdir(parents=True)
        shutil.copy2(SCRIPT.with_name("install.sh"), scripts / "install.sh")
        for name in ["LICENSE", "README.md"]:
            (self.root / name).write_text(name)

    def test_packages_foreign_binary_and_matching_plugins(self):
        with patch.object(package, "run", side_effect=["ELF 64-bit ARM aarch64", "Dynamic section"]) as run:
            package.package(self.target, cross=True)
        self.assertEqual([call.args[0] for call in run.call_args_list], ["file", "readelf"])
        with tarfile.open(f"dist/yinz-{self.target}.tar.gz") as archive:
            self.assertEqual(archive.extractfile("yinz").read(), b"target executable fixture")
            self.assertEqual(archive.extractfile("plugins/builtins/fixture.scm").read(),
                             b"(define source-commit 'fixture)")

    def test_rejects_wrong_architecture_before_packaging(self):
        with patch.object(package, "run", return_value="ELF 64-bit x86-64"):
            with self.assertRaisesRegex(RuntimeError, "Incorrect executable architecture"):
                package.package(self.target, cross=True)
        self.assertFalse(Path("dist").exists())

    def test_rejects_non_linux_cross_target(self):
        with self.assertRaisesRegex(RuntimeError, "only Linux"):
            package.package("aarch64-apple-darwin", cross=True)

    def test_rejects_non_elf_linux_binary(self):
        with patch.object(package, "run", return_value="Mach-O 64-bit ARM aarch64"):
            with self.assertRaisesRegex(RuntimeError, "Incorrect executable architecture"):
                package.package(self.target, cross=True)
        self.assertFalse(Path("dist").exists())

    def test_failed_installer_preserves_previous_archive(self):
        output = Path("dist")
        output.mkdir()
        archive = output / f"yinz-{self.target}.tar.gz"
        archive.write_bytes(b"previous validated archive")
        with patch.object(package, "run", side_effect=["ELF 64-bit ARM aarch64", "Dynamic section"]):
            with patch.object(package.subprocess, "run", side_effect=subprocess.CalledProcessError(1, "install.sh")):
                with self.assertRaises(subprocess.CalledProcessError):
                    package.package(self.target, cross=True)
        self.assertEqual(archive.read_bytes(), b"previous validated archive")
        self.assertEqual(list(output.iterdir()), [archive])
