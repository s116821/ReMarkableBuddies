#!/bin/sh
set -eu
umask 077
. "$(dirname "$0")/admission.sh"
test "$#" = 1
page_setup_admit "$1"
page_setup_recheck
# Exact ln -T no-clobber behavior must pass the owned packet preflight first.
# Never rename over an existing token; publication is one immutable hard link.
(set -C; printf '%s %s %s open\n' "$nonce" "$p" "$started" > "$root/open-arm.tmp")
chmod 600 "$root/open-arm.tmp"
test ! -e "$root/callback.json" && test ! -L "$root/callback.json" || exit 90
test ! -e "$root/entry.closed" && test ! -L "$root/entry.closed" || exit 90
test ! -e "$root/restore.claim" && test ! -L "$root/restore.claim" || exit 90
test ! -e "$root/restored" && test ! -L "$root/restored" || exit 90
ln -T "$root/open-arm.tmp" "$root/open-arm"
rm "$root/open-arm.tmp"
page_setup_release
printf 'development-open-arm-published\n'
