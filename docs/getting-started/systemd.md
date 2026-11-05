---
description: Run the RavaLink native binary as a systemd service.
---

# Systemd service

A systemd unit can keep RavaLink running in the background and restart it after a failure.

## Prepare the service account

Create a dedicated unprivileged account and install the binary and configuration in a directory readable by that account. The example below assumes:

- Binary: /opt/ravalink/ravalink
- Configuration: /etc/ravalink/ravalink.toml
- Service user and group: ravalink

Build the binary using the [source installation guide](binary.md), then install it at the path above.

## Create the unit

Create /etc/systemd/system/ravalink.service:

~~~ini
[Unit]
Description=RavaLink audio node
Wants=network-online.target
After=network-online.target

[Service]
Type=simple
User=ravalink
Group=ravalink
WorkingDirectory=/opt/ravalink
Environment=RAVALINK_CONFIG=/etc/ravalink/ravalink.toml
EnvironmentFile=/etc/ravalink/ravalink.env
ExecStart=/opt/ravalink/ravalink
Restart=on-failure
RestartSec=5
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/opt/ravalink

[Install]
WantedBy=multi-user.target
~~~

Create /etc/ravalink/ravalink.env with a strong secret:

~~~bash
RAVALINK_PASSWORD=replace-this-with-a-long-random-secret
~~~

Restrict access to the environment file:

~~~bash
sudo chown root:ravalink /etc/ravalink/ravalink.env
sudo chmod 0640 /etc/ravalink/ravalink.env
~~~

## Enable and inspect

~~~bash
sudo systemctl daemon-reload
sudo systemctl enable --now ravalink
sudo systemctl status ravalink
sudo journalctl -u ravalink -f
~~~

Adjust the paths and service account to match your installation. Do not expose port 2333 publicly without a firewall and a secure reverse-proxy/TLS configuration.
