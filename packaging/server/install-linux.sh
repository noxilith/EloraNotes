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
BINARY=${ELORA_SERVER_BIN:-/usr/local/bin/elora-server}
SERVICE="/etc/systemd/system/elora-server.service"
SERVICE_USER="elora"
DATA_DIR=${ELORA_SERVER_DATA_DIR:-/var/lib/elora-server}

say() { printf '%s\n' "$*"; }
fail() { printf 'error: %s\n' "$*" >&2; exit 1; }

if [ "$(id -u)" -ne 0 ]; then
    fail "run this with root rights (for example: sudo bash $0)"
fi

command -v systemctl >/dev/null 2>&1 \
    || fail "this machine has no systemd, so the installer cannot register the service"

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

# --- where the binary goes -----------------------------------------------------------------
# /usr/local/bin is not writable on every system (immutable roots, NixOS), and a failed download
# must not be the way people find that out, so the destination is checked first.
BIN_DIR=$(dirname -- "$BINARY")
mkdir -p "$BIN_DIR" 2>/dev/null || true
if [ ! -d "$BIN_DIR" ] || ! touch "$BIN_DIR/.elora-write-test" 2>/dev/null; then
    rm -f "$BIN_DIR/.elora-write-test" 2>/dev/null || true
    BINARY="/opt/elora-server/elora-server"
    BIN_DIR=$(dirname -- "$BINARY")
    say "$ELORA_SERVER_BIN is not writable here; installing the server to $BINARY instead."
    mkdir -p "$BIN_DIR" || fail "could not create $BIN_DIR"
fi
rm -f "$BIN_DIR/.elora-write-test" 2>/dev/null || true

# --- the binary ----------------------------------------------------------------------------
# The download goes to a temporary folder first: writing next to the destination fails on a
# read-only filesystem, and a half-written file must never end up being installed.
WORK_DIR=$(mktemp -d "${TMPDIR:-/tmp}/elora-server.XXXXXX") \
    || fail "could not create a temporary folder in ${TMPDIR:-/tmp}"
trap 'rm -rf "$WORK_DIR"' EXIT

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
STAGED=""
if [ -x "$SCRIPT_DIR/elora-server" ]; then
    say "Using $SCRIPT_DIR/elora-server"
    STAGED="$SCRIPT_DIR/elora-server"
elif [ -x "$SCRIPT_DIR/elora-server-linux-x86_64" ]; then
    say "Using $SCRIPT_DIR/elora-server-linux-x86_64"
    STAGED="$SCRIPT_DIR/elora-server-linux-x86_64"
else
    free_mb=$(df -Pk "$WORK_DIR" | awk 'NR == 2 { print int($4 / 1024) }')
    if [ -n "$free_mb" ] && [ "$free_mb" -lt 20 ]; then
        fail "only ${free_mb} MB free in $WORK_DIR; the server needs about 20 MB while it downloads"
    fi
    say "Downloading the server from the latest EloraNotes release…"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --retry 3 --retry-delay 2 \
            -o "$WORK_DIR/elora-server" "$RELEASE_URL" \
            || fail "could not download $RELEASE_URL (check that this machine can reach github.com; you can also put the elora-server binary next to this script and run it again)"
    elif command -v wget >/dev/null 2>&1; then
        wget -q -O "$WORK_DIR/elora-server" "$RELEASE_URL" \
            || fail "could not download $RELEASE_URL (check that this machine can reach github.com; you can also put the elora-server binary next to this script and run it again)"
    else
        fail "neither curl nor wget is installed; put the elora-server binary next to this script"
    fi
    STAGED="$WORK_DIR/elora-server"
fi

[ -s "$STAGED" ] || fail "the server binary is empty; something went wrong while fetching it"
install -m 0755 "$STAGED" "$BINARY" || fail "could not install the server to $BINARY"
"$BINARY" --help >/dev/null 2>&1 \
    || fail "$BINARY does not run on this machine (wrong CPU architecture or a corrupt download)"

# --- a user and folder of its own, so the server never runs as root -------------------------
if ! id -u "$SERVICE_USER" >/dev/null 2>&1; then
    useradd --system --home-dir "$DATA_DIR" --shell /usr/sbin/nologin "$SERVICE_USER"
fi
install -d -m 0750 "$DATA_DIR"

# The settings, certificate and notes are written here as root and handed to the service user
# afterwards, so the script never has to switch users (sudo is not always installed).
say "Writing the settings and the certificate…"
printf '%s\n' "$PASSWORD" | ELORA_SERVER_DIR="$DATA_DIR" \
    "$BINARY" init --name "$SERVER_NAME" --port "$PORT" --password-stdin
chown -R "$SERVICE_USER:$SERVICE_USER" "$DATA_DIR"

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
say "Server binary: $BINARY"
say "Notes live in $DATA_DIR/notes. Manage the service with:"
say "  systemctl status elora-server     systemctl restart elora-server"
say "  journalctl -u elora-server -f     (live log)"
