#!/bin/sh
# Main invokes once after the selected input observation interval.
set -eu
root=/run/rmb-qt-probe-b3e3ca0475a84432a9b328218395de0c
test -f "$root/input-observation-end-publisher" && test ! -L "$root/input-observation-end-publisher"
test "$(stat -c '%a %u' "$root/input-observation-end-publisher")" = '700 0'
test "$(sha256sum "$root/input-observation-end-publisher" | awk '{print $1}')" = 0000000000000000000000000000000000000000000000000000000000000000
exec "$root/input-observation-end-publisher" "$root" b3e3ca0475a84432a9b328218395de0c 071d85beef3ef2d4cc0e11002140b27b82a2cc04a2ed740a5669f591069b77df 0000000000000000000000000000000000000000000000000000000000000000
