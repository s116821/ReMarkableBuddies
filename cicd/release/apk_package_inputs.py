"""Explicit test-key packaging of independently accepted strict build inputs.

Never compiles or executes a target binary. This is not release/install authority.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import stat

from apk_build_inputs import (COMPLETION, MAX_EVIDENCE, producer_identity,
                              read_regular, verify)
from apk_builder import (IMAGE, MODULES, TOOL_SHA, copy_verified,
                         generate_and_observe)
from apk_contract import HEX40, HEX64, RM2_BUNDLE_SHA, integer, parse_build, require
from apk_payload import BUNDLE_FILE, TOOL_FILES
from apk_source_build import (run, sdk_release_source, sha, source_license,
                              source_snapshot)


def normalized(value):
    """Compare every proof field, allowing only JSON tuple/list equivalence."""
    return json.loads(json.dumps(value, sort_keys=True, allow_nan=False))


def staged_inventory(directory):
    require(directory.is_dir() and not directory.is_symlink(), 'regular staged root')
    result = {}
    for path in directory.rglob('*'):
        require(len(result) < 128, 'staged inventory bound')
        mode = path.lstat().st_mode
        require(stat.S_ISDIR(mode) or stat.S_ISREG(mode), 'regular staged inputs/modules')
        result[str(path.relative_to(directory))] = None if stat.S_ISDIR(mode) else sha(path)
    return result


def produce(args):
    source, sdk = args.source.resolve(), args.sdk_source.resolve()
    original = args.build_producer_source.resolve()
    executor = Path(__file__).resolve().parents[2]
    require(not args.build_inputs.is_symlink(), 'regular input root')
    retained = args.build_inputs.resolve()
    for value in (args.source_sha, args.build_producer_sha, args.executor_sha):
        require(isinstance(value, str) and HEX40.fullmatch(value), 'expected source/producer/executor commit')
    require(isinstance(args.expected_receipt_sha256, str) and
            HEX64.fullmatch(args.expected_receipt_sha256), 'external completion digest')
    require(integer(args.pkgrel, 0, 65535), 'pkgrel')
    require(original != executor, 'separate original producer and packaging executor')
    for path in (args.work_dir, args.output):
        require(not path.exists() and not path.is_symlink(), 'fresh packaging roots')
    work, output = args.work_dir.resolve(), args.output.resolve()
    protected = (source, sdk, original, executor, retained)
    for path in (work, output):
        require(all(not path.is_relative_to(root) and not root.is_relative_to(path)
                    for root in protected), 'external packaging roots')
    require(not work.is_relative_to(output) and not output.is_relative_to(work), 'disjoint packaging roots')

    def checked_receipt():
        raw = read_regular(retained / COMPLETION, 65536)
        require(hashlib.sha256(raw).hexdigest() == args.expected_receipt_sha256, 'external completion digest mismatch')
        result = verify(retained, args.tag, args.source_sha, args.build_producer_sha)
        require(read_regular(retained / COMPLETION, 65536) == raw and json.loads(raw) == result,
                'completion changed during admission')
        return result

    receipt = checked_receipt()
    producer_identity(original, args.build_producer_sha)
    producer_identity(executor, args.executor_sha)
    snapshot = source_snapshot(source, args.tag)
    require(snapshot['source'] == receipt['source'] and snapshot['working'] == snapshot['committed'],
            'actual release source correspondence')
    license_id = source_license(source)
    license_path = source / 'LICENSE'
    license_bytes, license_sha = license_path.stat().st_size, sha(license_path)
    require(0 < license_bytes <= 65536, 'bounded source license')
    builds, private = {}, {}
    for arch in ('armv7', 'aarch64'):
        builds[arch] = parse_build(read_regular(retained / arch / 'build.json', 65536))
        private[arch] = json.loads(read_regular(retained / arch / 'private-evidence.json', MAX_EVIDENCE))
        admission = private[arch].get('source_git_admission')
        expected_source = dict(tag_oid=snapshot['tag_oid'], tracked_files=len(snapshot['working']),
            file_identity_sha256=snapshot['file_identity'], raw_committed_match=True,
            working_inventory_sha256=hashlib.sha256(''.join(snapshot['working']).encode()).hexdigest(),
            committed_inventory_sha256=hashlib.sha256(''.join(snapshot['committed']).encode()).hexdigest())
        require(isinstance(admission, dict) and all(admission.get(k) == v for k, v in expected_source.items()) and
                private[arch].get('source_root') == str(source) and
                private[arch].get('source_license') == license_id, 'retained source/license proof correspondence')
    selected_sdk = builds['armv7']['sdk']
    proof = normalized(sdk_release_source(sdk, selected_sdk['tag'], selected_sdk['sha'],
                                        selected_sdk['manifest_sha256']))
    require(all(normalized(private[arch]['sdk_release_source']) == proof for arch in private),
            'retained independent SDK proof correspondence')

    bundle, tool = args.rm2_native_bundle, args.apk_tool
    require(bundle.is_file() and not bundle.is_symlink() and
            0 < bundle.stat().st_size <= 8 * 1024 * 1024 and sha(bundle) == RM2_BUNDLE_SHA,
            'approved required RM2 native bundle')
    require(tool.name == 'apk' and tool.is_file() and not tool.is_symlink() and sha(tool) == TOOL_SHA,
            'pinned APK tool')
    bundle, tool = bundle.resolve(), tool.resolve()
    require(run(['docker', 'image', 'inspect', IMAGE, '--format', '{{.Id}}'], source) == IMAGE,
            'existing pinned generation image')

    work.mkdir(exist_ok=False)
    inputs, artifacts, modules = (work / name for name in ('inputs', 'artifacts', 'modules'))
    for directory in (inputs, artifacts, modules):
        directory.mkdir()
    helper_root = Path(__file__).resolve().parent
    for name in MODULES:
        shutil.copyfile(helper_root / name, modules / name)
    resident_tools = modules / 'rm2-resident'
    resident_tools.mkdir()
    for name in TOOL_FILES:
        shutil.copyfile(helper_root.parent / 'deploy/rm2-resident' / name, resident_tools / name)
    copy_verified(bundle, inputs / BUNDLE_FILE, bundle.stat().st_size, RM2_BUNDLE_SHA)
    for arch in ('armv7', 'aarch64'):
        directory = inputs / arch
        directory.mkdir()
        for name in ('reader-buddy', 'build.json'):
            record = receipt['files'][f'{arch}/{name}']
            copy_verified(retained / arch / name, directory / name, record['bytes'], record['sha256'])
        copy_verified(license_path, directory / 'LICENSE', license_bytes, license_sha)
    with (inputs / 'input-set.json').open('x', encoding='ascii', newline='\n') as stream:
        json.dump(dict(pkgrel=args.pkgrel, source_license=license_id, targets=['armv7', 'aarch64']),
                  stream, sort_keys=True, separators=(',', ':'))
        stream.write('\n')
    with (work / 'private-source-build-evidence.json').open('x', encoding='utf-8') as stream:
        json.dump(private, stream, sort_keys=True)
    staged_inputs = staged_inventory(inputs)
    staged_modules = staged_inventory(modules)
    input_hashes = {name: digest for name, digest in staged_inputs.items() if digest is not None}

    def recheck():
        require(checked_receipt() == receipt, 'build receipt changed')
        producer_identity(original, args.build_producer_sha)
        producer_identity(executor, args.executor_sha)
        require(source_snapshot(source, args.tag) == snapshot and source_license(source) == license_id and
                sha(license_path) == license_sha, 'release source/license changed')
        require(normalized(sdk_release_source(sdk, selected_sdk['tag'], selected_sdk['sha'],
                                             selected_sdk['manifest_sha256'])) == proof,
                'independent SDK proof changed')
        require(not bundle.is_symlink() and bundle.is_file() and not tool.is_symlink() and tool.is_file() and
                sha(bundle) == RM2_BUNDLE_SHA and sha(tool) == TOOL_SHA, 'packaging tool/bundle changed')
        for directory, expected in ((inputs, staged_inputs), (modules, staged_modules)):
            require(staged_inventory(directory) == expected,
                    'staged packaging inputs/modules changed')

    provenance = dict(receipt_sha256=args.expected_receipt_sha256,
                      original_producer_commit=args.build_producer_sha,
                      packaging_executor_commit=args.executor_sha)
    return generate_and_observe(source, args.tag, args.pkgrel, output, work, inputs, artifacts,
                                modules, tool, input_hashes, receipt['source'], recheck,
                                recheck=recheck, completion_context=provenance)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--build-inputs', type=Path, required=True)
    parser.add_argument('--expected-receipt-sha256', required=True)
    parser.add_argument('--build-producer-source', type=Path, required=True)
    parser.add_argument('--build-producer-sha', required=True)
    parser.add_argument('--executor-sha', required=True)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--source-sha', required=True)
    parser.add_argument('--tag', required=True)
    parser.add_argument('--sdk-source', type=Path, required=True)
    parser.add_argument('--pkgrel', type=int, default=0)
    parser.add_argument('--apk-tool', type=Path, required=True)
    parser.add_argument('--rm2-native-bundle', type=Path, required=True)
    parser.add_argument('--work-dir', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    print(json.dumps(produce(parser.parse_args()), sort_keys=True))
