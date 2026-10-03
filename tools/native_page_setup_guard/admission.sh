# Shared admission factored from accepted arm-open-once.sh; source only.
# Caller uses set -eu and must retain fd9 through one bounded effect/publication.
page_setup_initialize() {
nonce=$1
case "$nonce" in *[!0-9a-f]*|'') exit 90;; esac
test "${#nonce}" = 32
root=/run/rmb-qt-probe-$nonce
}
page_setup_namespace() {
test -d "$root" && test ! -L "$root" || exit 90
test "$(stat -c '%a %u' "$root")" = '700 0'
test -f "$root/owner" && test ! -L "$root/owner" || exit 90
test "$(stat -c '%a %u' "$root/owner")" = '600 0'
test "$(stat -c '%s' "$root/owner")" = 32
test "$(cat "$root/owner")" = "$nonce"
}
open_admission() {
    for name in callback.json entry.closed restore.claim restored open-arm open-arm.tmp; do
        test ! -e "$root/$name" && test ! -L "$root/$name" || return 1
    done
}
page_setup_validate() {
page_setup_namespace
open_admission
test -f "$root/attempt.identity" && test ! -L "$root/attempt.identity" || exit 90
test "$(stat -c '%a %u' "$root/attempt.identity")" = '600 0'
test "$(stat -c '%s' "$root/attempt.identity")" -le 64
read p started extra < "$root/attempt.identity"
test -z "$extra"
case "$p:$started" in *[!0-9:]*|:*|*:) exit 90;; esac
identity="$p $started"
test "$(stat -c '%s' "$root/attempt.identity")" -eq "$((${#identity}+1))"
test "$(cat "$root/attempt.identity")" = "$identity"
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
}
page_setup_admit() {
    page_setup_initialize "$1"
    page_setup_namespace
    test -f "$root/admission.lock" && test ! -L "$root/admission.lock" || exit 90
    test "$(stat -c '%u' "$root/admission.lock")" = 0
    exec 9<>"$root/admission.lock"
    flock -n 9
    stage_signature=$(stat -c '%d %i' "$root")
    page_setup_validate
}
page_setup_recheck() {
    test -d "$root" && test ! -L "$root" || exit 90
    test "$(stat -c '%d %i' "$root")" = "$stage_signature"
    page_setup_validate
}
page_setup_release() {
    flock -u 9
    exec 9>&-
}
