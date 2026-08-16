# The far machine for the remote-pane tests.
#
# Purpose-built rather than a prebuilt sshd image, because what a remote pane
# needs from an sshd is unusual and easy to have silently switched off:
# `direct-streamlocal@openssh.com`, i.e. `ssh -L /local.sock:/remote.sock`.
# Writing our own sshd_config is the only way to *state* that requirement
# instead of hoping some image's defaults happen to satisfy it.
#
# Alpine because the binary under test is the musl build — the same one
# `cargo x remote-test --build` produces and the same one the e2e container
# runs.
FROM alpine:3.21

RUN apk add --no-cache \
    openssh-server \
    git \
    procps \
    coreutils

# Host keys, once, at build time. The tests disable host-key checking anyway,
# but sshd will not start without them.
RUN ssh-keygen -A

# uid 1000 to match the usual host user, so the bind-mounted `target/` is
# readable without any chown dance.
#
# The `sed` unlocks the account. `adduser -D` leaves a `!` in the shadow file,
# and sshd refuses a locked account outright — before it ever looks at
# authorized_keys, so the failure reads as "Permission denied (publickey)" and
# sends you hunting for a key problem that isn't there. `*` means "no password
# will ever match", which is what is wanted: key auth only.
RUN adduser -D -u 1000 -s /bin/sh test \
    && sed -i 's/^test:!/test:*/' /etc/shadow \
    && mkdir -p /home/test/.ssh \
    && chown -R test:test /home/test/.ssh \
    && chmod 700 /home/test/.ssh

COPY remote-pane-sshd_config /etc/ssh/sshd_config
COPY remote-pane-entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

EXPOSE 2222
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
