#!/bin/sh
# Main selects exactly one effect OR capture; no sequence/navigation/retry.
set -eu
umask 077
. "$(dirname "$0")/admission.sh"
test "$#" -ge 3
nonce_arg=$1
seconds=$2
shift 2
case "$seconds" in 1|2|3|4|5) :;; *) exit 90;; esac
case "$1" in /*) :;; *) exit 90;; esac
test -f "$1" && test ! -L "$1" && test -x "$1" || exit 90
command -v timeout >/dev/null
page_setup_admit "$nonce_arg"
trap 'status=$?; page_setup_release; exit "$status"' 0
page_setup_recheck
# Ignore wrapper cancellation while its one bounded child is outstanding, so
# recovery cannot acquire fd9 before that child terminates. The timeout child
# resets these signal dispositions and retains fd9 if the wrapper is killed.
trap '' HUP INT TERM
(
    trap - HUP INT TERM
    exec timeout --signal=TERM --kill-after=1s "${seconds}s" "$@"
)
