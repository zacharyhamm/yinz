"""Stream only the generic workflow to the configured Mac, then atomically publish its signature."""
import os
from pathlib import Path
import re
import subprocess
import tempfile

from shortcut import workflow


def main():
    host = os.environ["SHORTCUT_SIGNING_HOST"]
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.@-]*", host):
        raise ValueError("Expected an SSH host alias in SHORTCUT_SIGNING_HOST (vault SIRI_SIGNING_HOST)")
    remote = """
set -eu
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
cat > "$dir/Talk to Yinz.shortcut"
/usr/bin/shortcuts sign --mode anyone --input "$dir/Talk to Yinz.shortcut" --output "$dir/signed.shortcut" >&2
cat "$dir/signed.shortcut"
"""
    signed = subprocess.run(["ssh", "-T", "-o", "BatchMode=yes", "--", host, remote],
                            input=workflow(), stdout=subprocess.PIPE, check=True, timeout=120).stdout
    if not signed.startswith(b"AEA1"):
        raise ValueError("The signer did not return an Apple Encrypted Archive")
    root = Path(__file__).resolve().parents[2]
    out = root / "yinz-steel-builtins/assets/plugins/web/static/talk-to-yinz.shortcut"
    with tempfile.NamedTemporaryFile(dir=out.parent, delete=False) as f:
        tmp = Path(f.name)
        try:
            f.write(signed)
            f.flush()
            os.fchmod(f.fileno(), 0o644)
            tmp.replace(out)
        finally:
            tmp.unlink(missing_ok=True)
    print(f"Signed {out.name} ({len(signed)} bytes)")


if __name__ == "__main__":
    main()
