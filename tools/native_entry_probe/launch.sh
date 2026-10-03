#!/bin/sh
# One approved development exec; preserve stock restart policy and environment.
set -eu
root=/run/rmb-qt-probe-d860f84bc1384e94b3eeafbf776297e9
nonce=d860f84bc1384e94b3eeafbf776297e9
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
if ! test "$(sha256sum "$root/payload.so" | awk '{print $1}')" = db9e0f3436dbac695a03b00d5dcbd27545ee500f745f0d30163799a3ad82bf4c; then flock -u 9; exec 9>&-; stock; fi
if ! printf '%s %s\n' "$$" "$(awk '{print $22}' /proc/$$/stat)" > "$root/attempt.identity"; then flock -u 9; exec 9>&-; stock; fi
flock -u 9
exec 9>&-
if test -e "$root/entry.closed" || test -e "$root/restore.claim"; then stock; fi
umask "$previous_umask"
LD_PRELOAD="$root/payload.so" exec /usr/bin/xochitl --system
