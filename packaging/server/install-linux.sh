#!/usr/bin/env bash
# Installs the EloraNotes collaboration server on this machine.
#
#   curl -fsSLO https://github.com/noxilith/EloraNotes/releases/latest/download/install-elora-server.sh
#   sudo bash install-elora-server.sh
#
# It asks for the name people will see in their sidebar, the port and the password everyone
# connects with, then installs the binary, creates the notes folder, generates the certificate,
# and registers a service that starts at boot. The server listens on all interfaces so people on
# other networks can reach it; the last step opens the port in the firewall if one is running.
#
# Run it again on a machine that already has a server and it says what it found and offers to
# update it (keeping the settings, the password, the notes and the certificate), to uninstall it
# and keep the notes, or to remove everything.
#
# On a system whose /etc/systemd/system is read-only (NixOS and other immutable systems) the
# installer cannot add a system service. It then registers a service for your own account instead,
# with the notes in ~/.local/share/elora-server and lingering enabled so it still starts at boot.
# Set ELORA_SERVICE_MODE=system or ELORA_SERVICE_MODE=user to choose that yourself.
#
# For scripted installs, answer the questions through the environment:
#   ELORA_NAME=...  ELORA_PORT=...  ELORA_PASSWORD=...   (or ELORA_PASSWORD_FILE=/path/to/file)
#   ELORA_ACTION=update|uninstall|purge   ELORA_VERSION=0.10.1   (default: the latest release)
# The password is read from the environment or a file, never from the command line.
set -euo pipefail

RELEASES_URL="https://github.com/noxilith/EloraNotes/releases"
if [ -n "${ELORA_VERSION:-}" ]; then
    RELEASE_BASE="$RELEASES_URL/download/v${ELORA_VERSION#v}"
else
    RELEASE_BASE="$RELEASES_URL/latest/download"
fi
RELEASE_BASE=${ELORA_RELEASE_BASE:-$RELEASE_BASE}
ASSET="elora-server-linux-x86_64"
SERVICE_NAME="elora-server.service"
SERVICE_USER="elora"
SERVICE_MODE=${ELORA_SERVICE_MODE:-auto}
# How long to wait for the new server to start accepting connections.
HEALTH_WAIT=${ELORA_HEALTH_WAIT:-15}

say() { printf '%s\n' "$*"; }
fail() { printf 'error: %s\n' "$*" >&2; exit 1; }

# --- helpers --------------------------------------------------------------------------------

# A port a person may pick: 1024 and up, so the server needs no special rights.
valid_port() { [[ "$1" =~ ^[0-9]+$ ]] && [ "$((10#$1))" -ge 1024 ] && [ "$((10#$1))" -le 65535 ]; }

# One value from the server's config.json, which the server itself writes one key per line.
json_field() { # json_field FILE KEY
    sed -n "s/^[[:space:]]*\"$2\"[[:space:]]*:[[:space:]]*//p" "$1" 2>/dev/null | head -n 1 |
        sed -e 's/,[[:space:]]*$//' -e 's/^"//' -e 's/"$//' -e 's/\\"/"/g' -e 's/\\\\/\\/g'
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{ print $1 }'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{ print $1 }'
    elif command -v openssl >/dev/null 2>&1; then
        openssl dgst -sha256 "$1" | awk '{ print $NF }'
    else
        return 1
    fi
}

# The checksum a release's SHA256SUMS file lists for one file name.
listed_sha256() { # listed_sha256 SUMS_FILE NAME
    awk -v name="$2" '{ file = $2; sub(/^\*/, "", file); if (file == name) { print tolower($1); exit } }' "$1"
}

fetch() { # fetch URL DESTINATION
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --retry 3 --retry-delay 2 -o "$2" "$1"
    elif command -v wget >/dev/null 2>&1; then
        wget -q -O "$2" "$1"
    else
        return 127
    fi
}

# systemd reads a % in a unit file as the start of a specifier, so a literal one is doubled.
unit_escape() { printf '%s' "${1//%/%%}"; }

# Something is already listening on the port (on this machine).
port_listening() {
    local out
    if command -v ss >/dev/null 2>&1; then
        out=$(ss -ltn "sport = :$1" 2>/dev/null || true)
        [ -n "$(printf '%s\n' "$out" | sed 1d)" ]
    else
        ( exec 9<>"/dev/tcp/127.0.0.1/$1" ) 2>/dev/null
    fi
}

port_owner() {
    command -v ss >/dev/null 2>&1 || return 0
    { ss -ltnp "sport = :$1" 2>/dev/null || true; } |
        sed -n 's/.*users:(("\([^"]*\)".*/\1/p' | head -n 1
}

# True once the server accepts a connection, which is the only proof that it is really up: a
# service manager can call a process "active" a moment before it crashes.
wait_for_port() { # wait_for_port PORT SECONDS
    local waited=0
    while [ "$waited" -lt "$2" ]; do
        if ( exec 9<>"/dev/tcp/127.0.0.1/$1" ) 2>/dev/null; then
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done
    return 1
}

# `ufw status | grep -q ...` is not used on purpose: grep stops reading at the first match, and a
# ufw still writing is then killed by SIGPIPE, which `set -o pipefail` reports as "not active".
ufw_active() {
    local out
    command -v ufw >/dev/null 2>&1 || return 1
    out=$(ufw status 2>/dev/null) || return 1
    [[ "$out" == *"Status: active"* ]]
}

ufw_has_rule() { # ufw_has_rule PORT
    local out line
    out=$(ufw status 2>/dev/null) || return 1
    while IFS= read -r line; do
        case "$line" in "$1/tcp"*) return 0 ;; esac
    done <<<"$out"
    return 1
}

firewalld_running() {
    command -v firewall-cmd >/dev/null 2>&1 && firewall-cmd --state >/dev/null 2>&1
}

firewall_open() { # firewall_open PORT
    if ufw_active; then
        ufw allow "$1"/tcp >/dev/null && say "Opened port $1 in ufw"
    elif firewalld_running; then
        firewall-cmd --permanent --add-port="$1"/tcp >/dev/null
        firewall-cmd --reload >/dev/null
        say "Opened port $1 in firewalld"
    else
        say "No active ufw or firewalld rule was found. If you use another firewall, allow TCP $1."
    fi
}

firewall_close() { # firewall_close PORT
    if ufw_active; then
        if ufw_has_rule "$1"; then
            ufw delete allow "$1"/tcp >/dev/null 2>&1 && say "  Closed port $1 in ufw"
        fi
    elif firewalld_running; then
        if firewall-cmd --permanent --query-port="$1"/tcp >/dev/null 2>&1; then
            firewall-cmd --permanent --remove-port="$1"/tcp >/dev/null 2>&1 || true
            firewall-cmd --reload >/dev/null 2>&1 || true
            say "  Closed port $1 in firewalld"
        fi
    fi
}

# Sourced by the tests, which exercise the helpers above without installing anything.
if [ "${ELORA_INSTALLER_LIBRARY:-}" = "1" ]; then
    # shellcheck disable=SC2317  # `exit` is the fallback when the file is run, not sourced
    return 0 2>/dev/null || exit 0
fi

if [ "$(id -u)" -ne 0 ]; then
    fail "run this with root rights (for example: sudo bash $0)"
fi

command -v systemctl >/dev/null 2>&1 \
    || fail "this machine has no systemd, so the installer cannot register the service"
command -v runuser >/dev/null 2>&1 \
    || fail "runuser (part of util-linux) is missing, so the installer cannot switch users"
# Questions are asked on the terminal even when the script itself arrives on stdin
# (curl ... | sudo bash); with no terminal at all, the answers must come from the environment.
INTERACTIVE=no
if [ -t 0 ]; then
    exec 3<&0
    INTERACTIVE=yes
elif ( : </dev/tty ) 2>/dev/null; then
    exec 3</dev/tty
    INTERACTIVE=yes
fi

ask() { # ask VARIABLE "question" "default"
    local __answer=""
    if [ "$INTERACTIVE" = yes ]; then
        if [ -n "${3:-}" ]; then
            printf '%s [%s]: ' "$2" "$3" >&2
        else
            printf '%s: ' "$2" >&2
        fi
        IFS= read -r __answer <&3 || __answer=""
    fi
    printf -v "$1" '%s' "${__answer:-${3:-}}"
}

ask_secret() { # ask_secret VARIABLE "question"
    local __answer=""
    if [ "$INTERACTIVE" = yes ]; then
        printf '%s: ' "$2" >&2
        IFS= read -r -s __answer <&3 || __answer=""
        printf '\n' >&2
    fi
    printf -v "$1" '%s' "$__answer"
}

# --- is a server already installed here? ----------------------------------------------------
# Checked before anything is asked or downloaded: an update must never look like a fresh install,
# and uninstalling has to know where the old files and service are.
EXISTING_MODE=""
EXISTING_UNIT=""
EXISTING_DATA=""
EXISTING_BINARY=""
EXISTING_OWNER=""
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
for candidate in "$SYSTEM_DATA_DIR" "$USER_DATA_DIR"; do
    if [ -n "$candidate" ] && [ -f "$candidate/config.json" ]; then
        EXISTING_DATA="$candidate"
        break
    fi
done

EXISTING=no
if [ -n "$EXISTING_MODE$EXISTING_BINARY$EXISTING_DATA" ]; then
    EXISTING=yes
fi

# What the existing settings say, used for the summary, for the defaults and to close the port.
CUR_NAME=""
CUR_PORT=""
CUR_NOTES=""
CUR_BIND=""
if [ -n "$EXISTING_DATA" ]; then
    CUR_NAME=$(json_field "$EXISTING_DATA/config.json" name)
    CUR_PORT=$(json_field "$EXISTING_DATA/config.json" port)
    CUR_NOTES=$(json_field "$EXISTING_DATA/config.json" notes_dir)
    CUR_BIND=$(json_field "$EXISTING_DATA/config.json" bind)
fi

# ELORA_ACTION=update|uninstall|purge skips the question (for scripted installs).
ACTION=${ELORA_ACTION:-}
case "$ACTION" in
    "" | update | uninstall | purge) ;;
    *) fail "ELORA_ACTION must be 'update', 'uninstall' or 'purge', not '$ACTION'" ;;
esac

# A service this installer stopped, so it can be started again if the install fails half way.
STOPPED_UNIT=""
SERVICE_STARTED=no
WORK_DIR=""

start_stopped_service() {
    case "$STOPPED_UNIT" in
        system) systemctl start "$SERVICE_NAME" ;;
        user)
            runuser -u "$EXISTING_OWNER" -- env XDG_RUNTIME_DIR="/run/user/$(id -u "$EXISTING_OWNER")" \
                systemctl --user start "$SERVICE_NAME"
            ;;
    esac
}

on_exit() {
    local code=$?
    [ -z "$WORK_DIR" ] || rm -rf "$WORK_DIR"
    if [ "$code" -ne 0 ] && [ -n "$STOPPED_UNIT" ] && [ "$SERVICE_STARTED" != yes ]; then
        say "" >&2
        say "The installer stopped before it finished, so the server that was running is being started again." >&2
        start_stopped_service >/dev/null 2>&1 \
            || say "It could not be started from here; start it with: systemctl $([ "$STOPPED_UNIT" = user ] && echo --user) start elora-server" >&2
    fi
}
trap on_exit EXIT

stop_existing_service() {
    if [ -f "$SYSTEM_UNIT_PATH" ] && systemctl is-active --quiet "$SERVICE_NAME" 2>/dev/null; then
        systemctl stop "$SERVICE_NAME" || fail "could not stop the running server"
        STOPPED_UNIT=system
    fi
    if [ -n "$USER_UNIT_PATH" ] && [ -f "$USER_UNIT_PATH" ] && [ -n "$EXISTING_OWNER" ]; then
        local runtime
        runtime="/run/user/$(id -u "$EXISTING_OWNER")"
        if runuser -u "$EXISTING_OWNER" -- env XDG_RUNTIME_DIR="$runtime" \
            systemctl --user is-active --quiet "$SERVICE_NAME" 2>/dev/null; then
            runuser -u "$EXISTING_OWNER" -- env XDG_RUNTIME_DIR="$runtime" \
                systemctl --user stop "$SERVICE_NAME" || fail "could not stop the running server"
            STOPPED_UNIT=user
        fi
    fi
}

if [ "$EXISTING" = "yes" ]; then
    NOTE_COUNT=0
    if [ -n "$EXISTING_DATA" ]; then
        NOTE_COUNT=$(find "${CUR_NOTES:-$EXISTING_DATA/notes}" -maxdepth 1 -name '*.md' 2>/dev/null | wc -l || true)
    fi
    CODE=""
    OLD_VERSION=""
    if [ -n "$EXISTING_BINARY" ]; then
        OLD_VERSION=$("$EXISTING_BINARY" version 2>/dev/null | head -n 1 || true)
        if [ -n "$EXISTING_DATA" ]; then
            CODE=$(ELORA_SERVER_DIR="$EXISTING_DATA" "$EXISTING_BINARY" fingerprint 2>/dev/null |
                head -n 1 || true)
        fi
    fi

    say ""
    say "EloraNotes server is already installed on this machine:"
    if [ -n "$EXISTING_MODE" ]; then
        say "  service:  $EXISTING_UNIT ($EXISTING_MODE service)"
    else
        say "  service:  none found"
    fi
    if [ -n "$EXISTING_BINARY" ]; then
        say "  binary:   $EXISTING_BINARY${OLD_VERSION:+ ($OLD_VERSION)}"
    fi
    if [ -n "$EXISTING_DATA" ]; then
        say "  settings: $EXISTING_DATA${CUR_NAME:+  (\"$CUR_NAME\", port ${CUR_PORT:-?})}"
        say "  notes:    $NOTE_COUNT in ${CUR_NOTES:-$EXISTING_DATA/notes}"
    fi
    [ -z "$CODE" ] || say "  code:     $CODE (clients already trust this certificate)"

    if [ -z "$ACTION" ]; then
        if [ "$INTERACTIVE" = yes ]; then
            say ""
            say "What would you like to do?"
            say "  1) Update it, keeping the settings, the notes and the certificate  (recommended)"
            say "  2) Uninstall it, keeping the notes on disk"
            say "  3) Uninstall it and delete the notes and certificate"
            say "  4) Cancel"
            while true; do
                printf 'Choice [1]: ' >&2
                IFS= read -r CHOICE <&3 \
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
        # The port this server opened is closed again; nothing else is listening there now.
        if valid_port "${CUR_PORT:-x}"; then
            firewall_close "$CUR_PORT"
        fi

        if [ "$ACTION" = "purge" ]; then
            if [ -n "$EXISTING_DATA" ] && [ -d "$EXISTING_DATA" ]; then
                rm -rf "$EXISTING_DATA" && say "  Deleted $EXISTING_DATA"
            fi
            # Notes kept somewhere else by hand-edited settings are not this installer's to delete.
            if [ -n "$CUR_NOTES" ] && [ -d "$CUR_NOTES" ]; then
                say "  Left $CUR_NOTES alone: the settings pointed the notes outside the server folder."
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
                say "The server is gone; the notes are still in ${CUR_NOTES:-$EXISTING_DATA/notes}."
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

# --- the binary ----------------------------------------------------------------------------
# Fetched, checked and test-run before anything on this machine is touched, so a failed download
# can never replace a server that works. The download goes to a temporary folder: writing next to
# the destination fails on a read-only filesystem, and a half-written file must never be installed.
WORK_DIR=$(mktemp -d "${TMPDIR:-/tmp}/elora-server.XXXXXX") \
    || fail "could not create a temporary folder in ${TMPDIR:-/tmp}"

# A script piped into bash has no file, and so no folder to look in for a binary beside it.
SCRIPT_DIR=""
if [ -n "${BASH_SOURCE[0]:-}" ] && [ -f "${BASH_SOURCE[0]}" ]; then
    SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
fi
LOCAL_BINARY=""
for candidate in "$SCRIPT_DIR/elora-server" "$SCRIPT_DIR/$ASSET"; do
    if [ -n "$SCRIPT_DIR" ] && [ -f "$candidate" ]; then
        LOCAL_BINARY="$candidate"
        break
    fi
done

STAGED="$WORK_DIR/elora-server"
if [ -n "$LOCAL_BINARY" ]; then
    say "Using $LOCAL_BINARY"
    cp -- "$LOCAL_BINARY" "$STAGED"
else
    case "$(uname -m)" in
        x86_64 | amd64) ;;
        *) fail "the released server is built for x86_64 and this machine is $(uname -m); put an elora-server binary built for it next to this script" ;;
    esac
    free_mb=$(df -Pk "$WORK_DIR" | awk 'NR == 2 { print int($4 / 1024) }')
    if [ -n "$free_mb" ] && [ "$free_mb" -lt 20 ]; then
        fail "only ${free_mb} MB free in $WORK_DIR; the server needs about 20 MB while it downloads"
    fi
    say "Downloading the server from ${ELORA_VERSION:+release v${ELORA_VERSION#v} of }the EloraNotes releases…"
    fetch "$RELEASE_BASE/$ASSET" "$STAGED" \
        || fail "could not download $RELEASE_BASE/$ASSET (check that this machine can reach github.com; you can also put the elora-server binary next to this script and run it again)"
    # The release lists the checksum of every file it carries; the download has to match it.
    fetch "$RELEASE_BASE/SHA256SUMS" "$WORK_DIR/SHA256SUMS" \
        || fail "could not download the checksum list $RELEASE_BASE/SHA256SUMS, so the download cannot be verified"
    WANT=$(listed_sha256 "$WORK_DIR/SHA256SUMS" "$ASSET")
    [ -n "$WANT" ] || fail "the release's checksum list has no entry for $ASSET"
    HAVE=$(sha256_of "$STAGED") \
        || fail "no sha256sum, shasum or openssl here to verify the download with"
    if [ "$HAVE" != "$WANT" ]; then
        fail "the downloaded server does not match its published checksum (expected $WANT, got $HAVE); nothing was installed"
    fi
    say "Checked the download against the release's checksum."
fi

[ -s "$STAGED" ] || fail "the server binary is empty; something went wrong while fetching it"
chmod 0755 "$STAGED"
NEW_VERSION=$("$STAGED" version 2>/dev/null | head -n 1) \
    || fail "the downloaded server does not run on this machine (wrong CPU architecture or a corrupt download); nothing was changed"
[ -n "$NEW_VERSION" ] || fail "the downloaded server does not run on this machine; nothing was changed"

# --- what the server should be called -------------------------------------------------------
# An existing install keeps its settings, its password included, unless a person asks to change
# them or the environment supplies new ones: an update is about the program, not the setup.
RECONFIGURE=yes
if [ -n "$EXISTING_DATA" ] && [ -n "$CUR_NAME" ] && valid_port "${CUR_PORT:-x}"; then
    RECONFIGURE=no
    if [ -n "${ELORA_NAME:-}${ELORA_PORT:-}${ELORA_PASSWORD:-}${ELORA_PASSWORD_FILE:-}" ]; then
        RECONFIGURE=yes
    elif [ "$INTERACTIVE" = yes ]; then
        say ""
        ask KEEP "Keep the current settings (name \"$CUR_NAME\", port $CUR_PORT, same password)? [Y/n]" "y"
        case "$KEEP" in [nN]*) RECONFIGURE=yes ;; esac
    fi
fi

if [ "$RECONFIGURE" = yes ]; then
    if [ -n "${ELORA_NAME:-}" ]; then SERVER_NAME=$ELORA_NAME; else
        ask SERVER_NAME "Server name (shown in everyone's sidebar)" "${CUR_NAME:-EloraNotes}"
    fi
    SERVER_NAME=${SERVER_NAME//$'\n'/ }
    [ -n "$SERVER_NAME" ] || SERVER_NAME=EloraNotes

    PORT=${ELORA_PORT:-}
    while true; do
        if [ -z "$PORT" ]; then
            ask PORT "Port" "${CUR_PORT:-7331}"
        fi
        if ! [[ "$PORT" =~ ^[0-9]+$ ]] || [ "$((10#$PORT))" -lt 1 ] || [ "$((10#$PORT))" -gt 65535 ]; then
            [ "$INTERACTIVE" = yes ] || fail "ELORA_PORT '$PORT' is not a port number"
            say "  That is not a port number."
            PORT=""
            continue
        fi
        if [ "$((10#$PORT))" -lt 1024 ]; then
            [ "$INTERACTIVE" = yes ] || fail "ELORA_PORT must be 1024 or higher"
            say "  Ports below 1024 are reserved; pick a higher one."
            PORT=""
            continue
        fi
        PORT=$((10#$PORT))
        break
    done

    if [ -n "${ELORA_PASSWORD_FILE:-}" ]; then
        [ -r "$ELORA_PASSWORD_FILE" ] || fail "cannot read ELORA_PASSWORD_FILE ($ELORA_PASSWORD_FILE)"
        PASSWORD=$(head -n 1 -- "$ELORA_PASSWORD_FILE")
    elif [ -n "${ELORA_PASSWORD:-}" ]; then
        PASSWORD=$ELORA_PASSWORD
    elif [ "$INTERACTIVE" = yes ]; then
        ask_secret PASSWORD "Password everyone will connect with"
        ask_secret PASSWORD_AGAIN "Repeat the password"
        [ "$PASSWORD" = "$PASSWORD_AGAIN" ] || fail "the passwords did not match"
    else
        fail "there is no terminal to ask for the password; set ELORA_PASSWORD or ELORA_PASSWORD_FILE"
    fi
    unset ELORA_PASSWORD
    [ -n "$PASSWORD" ] || fail "the password cannot be empty"
else
    SERVER_NAME=$CUR_NAME
    PORT=$((10#$CUR_PORT))
    PASSWORD=""
fi

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
    TARGET_GROUP=""
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

# --- stop the old server, then check the port is free ---------------------------------------
# A running server keeps its old program and old settings in memory, so an update has to stop it
# and start it again; it also has to be gone before its port can be tested.
stop_existing_service
if port_listening "$PORT"; then
    OWNER=$(port_owner "$PORT")
    fail "port $PORT is already in use${OWNER:+ by $OWNER}; stop that program or run the installer again and pick another port"
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
install -m 0755 "$STAGED" "$BINARY" || fail "could not install the server to $BINARY"

# --- the account, the folder, the settings -------------------------------------------------
# The settings are only written when they are new or changed; an update leaves them untouched.
# When they are rewritten, a notes folder or bind address that was edited in by hand is kept.
INIT_EXTRA=()
if [ "$RECONFIGURE" = yes ] && [ "$EXISTING_DATA" = "$DATA_DIR" ]; then
    [ -z "$CUR_NOTES" ] || INIT_EXTRA+=(--notes-dir "$CUR_NOTES")
    [ -z "$CUR_BIND" ] || INIT_EXTRA+=(--bind "$CUR_BIND")
fi

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
    if [ "$RECONFIGURE" = yes ]; then
        say "Writing the settings and the certificate…"
        printf '%s\n' "$PASSWORD" | ELORA_SERVER_DIR="$DATA_DIR" \
            "$BINARY" init --name "$SERVER_NAME" --port "$PORT" --password-stdin ${INIT_EXTRA[@]+"${INIT_EXTRA[@]}"} \
            || fail "could not write the settings"
    fi
    chown -R "$SERVICE_USER:$SERVICE_GROUP" "$DATA_DIR"
    UNIT_USER="User=$SERVICE_USER"
    UNIT_GROUP="Group=$SERVICE_GROUP"
    UNIT_WANTED="WantedBy=multi-user.target"
    # The server only needs the network and its own folder, so everything else is closed off.
    UNIT_HARDENING="ProtectSystem=full
ProtectHome=yes
PrivateTmp=yes
PrivateDevices=yes
NoNewPrivileges=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectControlGroups=yes
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX
RestrictNamespaces=yes
RestrictRealtime=yes
RestrictSUIDSGID=yes
LockPersonality=yes
SystemCallArchitectures=native"
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
    if [ "$RECONFIGURE" = yes ]; then
        say "Writing the settings and the certificate…"
        printf '%s\n' "$PASSWORD" | runuser -u "$TARGET_USER" -- \
            env ELORA_SERVER_DIR="$DATA_DIR" \
            "$BINARY" init --name "$SERVER_NAME" --port "$PORT" --password-stdin ${INIT_EXTRA[@]+"${INIT_EXTRA[@]}"} \
            || fail "could not write the settings as $TARGET_USER"
    fi
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
PASSWORD=""

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
    echo "Description=EloraNotes collaboration server ($(unit_escape "$SERVER_NAME"))"
    echo "After=network-online.target"
    echo "Wants=network-online.target"
    echo ""
    echo "[Service]"
    echo "Type=simple"
    [ -z "$UNIT_USER" ] || echo "$UNIT_USER"
    [ -z "$UNIT_GROUP" ] || echo "$UNIT_GROUP"
    echo "Environment=\"ELORA_SERVER_DIR=$(unit_escape "$DATA_DIR")\""
    echo "ExecStart=\"$(unit_escape "$BINARY")\" run"
    echo "Restart=on-failure"
    echo "RestartSec=3"
    printf '%s\n' "$UNIT_HARDENING"
    echo ""
    echo "[Install]"
    echo "$UNIT_WANTED"
} >"$UNIT_PATH"

# `enable --now` starts a stopped service but leaves a running one alone, which is how an update
# used to keep serving the old program; an explicit restart always runs the new one.
started=""
if [ "$SERVICE_MODE" = "system" ]; then
    systemctl daemon-reload
    systemctl enable "$SERVICE_NAME" >/dev/null 2>&1 || true
    systemctl restart "$SERVICE_NAME" || true
else
    chown "$TARGET_USER:$TARGET_GROUP" "$UNIT_PATH"
    RUNTIME_DIR="/run/user/$(id -u "$TARGET_USER")"
    loginctl enable-linger "$TARGET_USER" >/dev/null 2>&1 \
        || say "Could not enable lingering; the server would only run while $TARGET_USER is logged in."
    if runuser -u "$TARGET_USER" -- env XDG_RUNTIME_DIR="$RUNTIME_DIR" \
        systemctl --user daemon-reload 2>/dev/null; then
        runuser -u "$TARGET_USER" -- env XDG_RUNTIME_DIR="$RUNTIME_DIR" \
            systemctl --user enable "$SERVICE_NAME" >/dev/null 2>&1 || true
        runuser -u "$TARGET_USER" -- env XDG_RUNTIME_DIR="$RUNTIME_DIR" \
            systemctl --user restart "$SERVICE_NAME" \
            || say "Could not start the user service from here; see the commands at the end."
    else
        say "The service manager for $TARGET_USER is not reachable from here."
    fi
fi

# Up means accepting connections, not "the process exists": a server that cannot bind its port or
# read its settings exits a moment after systemd calls it active.
if wait_for_port "$PORT" "$HEALTH_WAIT"; then
    started="yes"
    SERVICE_STARTED=yes
elif [ "$SERVICE_MODE" = "system" ]; then
    say "The server did not start answering on port $PORT; recent log lines:"
    journalctl -u "$SERVICE_NAME" -n 20 --no-pager || true
    fail "fix the error above, then run: systemctl restart elora-server"
else
    say "The server did not start answering on port $PORT; recent log lines:"
    runuser -u "$TARGET_USER" -- env XDG_RUNTIME_DIR="/run/user/$(id -u "$TARGET_USER")" \
        journalctl --user -u "$SERVICE_NAME" -n 20 --no-pager || true
    say ""
    say "Start it by hand with:"
    say "  systemctl --user daemon-reload && systemctl --user restart elora-server"
fi

# --- firewall ------------------------------------------------------------------------------
# A changed port leaves the old one open otherwise.
if [ -n "$CUR_PORT" ] && valid_port "$CUR_PORT" && [ "$((10#$CUR_PORT))" -ne "$PORT" ]; then
    firewall_close "$CUR_PORT"
fi
firewall_open "$PORT"

# --- what to tell everyone else ------------------------------------------------------------
FINGERPRINT=$("$BINARY" fingerprint 2>/dev/null | head -n 1 || true)
LOCAL_IP=$(ip -4 route get 1.1.1.1 2>/dev/null | awk '{for (i = 1; i < NF; i++) if ($i == "src") print $(i + 1)}' | head -n 1)

say ""
if [ -n "$started" ]; then
    say "EloraNotes server \"$SERVER_NAME\" is running on port $PORT ($NEW_VERSION)."
else
    say "EloraNotes server \"$SERVER_NAME\" is installed ($NEW_VERSION) but is not answering on port $PORT yet."
fi
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
say "Notes live in ${CUR_NOTES:-$DATA_DIR/notes}."
if [ "$SERVICE_MODE" = "system" ]; then
    say "Manage the service with:"
    say "  systemctl status elora-server     systemctl restart elora-server"
    say "  journalctl -u elora-server -f     (live log)"
else
    say "It runs as a per-user service for $TARGET_USER, so manage it from your own shell:"
    say "  systemctl --user status elora-server     systemctl --user restart elora-server"
    say "  journalctl --user -u elora-server -f     (live log)"
fi
say "Update later by running this installer again; to remove it, run it again and choose Uninstall."

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
    say "      User = \"${TARGET_USER:-$SERVICE_USER}\";"
    say "    };"
    say "  };"
fi
