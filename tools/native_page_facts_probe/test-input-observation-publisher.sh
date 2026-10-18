#!/bin/sh
set -eu
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
c++ -std=c++17 -Wall -Wextra -Werror -O2 -fPIC tools/native_page_facts_probe/input-observation-publisher-fixture.cpp $(pkg-config --cflags --libs Qt6Core) -o "$work/fixture"
for mode in good zero leading-zero overflow width elapsed-leading-zero elapsed-boundary elapsed-last purpose oversize symlink fifo mode closed restore cross-facts existing-end existing-tmp; do
 root="$work/$mode/rmb-qt-probe-0123456789abcdef0123456789abcdef"
 mkdir -p "$root"
 chmod 700 "$root"
 INPUT_ROOT="$root" LD_PRELOAD="$root/payload.so" "$work/fixture" "$mode"
done
