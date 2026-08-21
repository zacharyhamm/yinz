#!/bin/sh
# Provision the far machine, then become sshd.
#
# Everything here has to happen at *run* time rather than build time: the
# authorized key is generated on the host per checkout, and the zellij binary
# arrives through a bind mount that does not exist while the image is building.
set -eu

KEY_FILE="${PUBLIC_KEY_FILE:-/keys/id_ed25519.pub}"
if [ ! -f "$KEY_FILE" ]; then
    echo "no public key at $KEY_FILE — run 'cargo x remote-test --build' first" >&2
    exit 1
fi
install -o test -g test -m 600 "$KEY_FILE" /home/test/.ssh/authorized_keys

# `zellij` on the PATH, which is the entire contract the connecting side has
# with this machine ("Needs a zellij of the same version on its PATH there").
#
# A wrapper rather than a symlink, so the test seams can be exported into the
# broker's environment. sshd builds a fresh environment for a non-interactive
# command and does not pass the container's along, so this is the only place
# they can be set: `ensure` inherits them here, and the detached `serve` it
# spawns inherits them in turn.
ZELLIJ_BIN="/usr/src/zellij/$(uname -m)-unknown-linux-musl/release/zellij"
{
    echo "#!/bin/sh"
    # Only when set: an empty value is not "unset", it is a value the seam has
    # to reject and complain about.
    if [ -n "${ZELLIJ_REMOTE_ORPHAN_TIMEOUT_SECS:-}" ]; then
        echo "export ZELLIJ_REMOTE_ORPHAN_TIMEOUT_SECS=$ZELLIJ_REMOTE_ORPHAN_TIMEOUT_SECS"
    fi
    if [ -n "${ZELLIJ_REMOTE_GIT_REFRESH_SECS:-}" ]; then
        echo "export ZELLIJ_REMOTE_GIT_REFRESH_SECS=$ZELLIJ_REMOTE_GIT_REFRESH_SECS"
    fi
    echo "exec \"$ZELLIJ_BIN\" \"\$@\""
} > /usr/local/bin/zellij
chmod +x /usr/local/bin/zellij

if [ ! -x "$ZELLIJ_BIN" ]; then
    echo "warning: $ZELLIJ_BIN is missing or not executable." >&2
    echo "         Is ./target bind-mounted, and has the musl build run?" >&2
fi

exec /usr/sbin/sshd -D -e
