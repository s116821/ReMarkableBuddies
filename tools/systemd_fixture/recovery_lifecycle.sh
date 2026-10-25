#!/bin/sh
# Explicit development-host fixture. Never starts/stops the real Reader or Buddy.
# Usage: sh recovery_lifecycle.sh <fresh 32 lowercase hex nonce> <case>
# Cases: clean, crash, watchdog, stop, recovery-retry.
# All units expire independently. Evidence remains in /run/buddy-recovery-NONCE.
set -eu
umask 077
nonce=${1-}
mode=${2-}
case "$nonce" in ''|*[!0-9a-f]*) exit 64;; esac
[ "${#nonce}" -eq 32 ] || exit 64
case "$mode" in clean|crash|watchdog|stop|recovery-retry) ;; *) exit 64;; esac
[ "$(id -u)" -eq 0 ] || exit 77
for command in systemctl systemd-notify journalctl stat sha256sum awk; do
    command -v "$command" >/dev/null || exit 69
done
awk '$5 == "/sys/fs/cgroup/unified" && / - cgroup2 / { found=1 } END { exit !found }' /proc/self/mountinfo
root=/run/buddy-recovery-$nonce
stock=buddy-recovery-$nonce-stock.service
actor=buddy-recovery-$nonce-actor.service
recovery=buddy-recovery-$nonce-recovery.service
for unit in "$stock" "$actor" "$recovery"; do
    [ "$(systemctl show "$unit" -p LoadState --value)" = not-found ] || exit 73
    [ ! -e "/run/systemd/system/$unit" ] || exit 73
    [ ! -L "/run/systemd/system/$unit" ] || exit 73
done
mkdir -m 700 "$root" # exclusive, never reuse a prior run
printf '%s\n' "$mode" > "$root/mode"
snapshot_real() {
    systemctl show xochitl.service reader-buddy.service \
        -p Id -p ActiveState -p SubState -p MainPID -p InvocationID -p Job
    sha256sum /usr/bin/xochitl /usr/lib/systemd/system/xochitl.service \
        /usr/lib/systemd/system/xochitl.service.d/xochitl-service-override.conf
}
snapshot_real > "$root/real-before.txt"
created=''
cleanup() {
    code=$?
    trap - EXIT HUP INT TERM
    set +e
    # Record before cleanup can itself trigger the actor's recovery dependency.
    for unit in "$stock" "$actor" "$recovery"; do
        systemctl show "$unit" -p Id -p ActiveState -p SubState -p Result \
            -p MainPID -p InvocationID -p ControlGroup -p Job
        journalctl -u "$unit" -n 80 --no-pager -o short-monotonic
    done > "$root/units-final.txt" 2>&1
    # Block restoration before stopping any remaining owned actor.
    touch "$root/cleanup"
    cleanup_ok=yes
    systemctl stop "$actor" "$recovery" "$stock" || cleanup_ok=no
    for unit in $created; do
        state=$(systemctl show "$unit" -p ActiveState --value) || cleanup_ok=no
        case "$state" in inactive|failed) ;; *) cleanup_ok=no;; esac
        [ "$(systemctl show "$unit" -p MainPID --value)" = 0 ] || cleanup_ok=no
        [ -z "$(systemctl show "$unit" -p Job --value)" ] || cleanup_ok=no
        path=/sys/fs/cgroup/unified/system.slice/$unit
        if [ -d "$path" ]; then
            grep -qx 'populated 0' "$path/cgroup.events" || cleanup_ok=no
        fi
    done
    if [ "$cleanup_ok" = yes ]; then
        for unit in $created; do
            rm -f "/run/systemd/system/$unit" || cleanup_ok=no
        done
        systemctl daemon-reload || cleanup_ok=no
        systemctl reset-failed "$actor" "$recovery" "$stock" >/dev/null 2>&1
    fi
    [ "$cleanup_ok" = yes ] || code=1
    snapshot_real > "$root/real-after.txt"
    if ! cmp -s "$root/real-before.txt" "$root/real-after.txt"; then code=1; fi
    printf 'exit=%s\ncleanup=%s\nroot=%s\n' "$code" "$cleanup_ok" "$root" > "$root/result.txt"
    cat "$root/result.txt"
    exit "$code"
}
trap cleanup EXIT
trap 'exit 130' HUP INT TERM
cat > "$root/role.sh" <<'ROLE'
set -eu
umask 077
role=$1
root=$2
base=${root##*/}
stock=$base-stock.service
actor=$base-actor.service
mode=$(cat "$root/mode")
case "$role" in
stock)
    # flock is unnecessary: PID1 serializes starts of this one fake unit.
    printf 'start\n' >> "$root/stock-starts.txt"
    exec sleep 65
    ;;
actor)
    systemd-notify --ready
    n=0
    while [ ! -e "$root/go" ]; do
        [ "$n" -lt 50 ] || exit 70
        n=$((n+1)); sleep .1
    done
    systemctl stop "$stock"
    group=$(systemctl show "$actor" -p ControlGroup --value)
    [ "$group" = "/system.slice/$actor" ] || exit 71
    printf '%s\n' "$group" > "$root/actor-group.txt"
    stat -c '%d:%i' "/sys/fs/cgroup/unified$group" > "$root/group-before.identity"
    cat "/sys/fs/cgroup/unified$group/cgroup.events" > "$root/group-before.events"
    sleep 60 &
    child=$!
    printf '%s\n' "$child" > "$root/child.pid"
    cat "/proc/$child/stat" > "$root/child-before.stat"
    touch "$root/armed"
    case "$mode" in
        clean|recovery-retry) exit 0;;
        crash) kill -KILL "$$";;
        watchdog|stop) wait "$child";;
    esac
    ;;
recovery)
    [ ! -e "$root/cleanup" ] || exit 0
    printf 'attempt\n' >> "$root/recovery-attempts.txt"
    state=$(systemctl show "$actor" -p ActiveState --value)
    case "$state" in inactive|failed) ;; *) exit 72;; esac
    child=$(cat "$root/child.pid")
    [ ! -e "/proc/$child" ] || exit 73
    group=$(cat "$root/actor-group.txt")
    [ "$group" = "/system.slice/$actor" ] || exit 74
    path=/sys/fs/cgroup/unified$group
    if [ -d "$path" ]; then
        stat -c '%d:%i' "$path" >> "$root/group-recovery.identity"
        cat "$path/cgroup.events" >> "$root/group-recovery.events"
        grep -qx 'populated 0' "$path/cgroup.events" || exit 75
    else
        # Observation only. NOT a production retained-identity/emptiness proof.
        printf 'absent-after-PID1-stop\n' >> "$root/group-recovery.identity"
    fi
    # Intent is durable across a helper crash; it is not a one-shot restore latch.
    if [ ! -e "$root/restore-intent" ]; then
        (set -C; : > "$root/restore-intent")
        if [ "$mode" = recovery-retry ]; then exit 76; fi
    fi
    # PID1 start of the same active unit is idempotent, including retry after a
    # successful start but before publishing restored. The host tests that retry.
    systemctl start "$stock"
    [ "$(systemctl show "$stock" -p ActiveState --value)" = active ]
    touch "$root/restored"
    ;;
*) exit 64;;
esac
ROLE
chmod 700 "$root/role.sh"
# Exclusive creation prevents overwriting any unrelated unit, including symlinks.
(set -C; cat > "/run/systemd/system/$stock" <<UNIT
[Unit]
Description=Disposable Buddy recovery fake stock $nonce
[Service]
ExecStart=/bin/sh $root/role.sh stock $root
Restart=no
KillMode=control-group
RuntimeMaxSec=70s
TimeoutStopSec=2s
UNIT
)
created="$stock"
(set -C; cat > "/run/systemd/system/$actor" <<UNIT
[Unit]
Description=Disposable Buddy recovery actor $nonce
OnSuccess=$recovery
OnFailure=$recovery
[Service]
Type=notify
NotifyAccess=all
ExecStart=/bin/sh $root/role.sh actor $root
Restart=no
KillMode=control-group
SendSIGKILL=yes
TimeoutStartSec=6s
TimeoutStopSec=2s
RuntimeMaxSec=20s
WatchdogSec=3s
WatchdogSignal=SIGKILL
UNIT
)
created="$created $actor"
(set -C; cat > "/run/systemd/system/$recovery" <<UNIT
[Unit]
Description=Disposable Buddy stock recovery $nonce
Conflicts=$actor
After=$actor
[Service]
Type=oneshot
ExecStart=/bin/sh $root/role.sh recovery $root
Restart=no
TimeoutStartSec=12s
TimeoutStopSec=2s
KillMode=control-group
UNIT
)
created="$created $recovery"
# Retain exact inputs even after owned unit files are removed.
for unit in $created; do cp "/run/systemd/system/$unit" "$root/$unit"; done
systemctl daemon-reload
systemctl start "$stock"
n=0
while [ ! -e "$root/stock-starts.txt" ]; do
    [ "$n" -lt 30 ] || exit 89
    n=$((n+1)); sleep .1
done
[ "$(wc -l < "$root/stock-starts.txt")" -eq 1 ] || exit 90
systemctl start "$actor"
touch "$root/go"
n=0
while [ ! -e "$root/armed" ]; do
    [ "$n" -lt 60 ] || exit 80
    n=$((n+1)); sleep .1
done
if [ "$mode" = stop ]; then systemctl stop "$actor"; fi
if [ "$mode" = recovery-retry ]; then
    n=0
    while [ "$(systemctl show "$recovery" -p ActiveState --value)" != failed ]; do
        [ "$n" -lt 100 ] || exit 81
        n=$((n+1)); sleep .1
    done
    [ ! -e "$root/restored" ] || exit 82
    systemctl start "$recovery"
fi
n=0
while [ ! -e "$root/restored" ]; do
    [ "$n" -lt 150 ] || exit 83
    n=$((n+1)); sleep .1
done
# Second recovery request after success must not launch another fake stock.
n=0
while [ "$(systemctl show "$recovery" -p ActiveState --value)" != inactive ]; do
    [ "$n" -lt 150 ] || exit 86
    n=$((n+1)); sleep .1
done
attempts=$(wc -l < "$root/recovery-attempts.txt")
systemctl start "$recovery"
[ "$(wc -l < "$root/recovery-attempts.txt")" -eq "$((attempts+1))" ] || exit 87
n=0
while [ "$(wc -l < "$root/stock-starts.txt")" -lt 2 ]; do
    [ "$n" -lt 30 ] || exit 88
    n=$((n+1)); sleep .1
done
[ "$(wc -l < "$root/stock-starts.txt")" -eq 2 ] || exit 84
[ ! -e "/proc/$(cat "$root/child.pid")" ] || exit 85
systemctl show "$actor" -p Result -p ActiveState -p SubState > "$root/actor-result.txt"
case "$mode" in
    clean|stop|recovery-retry) grep -qx 'Result=success' "$root/actor-result.txt";;
    crash) grep -qx 'Result=signal' "$root/actor-result.txt";;
    watchdog) grep -qx 'Result=watchdog' "$root/actor-result.txt";;
esac
