#!/bin/sh
# One approved development exec; preserve stock restart policy and environment.
set -eu
root=/run/rmb-qt-probe-42d7f551ecc2435b923e5415f228f723
nonce=42d7f551ecc2435b923e5415f228f723
previous_umask=$(umask)
stock() {
    umask "$previous_umask"
    unset LD_PRELOAD LD_LIBRARY_PATH XOVI_ROOT
    exec /usr/bin/xochitl --system
}
if ! test -d "$root" || test -L "$root" || ! test "$(stat -c '%a %u' "$root")" = '700 0'; then stock; fi
if ! test -f "$root/owner" || test -L "$root/owner" || ! test "$(cat "$root/owner")" = "$nonce"; then stock; fi
umask 077
# Consume before loading. Every later start takes the unmodified stock branch.
if ! (set -C; printf '%s' "$nonce" > "$root/attempt.claim") 2>/dev/null; then stock; fi
if ! test "$(sha256sum "$root/payload.so" | awk '{print $1}')" = ddd2f966f9e429b78d7f7b9e415a1994d9b355740e4bdb750e09025c0517ac41; then stock; fi
if ! printf '%s %s\n' "$$" "$(awk '{print $22}' /proc/$$/stat)" > "$root/attempt.identity"; then stock; fi
umask "$previous_umask"
LD_PRELOAD="$root/payload.so" exec /usr/bin/xochitl --system
