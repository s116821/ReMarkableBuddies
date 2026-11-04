#!/bin/sh
# Read-only positive current waiting proof for Main's deliberate fixture actions.
set -eu
root=/run/rmb-qt-probe-b3e3ca0475a84432a9b328218395de0c
nonce=b3e3ca0475a84432a9b328218395de0c
test -d "$root" && test ! -L "$root" || exit 90
test "$(stat -c '%a %u' "$root")" = '700 0'
for name in owner attempt.identity input-observation-ready payload.so; do
 test -f "$root/$name" && test ! -L "$root/$name" || exit 90
 test "$(stat -c '%a %u' "$root/$name")" = '600 0'
done
test "$(wc -c < "$root/owner")" = 32
test "$(cat "$root/owner")" = "$nonce"
test "$(wc -c < "$root/input-observation-ready")" -le 256
test "$(wc -c < "$root/attempt.identity")" -le 128
read n p started dev ino stage ms setup profile extra < "$root/input-observation-ready"
test -z "$extra" && test "$n" = "$nonce" || exit 90
case "$p:$started:$dev:$ino:$ms" in *[!0-9:]*|:*|*:|*::*) exit 90;; esac
for value in "$p" "$started" "$dev" "$ino"; do
 case "$value" in 0*) exit 90;; esac
 test "${#value}" -le 20 || exit 90
done
case "$ms" in 0) :;; 0*) exit 90;; esac
test "${#ms}" -le 6 || exit 90
test "$p" -gt 1 && test "$started" -gt 0 || exit 90
test "$stage" = waiting-input-observation && test "$setup" = 120000 && test "$profile" = main-dev-input-observation-120s || exit 90
test "$ms" -ge 0 && test "$ms" -lt 120000 || exit 90
test "$(stat -c '%d %i' "$root")" = "$dev $ino"
test "$(cat "$root/attempt.identity")" = "$p $started"
for name in entry.closed restore.claim callback.json input-observation-end input-observation-end.tmp facts-waiting facts-request facts-request.tmp input-observation-complete.json; do
 test ! -e "$root/$name" && test ! -L "$root/$name" || exit 90
done
test "$(systemctl show --property=MainPID --value xochitl.service)" = "$p"
test -z "$(systemctl show --property=Job --value xochitl.service)"
test "$(awk '{print $22}' "/proc/$p/stat")" = "$started"
test "$(sha256sum "/proc/$p/exe" | awk '{print $1}')" = 071d85beef3ef2d4cc0e11002140b27b82a2cc04a2ed740a5669f591069b77df
test "$(sha256sum "$root/payload.so" | awk '{print $1}')" = 0000000000000000000000000000000000000000000000000000000000000000
awk -v so="$root/payload.so" '$NF==so {found=1} END {exit !found}' "/proc/$p/maps"
test "$(awk '{print $22}' "/proc/$p/stat")" = "$started"
test "$(systemctl show --property=MainPID --value xochitl.service)" = "$p"
test -z "$(systemctl show --property=Job --value xochitl.service)"
test "$(stat -c '%d %i %a %u' "$root")" = "$dev $ino 700 0"
test "$(cat "$root/owner")" = "$nonce"
test "$(cat "$root/input-observation-ready")" = "$n $p $started $dev $ino $stage $ms $setup $profile"
for name in entry.closed restore.claim callback.json input-observation-end input-observation-end.tmp facts-waiting facts-request facts-request.tmp input-observation-complete.json; do
 test ! -e "$root/$name" && test ! -L "$root/$name" || exit 90
done
printf 'positive-input-observation-ready %s %s\n' "$p" "$started"
