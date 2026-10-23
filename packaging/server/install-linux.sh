#!/usr/bin/env bash
# Installs the EloraNotes collaboration server on this machine.
#
#   sudo ./install-linux.sh
#
# It asks for the name people will see in their sidebar, the port and the password everyone
# connects with, then installs the binary, creates the notes folder, generates the certificate,
# and registers a service that starts at boot. The server listens on all interfaces so people on
# other networks can reach it; the last step opens the port in the firewall if one is running.
#
# On a system whose /etc/systemd/system is read-only (NixOS and other immutable systems) the
# installer cannot add a system service. It then registers a service for your own account instead,
# with the notes in ~/.local/share/elora-server and lingering enabled so it still starts at boot.
# Set ELORA_SERVICE_MODE=system or ELORA_SERVICE_MODE=user to choose that yourself.
set -euo pipefail

RELEASE_URL="https://github.com/noxilith/EloraNotes/releases/latest/download/elora-server-linux-x86_64"
SERVICE_NAME="elora-server.service"
SERVICE_USER="elora"
SERVICE_MODE=${ELORA_SERVICE_MODE:-auto}

say() { printf '%s\n' "$*"; }
fail() { printf 'error: %s\n' "$*" >&2; exit 1; }

if [ "$(id -u)" -ne 0 ]; then
    fail "run this with root rights (for example: sudo bash $0)"
fi

command -v systemctl >/dev/null 2>&1 \
    || fail "this machine has no systemd, so the installer cannot register the service"
command -v runuser >/dev/null 2>&1 \
    || fail "runuser (part of util-linux) is missing, so the installer cannot switch users"

# --- is a server already installed here? ----------------------------------------------------
# Checked before anything is asked or downloaded: an update must never look like a fresh install,
# and uninstalling has to know where the old files and service are.
EXISTING_MODE=""
EXISTING_UNIT=""
EXISTING_DATA=""
EXISTING_BINARY=""
EXISTING_OWNER_HOME=""

# The usual places, overridable for tests and unusual layouts.
SYSTEM_UNIT_PATH="${ELORA_UNIT_DIR:-/etc/systemd/system}/$SERVICE_NAME"
SYSTEM_DATA_DIR="${ELORA_SERVER_DATA_DIR:-/var/lib/elora-server}"
SYSTEM_BIN="${ELORA_SERVER_BIN:-/usr/local/bin/elora-server}"

# The account a per-user service would belong to, needed before the mode is chosen.
if [ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != "root" ]; then
    EXISTING_OWNER="$SUDO_USER"
    EXISTING_OWNER_HOME=$(getent passwd "$SUDO_USER" | cut -d: -f6)
fi
USER_UNIT_PATH="${EXISTING_OWNER_HOME:+$EXISTING_OWNER_HOME/.config/systemd/user/$SERVICE_NAME}"
USER_DATA_DIR="${EXISTING_OWNER_HOME:+$EXISTING_OWNER_HOME/.local/share/elora-server}"
USER_BIN="${EXISTING_OWNER_HOME:+$EXISTING_OWNER_HOME/.local/bin/elora-server}"

# Only a service decides the mode. A leftover settings folder alone does not, because the mode
# still has to fit this system (a read-only /etc cannot take a system unit).
if [ -f "$SYSTEM_UNIT_PATH" ]; then
    EXISTING_MODE="system"
    EXISTING_UNIT="$SYSTEM_UNIT_PATH"
elif [ -n "$USER_UNIT_PATH" ] && [ -f "$USER_UNIT_PATH" ]; then
    EXISTING_MODE="user"
    EXISTING_UNIT="$USER_UNIT_PATH"
fi

# A binary or a settings folder can also be left behind by an install that stopped half way.
for candidate in "$SYSTEM_BIN" /opt/elora-server/elora-server "$USER_BIN"; do
    if [ -n "$candidate" ] && [ -x "$candidate" ]; then
        EXISTING_BINARY="$candidate"
        break
    fi
done
if [ -z "$EXISTING_DATA" ]; then
    for candidate in "$SYSTEM_DATA_DIR" "$USER_DATA_DIR"; do
        if [ -n "$candidate" ] && [ -f "$candidate/config.json" ]; then
            EXISTING_DATA="$candidate"
            break
        fi
    done
fi

EXISTING=no
if [ -n "$EXISTING_MODE$EXISTING_BINARY$EXISTING_DATA" ]; then
    EXISTING=yes
fi

# ELORA_ACTION=update|uninstall|purge skips the question (for scripted installs).
ACTION=${ELORA_ACTION:-}
case "$ACTION" in
    "" | update | uninstall | purge) ;;
    *) fail "ELORA_ACTION must be 'update', 'uninstall' or 'purge', not '$ACTION'" ;;
esac

if [ "$EXISTING" = "yes" ]; then
    NOTE_COUNT=0
    if [ -n "$EXISTING_DATA" ]; then
        NOTE_COUNT=$(ls -1 "$EXISTING_DATA/notes"/*.md 2>/dev/null | wc -l || true)
    fi
    CODE=""
    if [ -n "$EXISTING_BINARY" ] && [ -n "$EXISTING_DATA" ]; then
        CODE=$(ELORA_SERVER_DIR="$EXISTING_DATA" "$EXISTING_BINARY" fingerprint 2>/dev/null \
            | head -n 1 || true)
    fi

    say ""
    say "EloraNotes server is already installed on this machine:"
    if [ -n "$EXISTING_MODE" ]; then
        say "  service:  $EXISTING_UNIT ($EXISTING_MODE service)"
    else
        say "  service:  none found"
    fi
    [ -n "$EXISTING_BINARY" ] && say "  binary:   $EXISTING_BINARY"
    if [ -n "$EXISTING_DATA" ]; then
        say "  settings: $EXISTING_DATA"
        say "  notes:    $NOTE_COUNT in $EXISTING_DATA/notes"
    fi
    [ -n "$CODE" ] && say "  code:     $CODE (clients already trust this certificate)"

    if [ -z "$ACTION" ]; then
        if [ -t 0 ]; then
            say ""
            say "What would you like to do?"
            say "  1) Update it, keeping the notes and the certificate  (recommended)"
            say "  2) Uninstall it, keeping the notes on disk"
            say "  3) Uninstall it and delete the notes and certificate"
            say "  4) Cancel"
            while true; do
                read -r -p "Choice [1]: " CHOICE \
                    || fail "no answer was given; run the installer again and choose 1, 2, 3 or 4"
                case "${CHOICE:-1}" in
                    1) ACTION=update; break ;;
                    2) ACTION=uninstall; break ;;
                    3) ACTION=purge; break ;;
                    4) ACTION=cancel; break ;;
                    *) say "  Type 1, 2, 3 or 4." ;;
                esac
            done
        else
            ACTION=update
            say "No terminal to ask in, so the existing install is updated."
        fi
    fi

    if [ "$ACTION" = "cancel" ]; then
        say "Nothing was changed."
        exit 0
    fi

    if [ "$ACTION" = "uninstall" ] || [ "$ACTION" = "purge" ]; then
        say ""
        say "Uninstalling…"
        # Stop and forget the service, whichever kind of install it is.
        if [ -f "$SYSTEM_UNIT_PATH" ]; then
            systemctl stop "$SERVICE_NAME" >/dev/null 2>&1 || true
            systemctl disable "$SERVICE_NAME" >/dev/null 2>&1 || true
            rm -f "$SYSTEM_UNIT_PATH"
            systemctl daemon-reload >/dev/null 2>&1 || true
            systemctl reset-failed "$SERVICE_NAME" >/dev/null 2>&1 || true
            say "  Removed $SYSTEM_UNIT_PATH"
        fi
        if [ -n "$USER_UNIT_PATH" ] && [ -f "$USER_UNIT_PATH" ]; then
            RUNTIME_DIR="/run/user/$(id -u "$EXISTING_OWNER")"
            runuser -u "$EXISTING_OWNER" -- env XDG_RUNTIME_DIR="$RUNTIME_DIR" \
                systemctl --user disable --now "$SERVICE_NAME" >/dev/null 2>&1 || true
            rm -f "$USER_UNIT_PATH"
            runuser -u "$EXISTING_OWNER" -- env XDG_RUNTIME_DIR="$RUNTIME_DIR" \
                systemctl --user daemon-reload >/dev/null 2>&1 || true
            say "  Removed $USER_UNIT_PATH"
        fi
        # The binary, wherever this install put it.
        for path in "$SYSTEM_BIN" /opt/elora-server/elora-server "$USER_BIN"; do
            if [ -n "$path" ] && [ -e "$path" ]; then
                rm -f "$path" && say "  Removed $path"
            fi
        done
        rmdir /opt/elora-server >/dev/null 2>&1 || true

        if [ "$ACTION" = "purge" ]; then
            if [ -n "$EXISTING_DATA" ] && [ -d "$EXISTING_DATA" ]; then
                rm -rf "$EXISTING_DATA" && say "  Deleted $EXISTING_DATA"
            fi
            # The service account only exists for a system install; remove it with the data.
            if id -u "$SERVICE_USER" >/dev/null 2>&1; then
                userdel "$SERVICE_USER" >/dev/null 2>&1 || true
                say "  Removed the $SERVICE_USER account"
            fi
            if getent group "$SERVICE_USER" >/dev/null 2>&1; then
                groupdel "$SERVICE_USER" >/dev/null 2>&1 || true
            fi
            say ""
            say "The server and everything it held are gone."
        else
            say ""
            if [ -n "$EXISTING_DATA" ]; then
                say "The server is gone; the notes are still in $EXISTING_DATA/notes."
                say "Copy that folder somewhere safe if you want to keep them."
            else
                say "The server is gone."
            fi
        fi
        say "Run this installer again to set a server up from scratch."
        exit 0
    fi

    say ""
    say "Updating it; the notes and the certificate stay where they are."
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

# --- system service or per-user service ----------------------------------------------------
# A unit has to be written into a directory systemd reads from. On an immutable system
# /etc/systemd/system is a read-only link into the system image, so it is tested for real
# instead of assumed.
if [ "$SERVICE_MODE" = "auto" ]; then
    if [ -n "$EXISTING_MODE" ]; then
        # Keep an existing install where it is; switching would leave its notes behind.
        SERVICE_MODE="$EXISTING_MODE"
    elif SYSTEM_UNIT_DIR=$(dirname -- "$SYSTEM_UNIT_PATH") \
        && ( : > "$SYSTEM_UNIT_DIR/.elora-write-test" ) 2>/dev/null; then
        rm -f "$SYSTEM_UNIT_DIR/.elora-write-test" 2>/dev/null || true
        SERVICE_MODE="system"
    else
        SERVICE_MODE="user"
    fi
fi
case "$SERVICE_MODE" in
    system | user) ;;
    *) fail "ELORA_SERVICE_MODE must be 'system' or 'user', not '$SERVICE_MODE'" ;;
esac

if [ "$SERVICE_MODE" = "system" ]; then
    DATA_DIR=$SYSTEM_DATA_DIR
    UNIT_DIR=$(dirname -- "$SYSTEM_UNIT_PATH")
    BINARY=$SYSTEM_BIN
    TARGET_USER=""
    TARGET_HOME=""
else
    TARGET_USER=${ELORA_TARGET_USER:-${SUDO_USER:-}}
    [ -n "$TARGET_USER" ] || fail \
        "no account to run the service as; run the installer with sudo from your own account, or set ELORA_TARGET_USER"
    [ "$TARGET_USER" != "root" ] || fail \
        "the per-user service needs a normal account; set ELORA_TARGET_USER"
    TARGET_HOME=$(getent passwd "$TARGET_USER" | cut -d: -f6)
    [ -n "$TARGET_HOME" ] && [ -d "$TARGET_HOME" ] \
        || fail "could not find the home folder of $TARGET_USER"
    TARGET_GROUP=$(id -gn "$TARGET_USER")
    DATA_DIR=${ELORA_SERVER_DATA_DIR:-$TARGET_HOME/.local/share/elora-server}
    UNIT_DIR=${ELORA_UNIT_DIR:-$TARGET_HOME/.config/systemd/user}
    if [ -n "${ELORA_SERVER_BIN:-}" ]; then
        BINARY=$ELORA_SERVER_BIN
    elif [ -w /usr/local/bin ] && ( : > /usr/local/bin/.elora-write-test ) 2>/dev/null; then
        rm -f /usr/local/bin/.elora-write-test 2>/dev/null || true
        BINARY=/usr/local/bin/elora-server
    else
        BINARY=$TARGET_HOME/.local/bin/elora-server
    fi
    say ""
    say "/etc/systemd/system is read-only on this system, so the server is installed as a"
    say "per-user service for $TARGET_USER instead, with its notes in $DATA_DIR."
fi

# --- where the binary goes -----------------------------------------------------------------
BIN_DIR=$(dirname -- "$BINARY")
mkdir -p "$BIN_DIR" 2>/dev/null || true
if [ ! -d "$BIN_DIR" ] || ! touch "$BIN_DIR/.elora-write-test" 2>/dev/null; then
    rm -f "$BIN_DIR/.elora-write-test" 2>/dev/null || true
    BINARY="/opt/elora-server/elora-server"
    BIN_DIR=$(dirname -- "$BINARY")
    say "$BIN_DIR is not writable here; installing the server to $BINARY instead."
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

# --- the account, the folder, the settings -------------------------------------------------
if [ "$SERVICE_MODE" = "system" ]; then
    # `useradd --system` does not give the user a group of its own on Debian and Ubuntu (that is
    # adduser's behaviour), so the group is created first instead of assuming it exists.
    if ! getent group "$SERVICE_USER" >/dev/null 2>&1; then
        groupadd --system "$SERVICE_USER" || fail "could not create the $SERVICE_USER group"
    fi
    if ! id -u "$SERVICE_USER" >/dev/null 2>&1; then
        useradd --system --gid "$SERVICE_USER" --home-dir "$DATA_DIR" \
            --shell /usr/sbin/nologin "$SERVICE_USER" \
            || fail "could not create the $SERVICE_USER user"
    fi
    SERVICE_GROUP=$(id -gn "$SERVICE_USER")
    if [ "$SERVICE_GROUP" != "$SERVICE_USER" ]; then
        # An install from an older release left the user on `nogroup`; move it to its own group.
        usermod -g "$SERVICE_USER" "$SERVICE_USER" \
            || fail "could not move $SERVICE_USER to its own group"
        SERVICE_GROUP="$SERVICE_USER"
    fi
    install -d -m 0750 -o "$SERVICE_USER" -g "$SERVICE_GROUP" "$DATA_DIR"

    # The settings, certificate and notes are written here as root and handed to the service
    # user afterwards, so the script never has to switch users (sudo is not always installed).
    say "Writing the settings and the certificate…"
    printf '%s\n' "$PASSWORD" | ELORA_SERVER_DIR="$DATA_DIR" \
        "$BINARY" init --name "$SERVER_NAME" --port "$PORT" --password-stdin
    chown -R "$SERVICE_USER:$SERVICE_GROUP" "$DATA_DIR"
    UNIT_USER="User=$SERVICE_USER"
    UNIT_GROUP="Group=$SERVICE_GROUP"
    UNIT_WANTED="WantedBy=multi-user.target"
    UNIT_HARDENING="ProtectSystem=full
ProtectHome=yes
PrivateTmp=yes
NoNewPrivileges=yes"
else
    # Created as the user, so ~/.local/share never ends up owned by root.
    runuser -u "$TARGET_USER" -- mkdir -p "$DATA_DIR" \
        || fail "could not create $DATA_DIR"
    chmod 0750 "$DATA_DIR"

    # An install that stopped at the system-service step (or a switch from system to user mode)
    # already has a certificate. Carrying it over keeps the code clients confirm unchanged, and
    # any notes written meanwhile come along too.
    PREVIOUS_DATA_DIR=${ELORA_PREVIOUS_DATA_DIR:-$SYSTEM_DATA_DIR}
    if [ ! -f "$DATA_DIR/cert.pem" ] && [ -f "$PREVIOUS_DATA_DIR/cert.pem" ]; then
        say "Carrying the certificate over from $PREVIOUS_DATA_DIR, so clients keep the same code."
        if cp -a "$PREVIOUS_DATA_DIR/cert.pem" "$PREVIOUS_DATA_DIR/key.pem" "$DATA_DIR/" 2>/dev/null; then
            chown "$TARGET_USER:$TARGET_GROUP" "$DATA_DIR/cert.pem" "$DATA_DIR/key.pem" 2>/dev/null || true
            chmod 0600 "$DATA_DIR/key.pem" 2>/dev/null || true
        fi
    fi
    if [ -d "$PREVIOUS_DATA_DIR/notes" ] && [ -z "$(ls -A "$PREVIOUS_DATA_DIR/notes" 2>/dev/null)" ]; then
        : # nothing to bring over
    elif [ -d "$PREVIOUS_DATA_DIR/notes" ] && [ -z "$(ls -A "$DATA_DIR/notes" 2>/dev/null)" ]; then
        say "Copying the notes from $PREVIOUS_DATA_DIR/notes."
        runuser -u "$TARGET_USER" -- mkdir -p "$DATA_DIR/notes"
        cp -a "$PREVIOUS_DATA_DIR/notes/." "$DATA_DIR/notes/" 2>/dev/null || true
        chown -R "$TARGET_USER:$TARGET_GROUP" "$DATA_DIR/notes" 2>/dev/null || true
    fi
    say "Writing the settings and the certificate…"
    printf '%s\n' "$PASSWORD" | runuser -u "$TARGET_USER" -- \
        env ELORA_SERVER_DIR="$DATA_DIR" \
        "$BINARY" init --name "$SERVER_NAME" --port "$PORT" --password-stdin \
        || fail "could not write the settings as $TARGET_USER"
    chown -R "$TARGET_USER:$TARGET_GROUP" "$DATA_DIR"
    UNIT_USER=""
    UNIT_GROUP=""
    UNIT_WANTED="WantedBy=default.target"
    # A user service cannot take the home folder away from itself, so only the cheap guard stays.
    UNIT_HARDENING="NoNewPrivileges=yes"
fi

# The password and the private key are for the service only; a missing file is not fatal.
chmod 0600 "$DATA_DIR/config.json" "$DATA_DIR/key.pem" 2>/dev/null || true
chmod 0644 "$DATA_DIR/cert.pem" 2>/dev/null || true

# --- service -------------------------------------------------------------------------------
if [ "$SERVICE_MODE" = "system" ]; then
    mkdir -p "$UNIT_DIR" || fail "could not create $UNIT_DIR"
else
    runuser -u "$TARGET_USER" -- mkdir -p "$UNIT_DIR" \
        || fail "could not create $UNIT_DIR"
fi
UNIT_PATH="$UNIT_DIR/$SERVICE_NAME"
if ! ( : > "$UNIT_PATH" ) 2>/dev/null; then
    fail "cannot write $UNIT_PATH (is the filesystem read-only?)"
fi
{
    echo "[Unit]"
    echo "Description=EloraNotes collaboration server ($SERVER_NAME)"
    echo "After=network-online.target"
    echo "Wants=network-online.target"
    echo ""
    echo "[Service]"
    echo "Type=simple"
    [ -n "$UNIT_USER" ] && echo "$UNIT_USER"
    [ -n "$UNIT_GROUP" ] && echo "$UNIT_GROUP"
    echo "Environment=ELORA_SERVER_DIR=$DATA_DIR"
    echo "ExecStart=$BINARY run"
    echo "Restart=on-failure"
    echo "RestartSec=3"
    printf '%s\n' "$UNIT_HARDENING"
    echo ""
    echo "[Install]"
    echo "$UNIT_WANTED"
} >"$UNIT_PATH"

started=""
if [ "$SERVICE_MODE" = "system" ]; then
    systemctl daemon-reload
    systemctl enable --now "$SERVICE_NAME"
    sleep 1
    if ! systemctl is-active --quiet "$SERVICE_NAME"; then
        say "The service did not start; recent log lines:"
        journalctl -u "$SERVICE_NAME" -n 20 --no-pager || true
        fail "fix the error above, then run: systemctl restart elora-server"
    fi
    started="systemctl is-active"
else
    chown "$TARGET_USER:$TARGET_GROUP" "$UNIT_PATH"
    RUNTIME_DIR="/run/user/$(id -u "$TARGET_USER")"
    loginctl enable-linger "$TARGET_USER" >/dev/null 2>&1 \
        || say "Could not enable lingering; the server would only run while $TARGET_USER is logged in."
    if runuser -u "$TARGET_USER" -- env XDG_RUNTIME_DIR="$RUNTIME_DIR" \
        systemctl --user daemon-reload 2>/dev/null; then
        runuser -u "$TARGET_USER" -- env XDG_RUNTIME_DIR="$RUNTIME_DIR" \
            systemctl --user enable --now "$SERVICE_NAME" \
            || say "Could not start the user service from here; see the commands at the end."
        sleep 1
        if runuser -u "$TARGET_USER" -- env XDG_RUNTIME_DIR="$RUNTIME_DIR" \
            systemctl --user is-active --quiet "$SERVICE_NAME"; then
            started="systemctl --user is-active"
        else
            say "Recent log lines:"
            runuser -u "$TARGET_USER" -- env XDG_RUNTIME_DIR="$RUNTIME_DIR" \
                journalctl --user -u "$SERVICE_NAME" -n 20 --no-pager || true
        fi
    else
        say "The service manager for $TARGET_USER is not reachable from here."
    fi
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
say "Unit file:     $UNIT_PATH"
say "Notes live in $DATA_DIR/notes."
if [ "$SERVICE_MODE" = "system" ]; then
    say "Manage the service with:"
    say "  systemctl status elora-server     systemctl restart elora-server"
    say "  journalctl -u elora-server -f     (live log)"
else
    say "It runs as a per-user service for $TARGET_USER, so manage it from your own shell:"
    say "  systemctl --user status elora-server     systemctl --user restart elora-server"
    say "  journalctl --user -u elora-server -f     (live log)"
    if [ -z "$started" ]; then
        say ""
        say "Start it now with:"
        say "  systemctl --user daemon-reload && systemctl --user enable --now elora-server"
    fi
fi

if [ -e /etc/NIXOS ]; then
    say ""
    say "This looks like NixOS. The service above works, but survives a rebuild only if you"
    say "declare it; add this to configuration.nix instead if you prefer that:"
    say ""
    say "  systemd.services.elora-server = {"
    say "    description = \"EloraNotes collaboration server\";"
    say "    wantedBy = [ \"multi-user.target\" ];"
    say "    after = [ \"network-online.target\" ];"
    say "    serviceConfig = {"
    say "      ExecStart = \"$BINARY run\";"
    say "      Environment = \"ELORA_SERVER_DIR=$DATA_DIR\";"
    say "      Restart = \"on-failure\";"
    say "      User = \"$TARGET_USER\";"
    say "    };"
    say "  };"
fi
