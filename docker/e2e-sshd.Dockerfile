# Password-authenticated SSH fixture for the older terminal E2E suite.
FROM debian:trixie-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        openssh-server bash busybox procps psmisc coreutils ncurses-bin \
        ncurses-term libgcc-s1 libstdc++6 libssl3t64 zlib1g \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 1000 --shell /bin/bash test \
    && echo 'test:test' | chpasswd \
    && printf '%s\n' 'PS1="$ "' > /home/test/.bashrc \
    && chown test:test /home/test/.bashrc \
    && ln -s /bin/busybox /usr/bin/vi \
    && mkdir -p /run/sshd \
    && ssh-keygen -A \
    && printf 'Port 2222\nPasswordAuthentication yes\nPermitRootLogin no\nUsePAM yes\nPrintMotd no\n' \
        > /etc/ssh/sshd_config

EXPOSE 2222
CMD ["/usr/sbin/sshd", "-D", "-e"]
