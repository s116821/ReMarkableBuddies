#!/bin/sh
# SOURCE ONLY: Main freezes a fresh nonce/artifact packet before enabling this.
exit 90
set -eu
root=/run/rmb-qt-probe-b3e3ca0475a84432a9b328218395de0c
test -f "$root/capture-publisher" && test ! -L "$root/capture-publisher"
test "$(stat -c '%a %u' "$root/capture-publisher")" = '700 0'
test "$(sha256sum "$root/capture-publisher" | awk '{print $1}')" = 0000000000000000000000000000000000000000000000000000000000000000
exec "$root/capture-publisher" "$root" b3e3ca0475a84432a9b328218395de0c 071d85beef3ef2d4cc0e11002140b27b82a2cc04a2ed740a5669f591069b77df 0000000000000000000000000000000000000000000000000000000000000000
