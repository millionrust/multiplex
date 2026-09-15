FROM debian:bookworm-slim

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        openssh-server \
        netcat-openbsd \
        tmux \
    && rm -rf /var/lib/apt/lists/*

RUN useradd -m -s /bin/sh termirust \
    && echo "termirust:termirust-pass" | chpasswd \
    && mkdir -p /home/termirust/.ssh /run/sshd \
    && chown -R termirust:termirust /home/termirust/.ssh \
    && chmod 700 /home/termirust/.ssh \
    && ssh-keygen -A \
    && printf '\nPasswordAuthentication yes\nPermitRootLogin no\nKbdInteractiveAuthentication no\nChallengeResponseAuthentication no\nPrintMotd no\nUsePAM no\nAllowTcpForwarding yes\nGatewayPorts yes\nPermitOpen any\nPermitListen any\nAllowAgentForwarding yes\nX11Forwarding no\nTrustedUserCAKeys /etc/ssh/termirust_test_ca.pub\n' >> /etc/ssh/sshd_config

EXPOSE 22

COPY entrypoint.sh /entrypoint.sh
COPY termirust-controller-test-shim.sh /usr/local/bin/termirust
COPY id_ed25519.pub /tmp/id_ed25519.pub
RUN cat /tmp/id_ed25519.pub >> /home/termirust/.ssh/authorized_keys \
    && cp /tmp/id_ed25519.pub /etc/ssh/termirust_test_ca.pub \
    && chmod 644 /etc/ssh/termirust_test_ca.pub \
    && chown termirust:termirust /home/termirust/.ssh/authorized_keys \
    && chmod 600 /home/termirust/.ssh/authorized_keys \
    && rm /tmp/id_ed25519.pub
RUN chmod +x /entrypoint.sh /usr/local/bin/termirust

CMD ["/entrypoint.sh"]
