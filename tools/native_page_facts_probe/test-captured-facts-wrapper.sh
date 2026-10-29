#!/bin/sh
# Actual source wrapper with test-only root/artifact bindings; no target access.
set -eu
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
root="$work/rmb-qt-probe-0123456789abcdef0123456789abcdef"
mkdir -m700 "$root"
cat > "$root/facts-publisher" <<'STUB'
#!/bin/sh
printf '%s\n' "$7" > "$1/called"
STUB
chmod 700 "$root/facts-publisher"
printf 'Main local reviewed decision bytes\n' > "$root/capture-visual-review.json"
chmod 600 "$root/capture-visual-review.json"
publisher=$(sha256sum "$root/facts-publisher" | awk '{print $1}')
review=$(sha256sum "$root/capture-visual-review.json" | awk '{print $1}')
tr -d '\r' < tools/native_page_facts_probe/publish-captured-facts-source.sh > "$work/template"
# Guard removal and ephemeral bindings are confined to this owned fixture.
sed -e '/^exit 90$/d' -e "s|^root=.*|root=$root|" \
 -e "s|= 0000000000000000000000000000000000000000000000000000000000000000$|= $publisher|" \
 -e "s|^review=0000000000000000000000000000000000000000000000000000000000000000$|review=$review|" \
 "$work/template" > "$work/final-recipe"
/bin/sh "$work/final-recipe"
test "$(cat "$root/called")" = "$review"
rm -f "$root/called"
printf 'Remote replacement decision bytes\n' > "$root/capture-visual-review.json"
if /bin/sh "$work/final-recipe"; then exit 1; fi
test ! -e "$root/called"
printf 'PASS captured facts wrapper: local literal accepted; remote replacement refused (host fixture only)\n'
