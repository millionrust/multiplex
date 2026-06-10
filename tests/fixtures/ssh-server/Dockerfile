FROM debian:bookworm-slim

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        openssh-server \
        netcat-openbsd \
    && rm -rf /var/lib/apt/lists/*

RUN useradd -m -s /bin/sh tshell \
    && echo "tshell:tshell-pass" | chpasswd \
    && mkdir -p /home/tshell/.ssh /run/sshd \
    && chown -R tshell:tshell /home/tshell/.ssh \
    && chmod 700 /home/tshell/.ssh \
    && ssh-keygen -A \
    && printf '\nPasswordAuthentication yes\nPermitRootLogin no\nKbdInteractiveAuthentication no\nChallengeResponseAuthentication no\nPrintMotd no\nUsePAM no\nAllowTcpForwarding yes\nGatewayPorts yes\nPermitOpen any\nPermitListen any\nAllowAgentForwarding yes\nX11Forwarding no\n' >> /etc/ssh/sshd_config

EXPOSE 22

COPY entrypoint.sh /entrypoint.sh
COPY id_ed25519.pub /tmp/id_ed25519.pub
RUN cat /tmp/id_ed25519.pub >> /home/tshell/.ssh/authorized_keys \
    && chown tshell:tshell /home/tshell/.ssh/authorized_keys \
    && chmod 600 /home/tshell/.ssh/authorized_keys \
    && rm /tmp/id_ed25519.pub
RUN chmod +x /entrypoint.sh

CMD ["/entrypoint.sh"]
