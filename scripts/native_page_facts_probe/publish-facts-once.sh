#!/bin/sh
# Main invokes once after one positively guarded fixture opening/capture.
set -eu
root=/run/rmb-qt-probe-b3e3ca0475a84432a9b328218395de0c
test -f "$root/facts-publisher" && test ! -L "$root/facts-publisher"
test "$(stat -c '%a %u' "$root/facts-publisher")" = '700 0'
test "$(sha256sum "$root/facts-publisher" | awk '{print $1}')" = c8358d0f141f1556fb8ca27043c807db71aa5c3dbdc0430e2e6ff1dc554a22af
exec "$root/facts-publisher" "$root" b3e3ca0475a84432a9b328218395de0c 071d85beef3ef2d4cc0e11002140b27b82a2cc04a2ed740a5669f591069b77df a747186fb466b8947caf46a9629654f24743f090b0f7f3e27e764e2dbacdc226
