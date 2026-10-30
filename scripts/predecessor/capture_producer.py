"""Bind an existing Cargo-produced application command to its real CI inputs."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tomllib


def verify_build_identity(compiled, expected, tree, head_tree, declared_features, actual_features):
    if compiled != expected:
        raise ValueError('compiled commit differs from declared CI checkout')
    if head_tree != tree:
        raise ValueError('compiled merge and declared head trees must match before crediting head')
    if not set(declared_features).issubset(set(actual_features)):
        raise ValueError('all declared features must be present')


def select_command(messages, workspace):
    workspace = Path(workspace).resolve()
    expected = workspace / 'target/debug/reader-buddy'
    found = {}
    for line in messages.splitlines():
        try:
            value = json.loads(line)
        except (ValueError, TypeError):
            continue
        if not isinstance(value, dict) or value.get('reason') != 'compiler-artifact':
            continue
        target = value.get('target', {})
        if target.get('name') != 'reader-buddy' or target.get('kind') != ['bin'] or value.get('profile', {}).get('test') is not False:
            continue
        if Path(value.get('executable') or '').resolve() != expected:
            continue
        if Path(target.get('src_path') or '').resolve() != workspace / 'src/main.rs':
            continue
        record = {key: value[key] for key in ['package_id', 'target', 'profile', 'features', 'executable']}
        found[json.dumps(record, sort_keys=True)] = record
    if len(found) != 1:
        raise ValueError('one unambiguous normal reader-buddy compiler artifact required')
    return next(iter(found.values()))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--workspace', required=True)
    parser.add_argument('--cargo-messages', required=True)
    parser.add_argument('--source-sha', required=True)
    parser.add_argument('--source-head', required=True)
    parser.add_argument('--output', required=True)
    args = parser.parse_args()
    workspace = Path(args.workspace).resolve()

    def git(*arguments):
        return subprocess.check_output(['git', '-C', str(workspace), *arguments], text=True).strip()

    compiled = git('rev-parse', 'HEAD')
    tree = git('rev-parse', 'HEAD^{tree}')
    head_tree = git('rev-parse', args.source_head + '^{tree}')
    messages = Path(args.cargo_messages).read_bytes()
    artifact = select_command(messages.decode(), workspace)
    manifest = (workspace / 'Cargo.toml').read_bytes()
    declared_features = set(tomllib.loads(manifest.decode()).get('features', {}))
    verify_build_identity(compiled, args.source_sha, tree, head_tree, declared_features, artifact['features'])
    result = {
        'current_source': compiled, 'current_head': args.source_head, 'current_tree': tree,
        'head_tree_equivalent': True, 'producer_command': 'cargo test --locked --all-features --message-format=json',
        'cargo_artifact': artifact, 'cargo_messages_sha256': hashlib.sha256(messages).hexdigest(),
        'cargo_lock_sha256': hashlib.sha256((workspace / 'Cargo.lock').read_bytes()).hexdigest(),
        'manifest_sha256': hashlib.sha256(manifest).hexdigest(),
        'current_binary_sha256': hashlib.sha256(Path(artifact['executable']).read_bytes()).hexdigest(),
    }
    with Path(args.output).open('x', encoding='utf-8') as output:
        json.dump(result, output, indent=2)


if __name__ == '__main__':
    main()
