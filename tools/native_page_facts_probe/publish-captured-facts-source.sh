#!/bin/sh
# SOURCE ONLY: Main alone freezes and calls this after complete-image review.
exit 90
set -eu
root=/run/rmb-qt-probe-b3e3ca0475a84432a9b328218395de0c
test -f "$root/facts-publisher" && test ! -L "$root/facts-publisher"
test "$(stat -c '%a %u' "$root/facts-publisher")" = '700 0'
test "$(sha256sum "$root/facts-publisher" | awk '{print $1}')" = 0000000000000000000000000000000000000000000000000000000000000000
test -f "$root/capture-visual-review.json" && test ! -L "$root/capture-visual-review.json"
test "$(stat -c '%a %u' "$root/capture-visual-review.json")" = '600 0'
test "$(wc -c < "$root/capture-visual-review.json")" -le 4096
# SOURCE ONLY: Main prepares this final recipe AFTER full-image review, binding
# the independently hashed LOCAL decision as a literal, never a remote-derived hash.
review=0000000000000000000000000000000000000000000000000000000000000000
test "${#review}" -eq 64
case "$review" in *[!0-9a-f]*) exit 90;; esac
test "$(sha256sum "$root/capture-visual-review.json" | awk '{print $1}')" = "$review"
# Main's selected expected document/order are fixed in the frozen recipe.
document=e7f661f1-db6f-4dfc-854a-b38aff7f75de
order=SOURCE_ONLY_REPLACE_WITH_EXACT_SIX_PAGE_ORDER
exec "$root/facts-publisher" "$root" b3e3ca0475a84432a9b328218395de0c 071d85beef3ef2d4cc0e11002140b27b82a2cc04a2ed740a5669f591069b77df 0000000000000000000000000000000000000000000000000000000000000000 "$document" "$order" "$review"
