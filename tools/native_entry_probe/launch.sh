#!/bin/sh
# One approved development exec; preserve stock restart policy and environment.
set -eu
root=/run/rmb-qt-probe-c670c4e880304c1a91359005d8b96219
nonce=c670c4e880304c1a91359005d8b96219
previous_umask=$(umask)
stock() {
    exec 9>&-
    umask "$previous_umask"
    unset LD_PRELOAD LD_LIBRARY_PATH XOVI_ROOT
    exec /usr/bin/xochitl --system
}
if ! test -d "$root" || test -L "$root" || ! test "$(stat -c '%a %u' "$root")" = '700 0'; then stock; fi
if ! test -f "$root/owner" || test -L "$root/owner" || ! test "$(cat "$root/owner")" = "$nonce"; then stock; fi
umask 077
exec 9>"$root/admission.lock"
if ! flock -n 9; then stock; fi
if test -e "$root/entry.closed" || test -e "$root/restore.claim"; then
    flock -u 9
    exec 9>&-
    stock
fi
# Consume before loading. Every later start takes the unmodified stock branch.
if ! (set -C; printf '%s' "$nonce" > "$root/attempt.claim") 2>/dev/null; then flock -u 9; exec 9>&-; stock; fi
if ! test "$(sha256sum "$root/payload.so" | awk '{print $1}')" = 05c1d36ce3a871287860cd71a54d6df95bfb82c8f573de75c349e8ad9aff3e82; then flock -u 9; exec 9>&-; stock; fi
if ! printf '%s %s\n' "$$" "$(awk '{print $22}' /proc/$$/stat)" > "$root/attempt.identity"; then flock -u 9; exec 9>&-; stock; fi
flock -u 9
exec 9>&-
if test -e "$root/entry.closed" || test -e "$root/restore.claim"; then stock; fi
umask "$previous_umask"
LD_PRELOAD="$root/payload.so" exec /usr/bin/xochitl --system
