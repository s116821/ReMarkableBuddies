#!/bin/sh
# Main-only development setup permission after one verified source-page capture.
# This is not a UI command or proof of page ownership.
set -eu
umask 077
root=/run/rmb-qt-probe-604d9d8e17c046fe84fea4e44e608165
nonce=604d9d8e17c046fe84fea4e44e608165
test -d "$root" && test ! -L "$root" || exit 90
test "$(stat -c '%a %u' "$root")" = '700 0'
test -f "$root/owner" && test ! -L "$root/owner" || exit 90
test "$(stat -c '%a %u' "$root/owner")" = '600 0'
test "$(stat -c '%s' "$root/owner")" = 32
test "$(cat "$root/owner")" = "$nonce"
exec 9>"$root/admission.lock"
flock -n 9
open_admission() {
    for name in entry.closed restore.claim restored open-arm open-arm.tmp; do
        test ! -e "$root/$name" && test ! -L "$root/$name" || return 1
    done
}
open_admission
test -f "$root/attempt.identity" && test ! -L "$root/attempt.identity" || exit 90
test "$(stat -c '%a %u' "$root/attempt.identity")" = '600 0'
test "$(stat -c '%s' "$root/attempt.identity")" -le 64
read p started extra < "$root/attempt.identity"
test -z "$extra"
case "$p:$started" in *[!0-9:]*|:*|*:) exit 90;; esac
test -f "$root/open-waiting" && test ! -L "$root/open-waiting" || exit 90
test "$(stat -c '%a %u' "$root/open-waiting")" = '600 0'
expected="$nonce $p $started waiting"
test "$(stat -c '%s' "$root/open-waiting")" -eq "$((${#expected}+1))"
test "$(cat "$root/open-waiting")" = "$expected"
test "$(systemctl show --property=MainPID --value xochitl.service)" = "$p"
test "$(awk '{print $22}' "/proc/$p/stat")" = "$started"
test "$(sha256sum "/proc/$p/exe" | awk '{print $1}')" = 071d85beef3ef2d4cc0e11002140b27b82a2cc04a2ed740a5669f591069b77df
awk -v expected="LD_PRELOAD=$root/payload.so" 'BEGIN {RS="\0"} $0 == expected {found=1} END {exit !found}' "/proc/$p/environ"
test "$(awk '{print $22}' "/proc/$p/stat")" = "$started"
test "$(systemctl show --property=MainPID --value xochitl.service)" = "$p"
test -z "$(systemctl show --property=Job --value xochitl.service)"
open_admission
# Exact ln -T no-clobber behavior must pass the owned packet preflight first.
# Never rename over an existing token; publication is one immutable hard link.
(set -C; printf '%s %s %s open\n' "$nonce" "$p" "$started" > "$root/open-arm.tmp")
chmod 600 "$root/open-arm.tmp"
test ! -e "$root/entry.closed" && test ! -L "$root/entry.closed" || exit 90
test ! -e "$root/restore.claim" && test ! -L "$root/restore.claim" || exit 90
test ! -e "$root/restored" && test ! -L "$root/restored" || exit 90
ln -T "$root/open-arm.tmp" "$root/open-arm"
rm "$root/open-arm.tmp"
flock -u 9
exec 9>&-
printf 'development-open-arm-published\n'
