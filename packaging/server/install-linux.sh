#!/usr/bin/env bash
# Installs the EloraNotes collaboration server on this machine.
#
#   sudo ./install-linux.sh
#
# It asks for the name people will see in their sidebar, the port and the password everyone
# connects with, then installs the binary, creates the notes folder, generates the certificate,
# and registers a service that starts at boot. The server listens on all interfaces so people on
# other networks can reach it; the last step opens the port in the firewall if one is running.
set -euo pipefail

RELEASE_URL="https://github.com/noxilith/EloraNotes/releases/latest/download/elora-server-linux-x86_64"
BINARY="/usr/local/bin/elora-server"
SERVICE="/etc/systemd/system/elora-server.service"
SERVICE_USER="elora"
DATA_DIR="/var/lib/elora-server"

say() { printf '%s\n' "$*"; }
fail() { printf 'error: %s\n' "$*" >&2; exit 1; }

if [ "$(id -u)" -ne 0 ]; then
    fail "run this with sudo: sudo $0"
fi

# --- what the server should be called ------------------------------------------------------
read -r -p "Server name (shown in everyone's sidebar) [EloraNotes]: " SERVER_NAME
SERVER_NAME=${SERVER_NAME:-EloraNotes}

while true; do
    read -r -p "Port [7331]: " PORT
    PORT=${PORT:-7331}
    if ! [[ "$PORT" =~ ^[0-9]+$ ]] || [ "$PORT" -lt 1 ] || [ "$PORT" -gt 65535 ]; then
        say "  That is not a port number."
        continue
    fi
    if [ "$PORT" -lt 1024 ]; then
        say "  Ports below 1024 are reserved; pick a higher one."
        continue
    fi
    break
done

read -r -s -p "Password everyone will connect with: " PASSWORD; say
read -r -s -p "Repeat the password: " PASSWORD_AGAIN; say
[ -n "$PASSWORD" ] || fail "the password cannot be empty"
[ "$PASSWORD" = "$PASSWORD_AGAIN" ] || fail "the passwords did not match"

# --- the binary ----------------------------------------------------------------------------
SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
if [ -x "$SCRIPT_DIR/elora-server" ]; then
    say "Using $SCRIPT_DIR/elora-server"
    install -m 0755 "$SCRIPT_DIR/elora-server" "$BINARY"
elif [ -x "$SCRIPT_DIR/elora-server-linux-x86_64" ]; then
    say "Using $SCRIPT_DIR/elora-server-linux-x86_64"
    install -m 0755 "$SCRIPT_DIR/elora-server-linux-x86_64" "$BINARY"
else
    say "Downloading the server from the latest EloraNotes release…"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --retry 3 -o "$BINARY.download" "$RELEASE_URL" \
            || fail "could not download $RELEASE_URL (pass the binary next to this script instead)"
    elif command -v wget >/dev/null 2>&1; then
        wget -q -O "$BINARY.download" "$RELEASE_URL" \
            || fail "could not download $RELEASE_URL (pass the binary next to this script instead)"
    else
        fail "neither curl nor wget is installed; put the elora-server binary next to this script"
    fi
    install -m 0755 "$BINARY.download" "$BINARY"
    rm -f "$BINARY.download"
fi

# --- a user and folder of its own, so the server never runs as root -------------------------
if ! id -u "$SERVICE_USER" >/dev/null 2>&1; then
    useradd --system --home-dir "$DATA_DIR" --shell /usr/sbin/nologin "$SERVICE_USER"
fi
install -d -m 0750 -o "$SERVICE_USER" -g "$SERVICE_USER" "$DATA_DIR"

say "Writing the settings and the certificate…"
printf '%s\n' "$PASSWORD" | sudo -u "$SERVICE_USER" \
    env ELORA_SERVER_DIR="$DATA_DIR" \
    "$BINARY" init --name "$SERVER_NAME" --port "$PORT" --password-stdin

# --- service -------------------------------------------------------------------------------
cat >"$SERVICE" <<UNIT
[Unit]
Description=EloraNotes collaboration server ($SERVER_NAME)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=$SERVICE_USER
Group=$SERVICE_USER
Environment=ELORA_SERVER_DIR=$DATA_DIR
ExecStart=$BINARY run
Restart=on-failure
RestartSec=3
# The server only needs its own folder.
ProtectSystem=full
ProtectHome=yes
PrivateTmp=yes
NoNewPrivileges=yes

[Install]
WantedBy=multi-user.target
UNIT

systemctl daemon-reload
systemctl enable --now elora-server.service
sleep 1
if ! systemctl is-active --quiet elora-server.service; then
    say "The service did not start; recent log lines:"
    journalctl -u elora-server.service -n 20 --no-pager || true
    fail "fix the error above, then run: systemctl restart elora-server"
fi

# --- firewall ------------------------------------------------------------------------------
if command -v ufw >/dev/null 2>&1 && ufw status 2>/dev/null | grep -q "Status: active"; then
    ufw allow "$PORT"/tcp >/dev/null && say "Opened port $PORT in ufw"
elif command -v firewall-cmd >/dev/null 2>&1 && firewall-cmd --state >/dev/null 2>&1; then
    firewall-cmd --permanent --add-port="$PORT"/tcp >/dev/null
    firewall-cmd --reload >/dev/null
    say "Opened port $PORT in firewalld"
else
    say "No active ufw or firewalld rule was found. If you use another firewall, allow TCP $PORT."
fi

# --- what to tell everyone else ------------------------------------------------------------
FINGERPRINT=$("$BINARY" fingerprint 2>/dev/null | head -n 1 || true)
LOCAL_IP=$(ip -4 route get 1.1.1.1 2>/dev/null | awk '{for (i = 1; i < NF; i++) if ($i == "src") print $(i + 1)}' | head -n 1)

say ""
say "EloraNotes server \"$SERVER_NAME\" is running on port $PORT."
if [ -n "$LOCAL_IP" ]; then
    say "  Same network:  $LOCAL_IP  port $PORT"
fi
say "  Other networks: this machine's public IP or a DNS name, port $PORT"
say "  Router: forward TCP $PORT to this machine once."
if [ -n "$FINGERPRINT" ]; then
    say "  Certificate code clients confirm: $FINGERPRINT"
fi
say ""
say "Notes live in $DATA_DIR/notes. Manage the service with:"
say "  systemctl status elora-server     systemctl restart elora-server"
say "  journalctl -u elora-server -f     (live log)"
