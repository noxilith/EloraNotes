#!/usr/bin/env bash
# Runs install-linux.sh end to end against stand-ins for the system tools.
#
#   ELORA_SERVER_BINARY=target/debug/elora-server packaging/server/tests/test-install-linux.sh
#
# `systemctl`, `useradd`, `ufw` and friends are replaced by small scripts that record what they
# were asked to do. The fake `systemctl` really starts the server from the unit file the installer
# writes, so the unit's quoting, the restart on update and the "is it answering" check are all
# exercised for real. Nothing outside a temporary folder is touched.
set -uo pipefail

HERE=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
INSTALLER="$HERE/../install-linux.sh"
SERVER_BINARY=${ELORA_SERVER_BINARY:-}
[ -x "$SERVER_BINARY" ] || { echo "set ELORA_SERVER_BINARY to a built elora-server"; exit 2; }
SERVER_BINARY=$(cd -- "$(dirname -- "$SERVER_BINARY")" && pwd)/$(basename -- "$SERVER_BINARY")
REAL_PATH=$PATH

T=$(mktemp -d "${TMPDIR:-/tmp}/elora-installer-test.XXXXXX")
cleanup() {
    [ -f "$T/server.pid" ] && kill "$(cat "$T/server.pid")" 2>/dev/null
    [ -f "$T/listener.pid" ] && kill "$(cat "$T/listener.pid")" 2>/dev/null
    rm -rf "$T"
}
[ "${KEEP:-}" = 1 ] && trap 'echo "kept: $T"' EXIT || # KEEP=1 leaves the temporary folder behind for looking at what a failing check saw.
if [ "${KEEP:-}" = 1 ]; then trap 'echo "kept: $T"' EXIT; else trap cleanup EXIT; fi

PASSED=0
FAILED=0
check() { # check "what" command...
    local what=$1
    shift
    if "$@" >/dev/null 2>&1; then
        PASSED=$((PASSED + 1))
        printf '  ok    %s\n' "$what"
    else
        FAILED=$((FAILED + 1))
        printf '  FAIL  %s\n' "$what"
    fi
}
section() { printf '\n%s\n' "$1"; }

# --- stand-ins ------------------------------------------------------------------------------
mkdir -p "$T/bin" "$T/home/tester" "$T/release"
export ELORA_TEST_DIR=$T ELORA_REAL_PATH=$REAL_PATH
: >"$T/calls"
: >"$T/users"
: >"$T/groups"
: >"$T/ufw.rules"

stub() { # stub NAME  (body on stdin)
    { echo '#!/usr/bin/env bash'; cat; } >"$T/bin/$1"
    chmod +x "$T/bin/$1"
}

stub id <<'EOF'
case "$*" in
    "-u") echo 0 ;;
    "-u elora") grep -qx elora "$ELORA_TEST_DIR/users" && echo 998 || exit 1 ;;
    "-u tester") echo 1000 ;;
    "-gn elora" | "-gn tester") echo "${2}" ;;
    *) PATH=$ELORA_REAL_PATH exec id "$@" ;;
esac
EOF
stub getent <<'EOF'
case "$1 $2" in
    "group elora") grep -qx elora "$ELORA_TEST_DIR/groups" && echo "elora:x:998:" || exit 2 ;;
    "passwd tester") echo "tester:x:1000:1000::$ELORA_TEST_DIR/home/tester:/bin/bash" ;;
    "passwd elora") grep -qx elora "$ELORA_TEST_DIR/users" && echo "elora:x:998:998::/var/lib/elora-server:/usr/sbin/nologin" || exit 2 ;;
    *) exit 2 ;;
esac
EOF
stub groupadd <<'EOF'
echo "groupadd $*" >>"$ELORA_TEST_DIR/calls"; echo elora >>"$ELORA_TEST_DIR/groups"
EOF
stub useradd <<'EOF'
echo "useradd $*" >>"$ELORA_TEST_DIR/calls"; echo elora >>"$ELORA_TEST_DIR/users"
EOF
stub usermod <<'EOF'
echo "usermod $*" >>"$ELORA_TEST_DIR/calls"
EOF
stub userdel <<'EOF'
echo "userdel $*" >>"$ELORA_TEST_DIR/calls"; sed -i '/^elora$/d' "$ELORA_TEST_DIR/users"
EOF
stub groupdel <<'EOF'
echo "groupdel $*" >>"$ELORA_TEST_DIR/calls"; sed -i '/^elora$/d' "$ELORA_TEST_DIR/groups"
EOF
stub chown <<'EOF'
echo "chown $*" >>"$ELORA_TEST_DIR/calls"
EOF
stub loginctl <<'EOF'
echo "loginctl $*" >>"$ELORA_TEST_DIR/calls"
EOF
stub journalctl <<'EOF'
echo "journalctl $*" >>"$ELORA_TEST_DIR/calls"
EOF
stub runuser <<'EOF'
# runuser -u USER -- COMMAND...
shift 3
exec "$@"
EOF
# The real `install` cannot change owners without root; the rest of its job is kept.
stub install <<'EOF'
args=()
while [ $# -gt 0 ]; do
    case "$1" in -o | -g) shift 2 ;; *) args+=("$1"); shift ;; esac
done
PATH=$ELORA_REAL_PATH exec install "${args[@]}"
EOF
stub ufw <<'EOF'
T=$ELORA_TEST_DIR
echo "ufw $*" >>"$T/calls"
case "$1" in
    status) echo "Status: active"; cat "$T/ufw.rules" ;;
    allow) echo "$2 ALLOW Anywhere" >>"$T/ufw.rules" ;;
    delete) grep -vF "$3 ALLOW" "$T/ufw.rules" >"$T/ufw.new"; mv "$T/ufw.new" "$T/ufw.rules" ;;
esac
EOF
# A systemd that starts the server the unit file describes, the way systemd would read it.
stub systemctl <<'EOF'
T=$ELORA_TEST_DIR
cmd=""
for a in "$@"; do case "$a" in -*) ;; *) cmd=$a; break ;; esac; done
echo "systemctl $*" >>"$T/calls"
unit="${ELORA_STUB_UNIT:-$ELORA_UNIT_DIR/elora-server.service}"
alive() { [ -f "$T/server.pid" ] && kill -0 "$(cat "$T/server.pid")" 2>/dev/null; }
stop_server() { if alive; then kill "$(cat "$T/server.pid")"; sleep 0.3; fi; rm -f "$T/server.pid"; }
start_server() {
    [ -f "$unit" ] || exit 5
    dir=$(sed -n 's/^Environment="ELORA_SERVER_DIR=\(.*\)"$/\1/p' "$unit" | sed 's/%%/%/g')
    bin=$(sed -n 's/^ExecStart="\(.*\)" run$/\1/p' "$unit" | sed 's/%%/%/g')
    [ -n "$dir" ] && [ -x "$bin" ] || exit 6
    ELORA_SERVER_DIR=$dir setsid "$bin" run >"$T/server.log" 2>&1 </dev/null &
    echo $! >"$T/server.pid"
}
case "$cmd" in
    is-active) alive ;;
    stop) stop_server ;;
    start | restart) stop_server; start_server ;;
    *) exit 0 ;;
esac
EOF

free_port() { python3 -I -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1])'; }
sha_of() { sha256sum "$1" | awk '{ print $1 }'; }
calls() { cat "$T/calls"; }

# A release folder the installer can download from (file:// works with curl).
publish() { # publish BINARY_FILE [SUMS_FILE_CONTENT_SOURCE]
    cp "$1" "$T/release/elora-server-linux-x86_64"
    { echo "$(sha_of "${2:-$1}")  elora-server-linux-x86_64"; echo "0000  something-else"; } >"$T/release/SHA256SUMS"
}

# Runs the installer with no terminal, so every answer has to come from the environment.
install_run() { # install_run [VAR=value ...]  -> output in $T/out, status in $?
    ( cd "$T" && env -i HOME="$T/home/tester" PATH="$T/bin:$REAL_PATH" TMPDIR="$T" \
        ELORA_TEST_DIR="$T" ELORA_REAL_PATH="$REAL_PATH" \
        ELORA_UNIT_DIR="$T/units" ELORA_SERVER_DATA_DIR="$T/data" ELORA_SERVER_BIN="$T/usr/bin/elora-server" \
        ELORA_RELEASE_BASE="file://$T/release" ELORA_SERVICE_MODE=system ELORA_HEALTH_WAIT=10 \
        "$@" setsid -w bash "$INSTALLER" </dev/null ) >"$T/out" 2>&1
}

# The same, as a per-user service for the account "tester" (what an immutable system gets).
install_run_user() {
    ( cd "$T" && env -i HOME="$T/home/tester" PATH="$T/bin:$REAL_PATH" TMPDIR="$T" \
        ELORA_TEST_DIR="$T" ELORA_REAL_PATH="$REAL_PATH" SUDO_USER=tester \
        ELORA_STUB_UNIT="$T/home/tester/.config/systemd/user/elora-server.service" \
        ELORA_SERVER_BIN="$T/usr/bin/elora-server" \
        ELORA_RELEASE_BASE="file://$T/release" ELORA_SERVICE_MODE=user ELORA_HEALTH_WAIT=10 \
        "$@" setsid -w bash "$INSTALLER" </dev/null ) >"$T/out" 2>&1
}

NAME='Test "Srv" 100%'
PASSWORD='pässwörd'
PORT=$(free_port)
publish "$SERVER_BINARY"

# --- the helpers on their own ---------------------------------------------------------------
section "helpers"
# shellcheck disable=SC1090
( ELORA_INSTALLER_LIBRARY=1 source "$INSTALLER"
  [ "$(unit_escape '100% safe')" = '100%% safe' ] ) && check "a % in a name is doubled for systemd" true || check "a % in a name is doubled for systemd" false
cat >"$T/sample.json" <<'EOF'
{
  "name": "My \"Server\" \\ ok",
  "port": 7777,
  "notes_dir": "/srv/notes dir",
  "bind": "0.0.0.0"
}
EOF
# shellcheck disable=SC1090
( ELORA_INSTALLER_LIBRARY=1 source "$INSTALLER"
  [ "$(json_field "$T/sample.json" name)" = 'My "Server" \ ok' ] &&
  [ "$(json_field "$T/sample.json" port)" = 7777 ] &&
  [ "$(json_field "$T/sample.json" notes_dir)" = '/srv/notes dir' ] &&
  [ -z "$(json_field "$T/sample.json" missing)" ] ) && check "config.json values are read, escapes included" true || check "config.json values are read, escapes included" false
# shellcheck disable=SC1090
( ELORA_INSTALLER_LIBRARY=1 source "$INSTALLER"
  valid_port 1024 && valid_port 65535 && valid_port 07331 &&
  ! valid_port 1023 && ! valid_port 65536 && ! valid_port abc && ! valid_port "" ) && check "port validation" true || check "port validation" false
# shellcheck disable=SC1090
( ELORA_INSTALLER_LIBRARY=1 source "$INSTALLER"
  printf 'AAA  elora-server-linux-x86_64\nBBB *other\n' >"$T/sums"
  [ "$(listed_sha256 "$T/sums" elora-server-linux-x86_64)" = aaa ] &&
  [ "$(listed_sha256 "$T/sums" other)" = bbb ] &&
  [ -z "$(listed_sha256 "$T/sums" nope)" ] ) && check "the checksum list is read (binary-mode stars too)" true || check "the checksum list is read (binary-mode stars too)" false

# --- a fresh install ------------------------------------------------------------------------
section "fresh install (system service, answers from the environment)"
install_run ELORA_NAME="$NAME" ELORA_PORT="$PORT" ELORA_PASSWORD="$PASSWORD"
STATUS=$?
check "it succeeds" test "$STATUS" -eq 0
check "the download was checked against the release's checksum" grep -q "Checked the download" "$T/out"
check "the binary is installed and executable" test -x "$T/usr/bin/elora-server"
check "the settings were written with the chosen name, port and password" \
    bash -c "grep -q '\"port\": $PORT' '$T/data/config.json' && grep -q 'pässwörd' '$T/data/config.json' && grep -q 'Srv' '$T/data/config.json'"
check "a certificate exists" test -f "$T/data/cert.pem"
check "the unit doubles the % in the name" grep -q 'Test "Srv" 100%%' "$T/units/elora-server.service"
check "the unit quotes the paths" grep -q "^ExecStart=\"$T/usr/bin/elora-server\" run" "$T/units/elora-server.service"
check "the unit is sandboxed" grep -q '^RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX' "$T/units/elora-server.service"
check "the server really answers on the port" bash -c "( exec 9<>/dev/tcp/127.0.0.1/$PORT )"
check "the summary says it is running" grep -q "is running on port $PORT" "$T/out"
check "the port was opened in the firewall" grep -q "^$PORT/tcp" "$T/ufw.rules"
check "the password is never printed" bash -c "! grep -q 'pässwörd' '$T/out'"
FIRST_PID=$(cat "$T/server.pid")
CONFIG_SUM=$(sha_of "$T/data/config.json")

# --- an update keeps everything -------------------------------------------------------------
section "update with no answers (keeps the settings, restarts the server)"
sleep 1
install_run ELORA_ACTION=update
STATUS=$?
check "it succeeds" test "$STATUS" -eq 0
check "the settings file is untouched" test "$(sha_of "$T/data/config.json")" = "$CONFIG_SUM"
check "no password was asked for" bash -c "! grep -qi 'password everyone' '$T/out'"
check "the server was restarted (a new process)" bash -c "[ \"\$(cat '$T/server.pid')\" != '$FIRST_PID' ]"
check "and it answers again" bash -c "( exec 9<>/dev/tcp/127.0.0.1/$PORT )"
check "the service was explicitly restarted, not just enabled" grep -q "^systemctl restart elora-server.service" "$T/calls"

# --- a bad download never replaces a working server ----------------------------------------
section "a corrupted download is refused"
BAD="$T/corrupt"
cp "$SERVER_BINARY" "$BAD"
printf 'tampered' >>"$BAD"
cp "$BAD" "$T/release/elora-server-linux-x86_64"      # the listed checksum is still the good one
OLD_SUM=$(sha_of "$T/usr/bin/elora-server")
PID_BEFORE=$(cat "$T/server.pid")
install_run ELORA_ACTION=update
STATUS=$?
check "the installer fails" test "$STATUS" -ne 0
check "it says the checksum does not match" grep -q "does not match its published checksum" "$T/out"
check "the installed server is unchanged" test "$(sha_of "$T/usr/bin/elora-server")" = "$OLD_SUM"
check "the running server was never stopped" bash -c "[ \"\$(cat '$T/server.pid')\" = '$PID_BEFORE' ] && kill -0 $PID_BEFORE"

section "a download that does not run is refused before anything is touched"
printf '#!/bin/sh\nexit 1\n' >"$T/notaserver"
chmod +x "$T/notaserver"
publish "$T/notaserver"
install_run ELORA_ACTION=update
STATUS=$?
check "the installer fails" test "$STATUS" -ne 0
check "it says nothing was changed" grep -q "nothing was changed" "$T/out"
check "the installed server is unchanged" test "$(sha_of "$T/usr/bin/elora-server")" = "$OLD_SUM"
check "the running server was never stopped" bash -c "kill -0 $PID_BEFORE"
publish "$SERVER_BINARY"

section "a missing checksum list is refused"
rm -f "$T/release/SHA256SUMS"
install_run ELORA_ACTION=update
check "the installer fails" test "$?" -ne 0
check "it says the download cannot be verified" grep -q "cannot be verified" "$T/out"
publish "$SERVER_BINARY"

# --- reconfiguring --------------------------------------------------------------------------
section "changing the port (keeps a notes folder edited in by hand, moves the firewall rule)"
mkdir -p "$T/custom-notes"
sed -i "s#\"notes_dir\": \".*\"#\"notes_dir\": \"$T/custom-notes\"#" "$T/data/config.json"
NEW_PORT=$(free_port)
install_run ELORA_PORT="$NEW_PORT" ELORA_PASSWORD="another one"
STATUS=$?
check "it succeeds" test "$STATUS" -eq 0
check "the new port is in the settings" grep -q "\"port\": $NEW_PORT" "$T/data/config.json"
check "the custom notes folder survived" grep -q "custom-notes" "$T/data/config.json"
check "the old firewall rule was closed" bash -c "! grep -q '^$PORT/tcp' '$T/ufw.rules'"
check "the new one was opened" grep -q "^$NEW_PORT/tcp" "$T/ufw.rules"
check "the server answers on the new port" bash -c "( exec 9<>/dev/tcp/127.0.0.1/$NEW_PORT )"
PORT=$NEW_PORT

# --- uninstalling ---------------------------------------------------------------------------
section "uninstall keeps the notes and closes the port"
echo "keep me" >"$T/custom-notes/precious.md"
install_run ELORA_ACTION=uninstall
STATUS=$?
check "it succeeds" test "$STATUS" -eq 0
check "the unit is gone" test ! -e "$T/units/elora-server.service"
check "the binary is gone" test ! -e "$T/usr/bin/elora-server"
check "the firewall rule is gone" bash -c "! grep -q '^$PORT/tcp' '$T/ufw.rules'"
check "the notes are still there" test -f "$T/custom-notes/precious.md"
check "the settings are still there" test -f "$T/data/config.json"

section "purge removes the settings and the service account"
install_run ELORA_NAME=Again ELORA_PORT="$PORT" ELORA_PASSWORD=secret
check "a reinstall works" test "$?" -eq 0
install_run ELORA_ACTION=purge
check "purge succeeds" test "$?" -eq 0
check "the settings folder is gone" test ! -e "$T/data"
check "the service account was removed" grep -q '^userdel elora' "$T/calls"
check "notes outside the server folder are left alone and it says so" grep -q "Left $T/custom-notes alone" "$T/out"

# --- refusals -------------------------------------------------------------------------------
section "refusals"
install_run ELORA_NAME=x ELORA_PORT="$PORT"
check "no password and no terminal: it fails and says which variable to set" bash -c "grep -q 'ELORA_PASSWORD' '$T/out' && ! grep -q 'is running' '$T/out'"
install_run ELORA_NAME=x ELORA_PORT=80 ELORA_PASSWORD=p
check "a port below 1024 is refused" grep -q "1024 or higher" "$T/out"
install_run ELORA_NAME=x ELORA_PORT=abc ELORA_PASSWORD=p
check "a port that is not a number is refused" grep -q "not a port number" "$T/out"
install_run ELORA_ACTION=sideways
check "an unknown action is refused" grep -q "ELORA_ACTION must be" "$T/out"

python3 -I -c "
import socket,time,os,sys
s=socket.socket(); s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1); s.bind(('0.0.0.0',$PORT)); s.listen()
time.sleep(60)" &
echo $! >"$T/listener.pid"
sleep 0.5
rm -rf "${T:?}/data" "${T:?}/units" "${T:?}/usr"
install_run ELORA_NAME=x ELORA_PORT="$PORT" ELORA_PASSWORD=p
check "a port that is already in use is refused, naming the problem" grep -q "port $PORT is already in use" "$T/out"
check "and nothing was installed" test ! -e "$T/usr/bin/elora-server"
kill "$(cat "$T/listener.pid")" 2>/dev/null
rm -f "$T/listener.pid"

# --- the script arriving on stdin -----------------------------------------------------------
section "run as 'curl ... | sudo bash' (the script is on stdin, so it has no file of its own)"
rm -rf "${T:?}/data" "${T:?}/units" "${T:?}/usr"
( cd "$T" && env -i HOME="$T/home/tester" PATH="$T/bin:$REAL_PATH" TMPDIR="$T" \
    ELORA_TEST_DIR="$T" ELORA_REAL_PATH="$REAL_PATH" \
    ELORA_UNIT_DIR="$T/units" ELORA_SERVER_DATA_DIR="$T/data" ELORA_SERVER_BIN="$T/usr/bin/elora-server" \
    ELORA_RELEASE_BASE="file://$T/release" ELORA_SERVICE_MODE=system ELORA_HEALTH_WAIT=10 \
    ELORA_NAME=Piped ELORA_PORT="$PORT" ELORA_PASSWORD=pw setsid -w bash -s <"$INSTALLER" ) >"$T/out" 2>&1
check "it installs" grep -q "is running on port $PORT" "$T/out"

# --- a per-user service ---------------------------------------------------------------------
section "per-user service (read-only /etc): install, update, uninstall"
# The server left running by the scenario above would still hold the port.
if [ -f "$T/server.pid" ]; then kill "$(cat "$T/server.pid")" 2>/dev/null; sleep 0.5; rm -f "$T/server.pid"; fi
rm -rf "${T:?}/data" "${T:?}/units" "${T:?}/usr"
U_UNIT="$T/home/tester/.config/systemd/user/elora-server.service"
U_DATA="$T/home/tester/.local/share/elora-server"
install_run_user ELORA_NAME=Mine ELORA_PORT="$PORT" ELORA_PASSWORD=pw
STATUS=$?
check "it succeeds" test "$STATUS" -eq 0
check "the unit is in the user's own folder and starts with the login session" grep -q '^WantedBy=default.target' "$U_UNIT"
check "the unit has no User= line (the manager already runs as that user)" bash -c "! grep -q '^User=' '$U_UNIT'"
check "the settings are in the user's data folder" grep -q "\"port\": $PORT" "$U_DATA/config.json"
check "lingering was enabled so it starts at boot" grep -q '^loginctl enable-linger tester' "$T/calls"
check "it was started through the user manager" grep -q '^systemctl --user restart' "$T/calls"
check "the server answers" bash -c "( exec 9<>/dev/tcp/127.0.0.1/$PORT )"
U_PID=$(cat "$T/server.pid")
sleep 1
install_run_user ELORA_ACTION=update
STATUS=$?
check "an update succeeds" test "$STATUS" -eq 0
check "and keeps the settings" grep -q "\"port\": $PORT" "$U_DATA/config.json"
check "the update restarted the user service" bash -c "[ \"\$(cat '$T/server.pid')\" != '$U_PID' ]"
install_run_user ELORA_ACTION=uninstall
check "uninstall removes the user unit" test ! -e "$U_UNIT"
check "uninstall keeps the notes folder" test -d "$U_DATA/notes"

printf '\n%d passed, %d failed\n' "$PASSED" "$FAILED"
[ "$FAILED" -eq 0 ]
