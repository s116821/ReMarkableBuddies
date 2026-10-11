"""Reviewed experimental capture contract; contains no proprietary native ABI."""
import hashlib
import re
import shlex

TOOLS = {
    "native_frame_acquire": "eaaf0799c40b70e582fc4c3b395f9e5354b659d5e548861b809874c7cddbd6f5",
    "owned_frame": "fe8a516a63ac90b754c8ca154bb74f5886fb5ffa6a8b9e7827f92d1fa5c41765",
    "owned_frame_alarm": "25e024efa8cdf3ac31fe2159b35c98acb899183c2dec74828881eeb510911b75",
}
SOURCES = {
    "private_acquisition": "dc739913913a05a9386818fee9406df458f3732fd04c9f77fed5db8e14a3a470",
    "converter_library": "9ede6e943e900a60dec17616a22e55c1e6682e8e5dc6226d22de081d80fc26f3",
    "converter_example": "4e0bcff64f46a9efe72fe3c51557513968a84591365cb1603d20409f36b3d08a",
    "private_alarm": "5bf7581b6785c6c2a1d6012d87f339952210e0757c0a6bcfccf804b9e2a88f5e",
}
NATIVE_HASHES = {
    "/usr/bin/xochitl": "071d85beef3ef2d4cc0e11002140b27b82a2cc04a2ed740a5669f591069b77df",
    "/usr/lib/libQt6Quick.so.6.10.3": "b20f9adaefe8cebeca445906e891370f75f0b0af878ac1b35e0c91159778628f",
    "/usr/lib/libQt6Gui.so.6.10.3": "93fe582cc61673342ca49e12306d7f689860016582fa46ce135ffa280a972839",
}
PROVIDERS = {
    "/lib/libc.so.6": "8d741a0db9727c97faebfd17e7f8409183c382241ff3d4c9b55a52e98d8dd832",
    "/lib/libm.so.6": "78827bb885c07dde4d99c87abb597aa392f7b892ad9d1dd5d09ff781a548a20e",
    "/lib/libgcc_s.so.1": "122abba5c29fb001c1de440e54b9845aa45be2d88650692a87aeea4969c644c5",
    "/lib/ld-linux-armhf.so.3": "9e7e72e7d488db4c885eac6a360132690771a0308e8f56e187710d07a6b46a62",
    "/lib/libpthread.so.0": "4a9c1f5b2270484f4c02546c7ec64925d0bae7781f3a0a658377fb0301ecdfee",
    "/lib/libdl.so.2": "f0aa5ff0ab3e889fe3ec9f0ba59d3c7c0c4fbcb501ecc195a10ff3cb73d62989",
}
CONTRACT = {"kind": "native-owned-rgb32-scoped-v1", "frame_bytes": 10513468,
            "map_closure": "frozen-exact-dependencies-provider-rows-monotonic-heap-v1",
            "wrapper_maps": "bounded-context-only",
            "maximum_producer_stderr_bytes": 525824,
            "source_sha256": SOURCES, "native_hashes": NATIVE_HASHES,
            "provider_hashes": PROVIDERS, "minimum_initial_free_kib": 196608,
            "maximum_ready_frames": 3, "maximum_open_frames": 5,
            "maximum_response_seconds": 20, "pin_checks": "ordered-alarm-batches-v1",
            "hash_tool_sha256": "df59cc89ea72164ae92b15db4ef089acae776eda5b373bf10e2fcc46b49c8c02"}


def hash_tool_checks():
    return ('test "$(readlink -f /usr/bin/sha256sum)" = /usr/bin/busybox.nosuid\n'
            'test -f /usr/bin/busybox.nosuid && test ! -L /usr/bin/busybox.nosuid\n'
            'test "$(stat -c \'%a %u %h %s\' /usr/bin/busybox.nosuid)" = \'755 0 1 605104\'\n'
            'test "$(sha256sum /usr/bin/busybox.nosuid | cut -d " " -f 1)" = df59cc89ea72164ae92b15db4ef089acae776eda5b373bf10e2fcc46b49c8c02\n')


def pinned_hash_checks(output):
    # Bootstrap before using the launcher; all native/provider paths are still
    # checked at both original checkpoints, with retained exact ordered output.
    text = hash_tool_checks()
    text += f'test "$(sha256sum "$root/owned_frame_alarm" | cut -d " " -f 1)" = {TOOLS["owned_frame_alarm"]}\n'
    paths, expected = [], []
    for path, digest in (NATIVE_HASHES | PROVIDERS).items():
        selected = '"/proc/$pid/exe"' if path == '/usr/bin/xochitl' else shlex.quote(path)
        paths.append(selected)
        expected.append(digest + '  ' + ('/proc/$pid/exe' if path == '/usr/bin/xochitl' else path))
    text += f'test ! -e "{output}"\ntest ! -L "{output}"\n'
    text += f'test ! -e "{output}.stderr"\ntest ! -L "{output}.stderr"\n'
    text += f'"$root/owned_frame_alarm" --hashes /usr/bin/sha256sum {" ".join(paths)} > "{output}" 2> "{output}.stderr"\n'
    for selected in (output, output + '.stderr'):
        text += f'private "{selected}"\ntest "$(stat -c \'%s\' "{selected}")" -le 2048\n'
    text += f'test ! -s "{output}.stderr"\n'
    # The only interpolation is the validated decimal PID, not input text.
    text += 'printf \'%s\\n\' "' + '\n'.join(expected) + f'" | cmp - "{output}"\n'
    return text + hash_tool_checks()


def capture_script(prefix, nonce, boot):
    if any(not isinstance(pin, str) or not re.fullmatch('[0-9a-f]{64}', pin)
           for pin in (TOOLS['native_frame_acquire'], SOURCES['private_acquisition'])):
        raise ValueError('Reviewed scoped producer source and artifact required')
    if not re.fullmatch(r'host-(?:ready-00[0-2]|trigger|result|open-00[0-4])', prefix):
        raise ValueError('Exact exclusive capture prefix required')
    if not re.fullmatch('[0-9a-f]{32}', nonce) or not re.fullmatch(r'[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}', boot):
        raise ValueError('Canonical capture nonce and boot required')
    s = f'private "$root/{prefix}.claim"\ntest "$(cat "$root/{prefix}.claim")" = {nonce}\n' + pinned_hash_checks(f'$root/{prefix}.pins-before')
    s += 'test "$(df -Pk /run | awk \'NR==2 {print $4}\')" -ge 32768\n'
    for suffix in ('frame', 'producer-exit', 'producer-stderr', 'capture', 'converter-exit', 'converter-stderr', 'images', 'maps-before', 'maps-after'):
        s += f'test ! -e "$root/{prefix}.{suffix}"\ntest ! -L "$root/{prefix}.{suffix}"\n'
    s += f'''test "$(now_upper)" -le $((cutoff-4))
cat /proc/$pid/maps > "$root/{prefix}.maps-before"
test "$(stat -c '%s' "$root/{prefix}.maps-before")" -le 262144
test "$(now_upper)" -le $((cutoff-4))
set +e
"$root/native_frame_acquire" "$pid" "$start" {boot} {nonce} > "$root/{prefix}.frame" 2> "$root/{prefix}.producer-stderr"
producer=$?
set -e
printf '%s\\n' "$producer" > "$root/{prefix}.producer-exit"
test "$(stat -c '%s' "$root/{prefix}.producer-stderr")" -le 525824
test "$producer" = 0
test ! -s "$root/{prefix}.producer-stderr"
cat /proc/$pid/maps > "$root/{prefix}.maps-after"
test "$(stat -c '%s' "$root/{prefix}.maps-after")" -le 262144
private "$root/{prefix}.frame"
test "$(stat -c '%s' "$root/{prefix}.frame")" = 10513468
frame_hash=$(sha256sum "$root/{prefix}.frame" | cut -d ' ' -f 1)
test "$(now_upper)" -le $((cutoff-6))
set +e
"$root/owned_frame_alarm" "$root/owned_frame" "$root/{prefix}.frame" "$root/{prefix}.images" {nonce} "$pid" "$start" {boot} "$frame_hash" > "$root/{prefix}.capture" 2> "$root/{prefix}.converter-stderr"
converter=$?
set -e
printf '%s\\n' "$converter" > "$root/{prefix}.converter-exit"
test "$converter" = 0
test ! -s "$root/{prefix}.converter-stderr"
private "$root/{prefix}.capture"
private "$root/{prefix}.images/native.png"
private "$root/{prefix}.images/overview.png"
test "$(stat -c '%s' "$root/{prefix}.images/native.png")" -le 2700000
test "$(stat -c '%s' "$root/{prefix}.images/overview.png")" -le 850000
test "$(sha256sum "$root/{prefix}.frame" | cut -d ' ' -f 1)" = "$frame_hash"
native_hash=$(sha256sum "$root/{prefix}.images/native.png" | cut -d ' ' -f 1)
cp "$root/{prefix}.images/overview.png" "$root/{prefix}.png"
test "$(now_upper)" -lt "$cutoff"
'''
    return s + pinned_hash_checks(f'$root/{prefix}.pins-after')


def validate_receipt(receipt, nonce, frame_hash, png, native_hash):
    if not re.fullmatch('[0-9a-f]{64}', frame_hash) or not re.fullmatch('[0-9a-f]{64}', native_hash):
        raise ValueError('Closed complete frame hash required')
    match = re.fullmatch(r'nonce=([0-9a-f]{32}) frame_sha256=([0-9a-f]{64}) native_sha256=([0-9a-f]{64}) overview_sha256=([0-9a-f]{64}) conversion_complete=true native_authority=false\n', receipt)
    if not match or match[1] != nonce or match[2] != frame_hash or match[3] != native_hash or match[4] != hashlib.sha256(png).hexdigest():
        raise ValueError('Closed producer/converter/PNG binding refused')
    return {'frame_sha256': frame_hash, 'native_sha256': match[3],
            'overview_sha256': match[4], 'producer_exit': 0, 'converter_exit': 0,
            'native_authority': False}
