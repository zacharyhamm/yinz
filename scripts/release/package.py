#!/usr/bin/env python3
"""Package and smoke-test a native release; run from the repository root."""
import argparse
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import tempfile

TARGETS = {
    "x86_64-unknown-linux-gnu": ("Linux", "x86_64"),
    "aarch64-unknown-linux-gnu": ("Linux", "aarch64"),
    "aarch64-apple-darwin": ("Darwin", "arm64"),
}


def run(*args):
    return subprocess.check_output(args, text=True).strip()


def package(target):
    if (platform.system(), platform.machine()) != TARGETS[target]:
        raise RuntimeError("Release target does not match native runner")
    binary = Path("target/release/yinz").resolve()
    binary_type = run("file", str(binary))
    expected_arch = {"x86_64": "x86-64", "aarch64": "ARM aarch64", "arm64": "arm64"}[platform.machine()]
    if expected_arch not in binary_type:
        raise RuntimeError(f"Incorrect executable architecture: {binary_type}")
    print(binary_type)
    if platform.system() == "Darwin":
        dependencies = run("otool", "-L", str(binary))
        for line in dependencies.splitlines()[1:]:
            dependency = line.strip().split(" (", 1)[0]
            if not dependency.startswith(("/usr/lib/", "/System/Library/")):
                raise RuntimeError(f"Non-system macOS dependency: {dependency}")
    else:
        dependencies = run("ldd", str(binary))
        if "not found" in dependencies:
            raise RuntimeError(dependencies)
    print(dependencies)
    output = Path("dist")
    output.mkdir(exist_ok=True)
    archive = output / f"yinz-{target}.tar.gz"
    with tempfile.TemporaryDirectory() as temp:
        root = Path(temp)
        stage = root / "stage"
        stage.mkdir()
        shutil.copy2(binary, stage / "yinz")
        shutil.copytree("yinz-steel-builtins/assets/plugins", stage / "plugins/builtins")
        shutil.copy2("scripts/release/install.sh", stage / "install.sh")
        shutil.copy2("LICENSE", stage / "LICENSE")
        shutil.copy2("README.md", stage / "README.md")
        (stage / "DEPENDENCIES.txt").write_text(dependencies + "\n")
        with tarfile.open(archive, "w:gz") as bundle:
            for path in sorted(stage.iterdir()):
                bundle.add(path, arcname=path.name)
        extracted = root / "extracted"
        with tarfile.open(archive) as bundle:
            bundle.extractall(extracted, filter="data")
        destination = root / "installed with spaces"
        subprocess.run([str(extracted / "install.sh"), "--bin-dir", str(destination / "bin"),
                        "--data-dir", str(destination / "data")], check=True)
        for source in (stage / "plugins/builtins").rglob("*"):
            if source.is_file():
                installed = destination / "data/plugins/builtins" / source.relative_to(stage / "plugins/builtins")
                if installed.read_bytes() != source.read_bytes():
                    raise RuntimeError(f"Incorrect installed plugin: {installed}")
        version = run(str(destination / "bin/yinz"), "--version")
        expected = os.environ.get("RELEASE_TAG", "").removeprefix("v")
        if expected and version != f"yinz {expected}":
            raise RuntimeError(f"Unexpected version: {version}")
        print(version)
    print(archive)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target", choices=TARGETS)
    package(parser.parse_args().target)
