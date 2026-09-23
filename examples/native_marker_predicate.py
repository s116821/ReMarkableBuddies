"""Offline diagnostic only. Requires rmscene==0.8.0; never edits native pages.

Pinned RM2 disposable PDF viewport calibration, not a general native transform.
Unknown records are compared opaquely, not claimed to be understood.
"""
import dataclasses
import hashlib
import io
import json
import math
from pathlib import Path

LIMIT = 8 * 1024 * 1024
# From the retained left/right sentinel endpoints on the same fixed viewport.
SX, TX = 2.5122531797827743, -964.795145174352
SY, TY = 2.5302505493164062, -55.32414245605469
# Independent top-sentinel and selection-X endpoints: maximum residual .681
# native units. One native unit (<.4 screen px) is the fixed quantization bound.
TOLERANCE = 1.0


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def bounded(path):
    with Path(path).open("rb") as stream:
        data = stream.read(LIMIT + 1)
    require(len(data) <= LIMIT, "native file exceeds 8MiB")
    return data


def decode(data):
    from rmscene.scene_stream import read_blocks, read_tree, SceneLineItemBlock, PageInfoBlock
    from rmscene.scene_items import Line
    blocks = list(read_blocks(io.BytesIO(data)))
    require(len(blocks) <= 100_000, "too many blocks")
    lines, opaque = {}, []
    for block in blocks:
        if isinstance(block, SceneLineItemBlock):
            if block.item.value is None:
                # Historical deletion records may already exist. Require exact
                # opaque equality across snapshots, never interpret new ones.
                opaque.append(block)
                continue
            require(isinstance(block.item.value, Line) and not block.item.deleted_length,
                    "line tombstone or ambiguous line item")
            key = str(block.item.item_id)
            require(key not in lines, "duplicate line ID")
            lines[key] = dataclasses.asdict(block.item.value)
        elif not isinstance(block, PageInfoBlock):
            # Includes unsupported blocks and tombstones: no added/changed ones.
            opaque.append(block)
    live = [dataclasses.asdict(x) for x in read_tree(io.BytesIO(data)).walk() if isinstance(x, Line)]
    require(len(lines) == len(live) and all(v in live for v in lines.values()),
            "block/tree live-line interpretations differ")
    return lines, opaque


def native(point):
    return (point[0] * SX + TX, point[1] * SY + TY)


def projections(point, path):
    offset = 0.0
    for a, b in zip(path, path[1:]):
        dx, dy = b[0] - a[0], b[1] - a[1]
        length = math.hypot(dx, dy)
        if not length:
            continue
        t = min(1.0, max(0.0, ((point[0] - a[0]) * dx + (point[1] - a[1]) * dy) / length**2))
        distance = math.hypot(point[0] - a[0] - t * dx, point[1] - a[1] - t * dy)
        yield distance, offset + t * length
        offset += length


def samples(path):
    for a, b in zip(path, path[1:]):
        count = max(1, math.ceil(math.dist(a, b) / .5))
        for index in range(count):
            t = index / count
            yield (a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1]))
    yield path[-1]


def follows(actual, expected):
    if len(actual) < 2 or len(actual) > 128 or sum(math.dist(a, b) for a, b in zip(actual, actual[1:])) > 512:
        return False
    if math.dist(actual[0], expected[0]) > TOLERANCE or math.dist(actual[-1], expected[-1]) > TOLERANCE:
        return False
    # Both trajectories must cover one another in order; rejects detours, gaps,
    # reversal and mere endpoint/ROI matches. Conservative ambiguity refusal.
    for source, target in [(actual, expected), (expected, actual)]:
        previous = 0.0
        for point in samples(source):
            candidates = [arc for distance, arc in projections(point, target)
                          if distance <= TOLERANCE and arc >= previous - TOLERANCE]
            if not candidates:
                return False
            previous = max(previous, min(candidates))
    return True


def validate_lines(before, after, screen_paths):
    require(len(screen_paths) == 10 and all(2 <= len(p) <= 25 for p in screen_paths), "expected ten bounded paths")
    require(all(after.get(k) == value for k, value in before.items()), "baseline line removed or changed")
    added = {k: v for k, v in after.items() if k not in before}
    require(len(added) == 10, "expected ten new marker lines")
    expected = [[native(p) for p in path] for path in screen_paths]
    matched = set()
    for line in added.values():
        require(line['tool'] == 17 and line['color'] == 0 and line['color_rgba'] is None
                and line['move_id'] is None and line['starting_length'] == 0.0
                and abs(line['thickness_scale'] - 2.748575448989868) < 1e-9,
                "unexpected native marker style")
        require(all(p['width'] == 22 for p in line['points']), "unexpected point width")
        actual = [(p['x'], p['y']) for p in line['points']]
        require(all(math.isfinite(x) and math.isfinite(y) and
                    698 <= (x-TX)/SX <= 747 and 934 <= (y-TY)/SY <= 983
                    for x, y in actual), "marker outside owned area")
        matches = [i for i, path in enumerate(expected) if follows(actual, path)]
        require(len(matches) == 1 and matches[0] not in matched, "no unique ordered marker-path match")
        matched.add(matches[0])
    return sorted(added)


def validate(initial, candidate, expected):
    before, opaque_before = decode(initial)
    after, opaque_after = decode(candidate)
    require(opaque_before == opaque_after, "opaque non-PageInfo records changed")
    added = validate_lines(before, after, expected['paths'])
    return {'run': expected['run'], 'sha256': hashlib.sha256(candidate).hexdigest(),
            'added_ids': added, 'original_lines': len(before), 'tolerance_native': TOLERANCE,
            'limit': 'Recognized lines plus opaque equality; PageInfo metadata excluded; fixed disposable viewport only'}


if __name__ == '__main__':
    import sys
    require(len(sys.argv) == 4, 'usage: native_marker_predicate.py INITIAL CANDIDATE EXPECTED_JSON')
    require(Path(sys.argv[3]).stat().st_size <= 8192, 'expected geometry too large')
    print(json.dumps(validate(bounded(sys.argv[1]), bounded(sys.argv[2]),
                              json.loads(Path(sys.argv[3]).read_text())), indent=2))
