#!/bin/sh
# Read-only positive current waiting proof for Main's deliberate fixture actions.
set -eu
root=/run/rmb-qt-probe-b3e3ca0475a84432a9b328218395de0c
nonce=b3e3ca0475a84432a9b328218395de0c
test -d "$root" && test ! -L "$root"
test "$(stat -c '%a %u' "$root")" = '700 0'
for name in owner attempt.identity facts-waiting payload.so; do
 test -f "$root/$name" && test ! -L "$root/$name"
 test "$(stat -c '%a %u' "$root/$name")" = '600 0'
done
test "$(wc -c < "$root/owner")" = 32
test "$(cat "$root/owner")" = "$nonce"
test "$(wc -c < "$root/facts-waiting")" -le 256
test "$(wc -c < "$root/attempt.identity")" -le 128
read n p started dev ino stage ms setup profile extra < "$root/facts-waiting"
test -z "$extra" && test "$n" = "$nonce"
case "$p:$started:$dev:$ino:$ms" in *[!0-9:]*|:*|*:|*::*) exit 90;; esac
test "$p" -gt 1 && test "$started" -gt 0
test "$stage" = waiting-facts && test "$setup" = 120000 && test "$profile" = main-dev-facts-120s
test "$ms" -ge 0 && test "$ms" -lt 120000
test "$(stat -c '%d %i' "$root")" = "$dev $ino"
test "$(cat "$root/attempt.identity")" = "$p $started"
for name in entry.closed restore.claim callback.json facts-request facts-request.tmp; do
 test ! -e "$root/$name" && test ! -L "$root/$name"
done
test "$(systemctl show --property=MainPID --value xochitl.service)" = "$p"
test -z "$(systemctl show --property=Job --value xochitl.service)"
test "$(awk '{print $22}' "/proc/$p/stat")" = "$started"
test "$(sha256sum "/proc/$p/exe" | awk '{print $1}')" = 071d85beef3ef2d4cc0e11002140b27b82a2cc04a2ed740a5669f591069b77df
test "$(sha256sum "$root/payload.so" | awk '{print $1}')" = a747186fb466b8947caf46a9629654f24743f090b0f7f3e27e764e2dbacdc226
awk -v so="$root/payload.so" '$NF==so {found=1} END {exit !found}' "/proc/$p/maps"
test "$(awk '{print $22}' "/proc/$p/stat")" = "$started"
test "$(systemctl show --property=MainPID --value xochitl.service)" = "$p"
test -z "$(systemctl show --property=Job --value xochitl.service)"
test "$(stat -c '%d %i %a %u' "$root")" = "$dev $ino 700 0"
test "$(cat "$root/owner")" = "$nonce"
test "$(cat "$root/facts-waiting")" = "$n $p $started $dev $ino $stage $ms $setup $profile"
for name in entry.closed restore.claim callback.json facts-request facts-request.tmp; do
 test ! -e "$root/$name" && test ! -L "$root/$name"
done
printf 'positive-waiting %s %s\n' "$p" "$started"
