"""Explicit strict build inputs only. No APK generation, signing or release writes."""
import argparse
import json
import stat
from pathlib import Path
from apk_builder import copy_verified
from apk_contract import (HEX40, REPOSITORY, RM2_FEATURE, RM2_SDK_SHA,
                          canonical, parse_build, require, stable)
from apk_source_build import build, sha, source_git, source_snapshot

TARGETS = [('armv7', 'armv7-unknown-linux-gnueabihf'),
           ('aarch64', 'aarch64-unknown-linux-gnu')]
MEMBERS = ['reader-buddy', 'build.json', 'private-evidence.json']
COMPLETION = 'build-input-receipt.json'
MAX_EVIDENCE = 32 * 1024 * 1024

def encoded(value):
    return (json.dumps(value, sort_keys=True, separators=(',', ':'), allow_nan=False) + '\n').encode()

def write_new(path, raw):
    with path.open('xb') as stream:
        stream.write(raw)

def read_regular(path, limit):
    info = path.lstat()
    require(stat.S_ISREG(info.st_mode) and 0 < info.st_size <= limit, 'bounded regular build input')
    with path.open('rb') as stream:
        raw = stream.read(limit + 1)
    require(len(raw) == info.st_size, 'build input changed while reading')
    return raw

def names(path, maximum):
    result = set()
    for child in path.iterdir():
        result.add(child.name)
        require(len(result) <= maximum, 'build input inventory bound')
    return result

def _verify(directory, tag, commit, producer_commit, completion):
    """Recheck exact retained bytes/metadata, not provenance of caller-created JSON."""
    require(stable(tag) and HEX40.fullmatch(commit) and HEX40.fullmatch(producer_commit), 'expected input identity')
    require(not directory.is_symlink() and directory.is_dir(), 'regular input root')
    require(names(directory, 3) == {'armv7', 'aarch64', completion}, 'exact build input roots')
    raw = read_regular(directory / completion, 65536)
    receipt = json.loads(raw)
    require(encoded(receipt) == raw, 'canonical build input completion')
    require(set(receipt) == {'schema', 'status', 'eligibility', 'producer_commit', 'source', 'files'}, 'completion fields')
    require(type(receipt['schema']) is int and receipt['schema'] == 1 and
            receipt['status'] == 'strict-build-inputs-only' and receipt['eligibility'] == 'not-qualified' and
            receipt['producer_commit'] == producer_commit, 'completion identity')
    require(set(receipt['files']) == {f'{arch}/{name}' for arch, _ in TARGETS for name in MEMBERS}, 'completion inventory')
    sdk = None
    for arch, target in TARGETS:
        folder = directory / arch
        require(not folder.is_symlink() and folder.is_dir() and names(folder, 3) == set(MEMBERS), 'target input inventory')
        for name, limit in zip(MEMBERS, [134217728, 65536, MAX_EVIDENCE]):
            path = folder / name
            info = path.lstat()
            require(stat.S_ISREG(info.st_mode) and 0 < info.st_size <= limit, 'bounded regular input')
            expected = receipt['files'][f'{arch}/{name}']
            require(set(expected) == {'bytes', 'sha256'} and type(expected['bytes']) is int and
                    info.st_size == expected['bytes'] and sha(path) == expected['sha256'], 'input bytes changed')
        metadata = parse_build(read_regular(folder / 'build.json', 65536))
        require(metadata['source'] == receipt['source'] and metadata['source']['repository'] == REPOSITORY and
                metadata['source']['tag'] == tag and metadata['source']['sha'] == commit, 'source correspondence')
        require(metadata['target']['rust_target'] == target and
                (RM2_FEATURE in metadata['cargo']['features']) == (arch == 'armv7'), 'target/profile correspondence')
        require(metadata['binary']['sha256'] == receipt['files'][f'{arch}/reader-buddy']['sha256'] and
                metadata['binary']['bytes'] == receipt['files'][f'{arch}/reader-buddy']['bytes'], 'binary correspondence')
        current_sdk = metadata['sdk']
        require(current_sdk is not None and current_sdk['ref_kind'] == 'tagged-git' and
                current_sdk['tag'] == 'v0.2.0' and current_sdk['sha'] == RM2_SDK_SHA, 'selected SDK correspondence')
        require(sdk is None or sdk == current_sdk, 'cross-target SDK changed')
        sdk = current_sdk
    return receipt

def verify(directory, tag, commit, producer_commit):
    return _verify(directory, tag, commit, producer_commit, COMPLETION)

def producer_identity(producer, expected):
    require((producer / '.git').is_dir() and
            source_git(producer, 'rev-parse', '--is-shallow-repository') == 'false' and
            source_git(producer, 'rev-parse', 'HEAD') == expected and
            source_git(producer, 'status', '--porcelain') == '' and
            source_git(producer, 'remote', 'get-url', 'origin').removesuffix('.git') == REPOSITORY, 'exact clean producer checkout')

def produce(args):
    source, sdk = args.source.resolve(), args.sdk_source.resolve()
    target, output = args.target_dir.resolve(), args.output.resolve()
    producer = Path(__file__).resolve().parents[2]
    require(stable(args.tag) and HEX40.fullmatch(args.source_sha) and HEX40.fullmatch(args.producer_sha), 'source/tag arguments')
    require(target != output and not target.is_relative_to(output) and not output.is_relative_to(target), 'disjoint build/output roots')
    for path in (target, output):
        require(not path.exists() and not path.is_symlink(), 'fresh output root')
        require(all(not path.is_relative_to(root) and not root.is_relative_to(path)
                    for root in (source, sdk, producer)), 'external output roots')
    producer_identity(producer, args.producer_sha)
    identity = source_snapshot(source, args.tag)['source']
    require(identity['sha'] == args.source_sha, 'expected release commit')
    target.mkdir()
    output.mkdir()
    (target / 'diagnostics').mkdir()
    receipt = dict(schema=1, status='strict-build-inputs-only', eligibility='not-qualified',
                   producer_commit=args.producer_sha, source=identity, files={})
    for arch, rust_target in TARGETS:
        binary, metadata, private = build(source, args.tag, rust_target, target / arch,
            producer / 'cicd/release/Cross.apk.toml', sdk_tag='v0.2.0',
            rm2_resident=arch == 'armv7', sdk_tag_source=sdk,
            diagnostics=target / 'diagnostics' / arch)
        require(metadata['source'] == identity, 'build source changed')
        folder = output / arch
        folder.mkdir()
        copy_verified(binary, folder / 'reader-buddy', metadata['binary']['bytes'], metadata['binary']['sha256'])
        write_new(folder / 'build.json', canonical(metadata))
        evidence = encoded(private)
        require(len(evidence) <= MAX_EVIDENCE, 'private build evidence bound')
        write_new(folder / 'private-evidence.json', evidence)
        for name in MEMBERS:
            path = folder / name
            receipt['files'][f'{arch}/{name}'] = dict(bytes=path.stat().st_size, sha256=sha(path))
    # Validate a candidate manifest before publishing the authoritative marker.
    # The final check also rehashes every retained file after publication.
    producer_identity(producer, args.producer_sha)
    pending = output / 'completion-pending.json'
    write_new(pending, encoded(receipt))
    _verify(output, args.tag, args.source_sha, args.producer_sha, pending.name)
    # Exclusive hard-link publication cannot replace an existing marker.
    import os
    os.link(pending, output / COMPLETION)
    pending.unlink()
    return verify(output, args.tag, args.source_sha, args.producer_sha)

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--source-sha', required=True)
    parser.add_argument('--tag', required=True)
    parser.add_argument('--sdk-source', type=Path, required=True)
    parser.add_argument('--producer-sha', required=True)
    parser.add_argument('--target-dir', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    result = produce(args)
    print(json.dumps({'status': result['status'], 'eligibility': result['eligibility'],
                      'receipt_sha256': sha(args.output / COMPLETION)}, sort_keys=True))
