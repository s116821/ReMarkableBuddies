#!/bin/sh
set -eu
if [ -n "${QT_PROBE_SDK_ENV:-}" ]; then . "$QT_PROBE_SDK_ENV"; fi
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
${CXX:-c++} -std=c++17 -Wall -Wextra -Werror -fPIC -fsyntax-only tools/native_page_facts_probe/facts-request-publisher.cpp $(pkg-config --cflags Qt6Core)
${CXX:-c++} -std=c++17 -Wall -Wextra -Werror -O2 -fPIC \
 tools/native_page_facts_probe/publisher-fixture.cpp \
 $(pkg-config --cflags --libs Qt6Core) -o "$work/fixture"
for case in good duplicate malformed late wrong-executable unmapped closed restoring callback request-existing temporary-existing owner-mode waiting-mode directory-mode waiting-symlink generation wrong-payload payload-fifo locked; do
 root="$work/rmb-qt-probe-0123456789abcdef0123456789abcdef"
 mkdir -m 700 "$root"
 printf 'OWNED FIXTURE DATA ONLY; not an ELF or executable extension.\n' > "$root/payload.so"
 chmod 600 "$root/payload.so"
 if [ -n "${QT_PROBE_SDK_ENV:-}" ]; then
  LD_PRELOAD="$root/payload.so" /opt/codex/rm2/5.8.203/sysroots/x86_64-codexsdk-linux/usr/bin/qemu-arm \
   -L "$SDKTARGETSYSROOT" -E LANG=C.UTF-8 -E QT_QPA_PLATFORM=offscreen "$work/fixture" "$root" "$case"
 else LD_PRELOAD="$root/payload.so" "$work/fixture" "$root" "$case"; fi
 rm -rf "$root"
done
