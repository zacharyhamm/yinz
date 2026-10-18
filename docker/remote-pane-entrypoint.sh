#!/bin/sh
# Provision the far machine, then become sshd.
#
# Everything here has to happen at *run* time rather than build time: the
# authorized key is generated on the host per checkout, and the yinz binary
# arrives through a bind mount that does not exist while the image is building.
set -eu

KEY_FILE="${PUBLIC_KEY_FILE:-/keys/id_ed25519.pub}"
if [ ! -f "$KEY_FILE" ]; then
    echo "no public key at $KEY_FILE — run 'cargo x remote-test --build' first" >&2
    exit 1
fi
install -o test -g test -m 600 "$KEY_FILE" /home/test/.ssh/authorized_keys

# `yinz` on the PATH, which is the entire contract the connecting side has
# with this machine ("Needs a yinz of the same version on its PATH there").
#
# A wrapper rather than a symlink, so the test seams can be exported into the
# broker's environment. sshd builds a fresh environment for a non-interactive
# command and does not pass the container's along, so this is the only place
# they can be set: `ensure` inherits them here, and the detached `serve` it
# spawns inherits them in turn.
YINZ_BIN="/usr/src/yinz/remote-test/yinz"
mkdir -p /home/test/.local/share/yinz/plugins/builtins
cp -R /usr/src/yinz/remote-test/data/plugins/builtins/. /home/test/.local/share/yinz/plugins/builtins/
chown -R test:test /home/test/.local/share/yinz
{
    echo "#!/bin/sh"
    # Only when set: an empty value is not "unset", it is a value the seam has
    # to reject and complain about.
    if [ -n "${YINZ_REMOTE_ORPHAN_TIMEOUT_SECS:-}" ]; then
        echo "export YINZ_REMOTE_ORPHAN_TIMEOUT_SECS=$YINZ_REMOTE_ORPHAN_TIMEOUT_SECS"
    fi
    if [ -n "${YINZ_REMOTE_GIT_REFRESH_SECS:-}" ]; then
        echo "export YINZ_REMOTE_GIT_REFRESH_SECS=$YINZ_REMOTE_GIT_REFRESH_SECS"
    fi
    echo "exec \"$YINZ_BIN\" \"\$@\""
} > /usr/local/bin/yinz
chmod +x /usr/local/bin/yinz

if [ ! -x "$YINZ_BIN" ]; then
    echo "error: $YINZ_BIN is missing or not executable." >&2
    echo "Run 'cargo x remote-test --build' (or '--build --native' on Linux)." >&2
    exit 1
fi

exec /usr/sbin/sshd -D -e
