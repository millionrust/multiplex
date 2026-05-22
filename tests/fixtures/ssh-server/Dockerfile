FROM alpine:3.20

RUN apk add --no-cache openssh shadow

RUN adduser -D -s /bin/sh termirust \
    && echo "termirust:termirust-pass" | chpasswd \
    && mkdir -p /home/termirust/.ssh /run/sshd \
    && chown -R termirust:termirust /home/termirust/.ssh \
    && chmod 700 /home/termirust/.ssh \
    && ssh-keygen -A \
    && sed -i 's/^#\\?PasswordAuthentication .*/PasswordAuthentication yes/' /etc/ssh/sshd_config \
    && sed -i 's/^#\\?PermitRootLogin .*/PermitRootLogin no/' /etc/ssh/sshd_config \
    && printf '\nUsePAM no\nKbdInteractiveAuthentication no\nChallengeResponseAuthentication no\nPrintMotd no\n' >> /etc/ssh/sshd_config

EXPOSE 22

COPY entrypoint.sh /entrypoint.sh
COPY id_ed25519.pub /tmp/id_ed25519.pub
RUN cat /tmp/id_ed25519.pub >> /home/termirust/.ssh/authorized_keys \
    && chown termirust:termirust /home/termirust/.ssh/authorized_keys \
    && chmod 600 /home/termirust/.ssh/authorized_keys \
    && rm /tmp/id_ed25519.pub
RUN chmod +x /entrypoint.sh

CMD ["/entrypoint.sh"]
