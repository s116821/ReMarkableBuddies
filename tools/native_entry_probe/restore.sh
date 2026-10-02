#!/bin/sh
# Sole transient oneshot writer; main and timer start the SAME service.
set -eu
umask 077
root=/run/rmb-qt-probe-b5e236df81cb4d3e96c981eeb41b8769
nonce=b5e236df81cb4d3e96c981eeb41b8769
parent=/run/systemd/system/xochitl.service.d
dropin="$parent/zz-rmb-qt-probe-$nonce.conf"
test -d "$root" && test ! -L "$root"
test "$(stat -c '%a %u' "$root")" = '700 0'
test -f "$root/owner" && test ! -L "$root/owner"
test "$(cat "$root/owner")" = "$nonce"
phase=stock-files
case "${1-}" in ''|--verify)
    failure_file=restore.failure
    if test "${1-}" = --verify; then failure_file=verification.first-refusal; fi
    trap 'result=$?; if test "$result" -ne 0; then (set -C; printf "phase=%s exit=%s\n" "$phase" "$result" > "$root/$failure_file") 2>/dev/null || :; fi' EXIT
esac
stock_files() {
    phase=original-unit-hash
    test "$(sha256sum /usr/lib/systemd/system/xochitl.service | awk '{print $1}')" = adb0a2654ce9ec884f67c0627c22d539d6475dd0af80816819ee80bf13c0e6d6
    phase=vendor-dropin-hash
    test "$(sha256sum /usr/lib/systemd/system/xochitl.service.d/xochitl-service-override.conf | awk '{print $1}')" = b15560e1dca2f4451b59537c490015aa2f5ea691437bd7ddddc7a65411aaad6e
}
healthy_stock() {
    stock_files
    phase=runtime-override-absent
    test ! -e "$dropin"
    phase=stock-command
    case "$(systemctl show --property=ExecStart --value xochitl.service)" in *'path=/usr/bin/xochitl ; argv[]=/usr/bin/xochitl --system ;'*) :;; *) return 1;; esac
    phase=restart-policy
    test "$(systemctl show --property=Restart --value xochitl.service)" = on-failure
    test "$(systemctl show --property=RestartMode --value xochitl.service)" = direct
    phase=stock-job-empty
    test -z "$(systemctl show --property=Job --value xochitl.service)"
    phase=original-services-active
    systemctl is-active xochitl.service reader-buddy.service rm-sync.service >/dev/null
    phase=stock-pid-start
    p=$(systemctl show --property=MainPID --value xochitl.service)
    case "$p" in ''|*[!0-9]*) return 1;; esac
    test "$p" -gt 1
    ps=$(awk '{print $22}' "/proc/$p/stat")
    phase=stock-process-state
    case "$(awk '{print $3}' "/proc/$p/stat")" in R|S) :;; *) return 1;; esac
    phase=stock-executable-hash
    test "$(sha256sum "/proc/$p/exe" | awk '{print $1}')" = 071d85beef3ef2d4cc0e11002140b27b82a2cc04a2ed740a5669f591069b77df
    # Exact stock cgroup, bounded enumeration; no loaded payload in any member.
    phase=stock-cgroup-payload-absence
    members=$(awk 'NR>32 {exit 1} {print}' /sys/fs/cgroup/systemd/system.slice/xochitl.service/cgroup.procs)
    for member in $members; do
        case "$member" in ''|*[!0-9]*) return 1;; esac
        awk -v so="$root/payload.so" '$NF==so {found=1} END {exit found}' "/proc/$member/maps"
    done
    phase=stock-identity-stable
    test "$(systemctl show --property=MainPID --value xochitl.service)" = "$p"
    test "$(awk '{print $22}' "/proc/$p/stat")" = "$ps"
}
attempt_gone() {
    phase=attempt-gone
    if test -f "$root/attempt.identity"; then
        read attempted started < "$root/attempt.identity"
        case "$attempted" in ''|*[!0-9]*) return 1;; esac
        case "$started" in ''|*[!0-9]*) return 1;; esac
        if test -d "/proc/$attempted" && test "$(awk '{print $22}' "/proc/$attempted/stat")" = "$started"; then
            case "$(awk '{print $3}' "/proc/$attempted/stat")" in Z|X) :;; *) return 1;; esac
        fi
    fi
}
case "${1-}" in
    --verify) healthy_stock; attempt_gone; exit 0;;
    '') :;;
    *) exit 90;;
esac
printf '%s' "$nonce" > "$root/entry.closed"
phase=admission-lock
exec 9>"$root/admission.lock"
waits=0
while ! flock -n 9; do
    waits=$((waits + 1))
    test "$waits" -lt 30
    sleep 1
done
# A failed previous execution is historical uncertainty, not restart permission.
phase=restore-claim
if ! (set -C; printf '%s' "$nonce" > "$root/restore.claim") 2>/dev/null; then
    healthy_stock
    attempt_gone
    printf '%s stock-verified\n' "$nonce" > "$root/restored"
    exit 0
fi
phase=stock-files
stock_files
changed=no
if test -e "$dropin"; then
    phase=owned-dropin-validation
    test -f "$dropin" && test ! -L "$dropin"
    test "$(sha256sum "$dropin" | awk '{print $1}')" = "$(cat "$root/dropin.sha256")"
    test "$(stat -c '%d %i %a %u' "$parent")" = "$(cat "$root/parent.signature")"
    rm -f "$dropin"
    phase=owned-parent-removal
    rmdir "$parent" # Only the initially absent, exact owned empty parent.
    phase=stock-definition-reload
    systemctl daemon-reload
    changed=yes
fi
flock -u 9
exec 9>&-
phase=stock-restart
if test "$changed" = yes; then systemctl restart xochitl.service; fi
phase=original-services-start
systemctl start xochitl.service reader-buddy.service rm-sync.service
phase=fresh-stock-verification
# Read-only subprocess keeps set -e assertions effective inside each observation.
# Never retry physical service operations; at most ten fresh observations.
checks=0
deadline=$(awk '{print int($1)+20}' /proc/uptime)
while :; do
    phase=readiness-deadline
    test "$(awk '{print int($1)}' /proc/uptime)" -lt "$deadline"
    phase=fresh-stock-verification
    verified=no
    if /bin/sh "$root/restore.sh" --verify; then verified=yes; fi
    phase=readiness-deadline
    test "$(awk '{print int($1)}' /proc/uptime)" -lt "$deadline"
    if test "$verified" = yes; then break; fi
    checks=$((checks + 1))
    phase=readiness-observation-cap
    test "$checks" -lt 10
    sleep 1
done
phase=receipt-publication
printf '%s stock-verified\n' "$nonce" > "$root/restored"
