#!/bin/sh
# Sole transient oneshot writer; main and timer start the SAME service.
set -eu
umask 077
root=/run/rmb-qt-probe-894a910b62d947cf8d1602c30e162604
nonce=894a910b62d947cf8d1602c30e162604
parent=/run/systemd/system/xochitl.service.d
dropin="$parent/zz-rmb-qt-probe-$nonce.conf"
test -d "$root" && test ! -L "$root"
test "$(stat -c '%a %u' "$root")" = '700 0'
test -f "$root/owner" && test ! -L "$root/owner"
test "$(cat "$root/owner")" = "$nonce"
stock_files() {
    test "$(sha256sum /usr/lib/systemd/system/xochitl.service | awk '{print $1}')" = adb0a2654ce9ec884f67c0627c22d539d6475dd0af80816819ee80bf13c0e6d6
    test "$(sha256sum /usr/lib/systemd/system/xochitl.service.d/xochitl-service-override.conf | awk '{print $1}')" = b15560e1dca2f4451b59537c490015aa2f5ea691437bd7ddddc7a65411aaad6e
}
healthy_stock() {
    stock_files
    test ! -e "$dropin"
    case "$(systemctl show --property=ExecStart --value xochitl.service)" in *'path=/usr/bin/xochitl ; argv[]=/usr/bin/xochitl --system ;'*) :;; *) return 1;; esac
    test "$(systemctl show --property=Restart --value xochitl.service)" = on-failure
    test "$(systemctl show --property=RestartMode --value xochitl.service)" = direct
    test -z "$(systemctl show --property=Job --value xochitl.service)"
    systemctl is-active xochitl.service reader-buddy.service rm-sync.service >/dev/null
    p=$(systemctl show --property=MainPID --value xochitl.service)
    case "$p" in ''|*[!0-9]*) return 1;; esac
    test "$p" -gt 1
    ps=$(awk '{print $22}' "/proc/$p/stat")
    case "$(awk '{print $3}' "/proc/$p/stat")" in R|S) :;; *) return 1;; esac
    test "$(sha256sum "/proc/$p/exe" | awk '{print $1}')" = 071d85beef3ef2d4cc0e11002140b27b82a2cc04a2ed740a5669f591069b77df
    # Exact stock cgroup, bounded enumeration; no loaded payload in any member.
    members=$(awk 'NR>32 {exit 1} {print}' /sys/fs/cgroup/systemd/system.slice/xochitl.service/cgroup.procs)
    for member in $members; do
        case "$member" in ''|*[!0-9]*) return 1;; esac
        awk -v so="$root/payload.so" '$NF==so {found=1} END {exit found}' "/proc/$member/maps"
    done
    test "$(systemctl show --property=MainPID --value xochitl.service)" = "$p"
    test "$(awk '{print $22}' "/proc/$p/stat")" = "$ps"
}
attempt_gone() {
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
exec 9>"$root/admission.lock"
waits=0
while ! flock -n 9; do
    waits=$((waits + 1))
    test "$waits" -lt 30
    sleep 1
done
# A failed previous execution is historical uncertainty, not restart permission.
if ! (set -C; printf '%s' "$nonce" > "$root/restore.claim") 2>/dev/null; then
    healthy_stock
    attempt_gone
    printf '%s stock-verified\n' "$nonce" > "$root/restored"
    exit 0
fi
stock_files
changed=no
if test -e "$dropin"; then
    test -f "$dropin" && test ! -L "$dropin"
    test "$(sha256sum "$dropin" | awk '{print $1}')" = "$(cat "$root/dropin.sha256")"
    test "$(stat -c '%d %i %a %u' "$parent")" = "$(cat "$root/parent.signature")"
    rm -f "$dropin"
    rmdir "$parent" # Only the initially absent, exact owned empty parent.
    systemctl daemon-reload
    changed=yes
fi
flock -u 9
exec 9>&-
if test "$changed" = yes; then systemctl restart xochitl.service; fi
systemctl start xochitl.service reader-buddy.service rm-sync.service
healthy_stock
attempt_gone
printf '%s stock-verified\n' "$nonce" > "$root/restored"
