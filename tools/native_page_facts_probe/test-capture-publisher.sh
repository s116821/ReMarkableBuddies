#!/bin/sh
set -eu
if [ -n "${QT_PROBE_SDK_ENV:-}" ]; then . "$QT_PROBE_SDK_ENV"; fi
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
${CXX:-c++} -std=c++17 -Wall -Wextra -Werror -fPIC -fsyntax-only tools/native_page_facts_probe/capture-observation-publisher.cpp tools/native_page_facts_probe/captured-facts-publisher.cpp $(pkg-config --cflags Qt6Core)
${CXX:-c++} -std=c++17 -Wall -Wextra -Werror -O2 -fPIC tools/native_page_facts_probe/capture-publisher-fixture.cpp $(pkg-config --cflags --libs Qt6Core) -o "$work/fixture"
for case in capture-good capture-duplicate capture-phase-advance capture-early capture-purpose capture-stale facts-good facts-duplicate facts-epoch facts-late facts-original-expiry facts-dimension facts-extra facts-authority facts-document facts-page facts-other-page facts-identity facts-png facts-duplicate-json facts-negative facts-reviewer facts-review-extra facts-wrong-review-hash facts-missing-review held-replacement; do
 root="$work/rmb-qt-probe-0123456789abcdef0123456789abcdef"
 mkdir -m700 "$root"
 printf 'Owned fixture data only; not an ELF extension.\n' > "$root/payload.so"
 chmod 600 "$root/payload.so"
 if [ -n "${QT_PROBE_SDK_ENV:-}" ]; then
  LD_PRELOAD="$root/payload.so" /opt/codex/rm2/5.8.203/sysroots/x86_64-codexsdk-linux/usr/bin/qemu-arm \
   -L "$SDKTARGETSYSROOT" -E LANG=C.UTF-8 "$work/fixture" "$root" "$case"
 else LD_PRELOAD="$root/payload.so" "$work/fixture" "$root" "$case"; fi
 rm -rf "$root"
done
