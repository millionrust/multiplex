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
RUN chmod +x /entrypoint.sh

CMD ["/entrypoint.sh"]
