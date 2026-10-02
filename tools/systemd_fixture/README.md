# Exact upstream systemd 255.21 parser fixture

This original tooling builds upstream systemd-stable tag v255.21, peeled commit
70500d37992a01d3275b1c414c3ed161d6f91f9e, and exercises its offline test mode.
It uses disposable fake units, an unprivileged parser, empty generator paths,
and no tablet, host mounts, device mappings, or external network at run time.
No unit or job executes. The tablet vendor patch set is unknown; matching the
reported version does not establish identical vendor behavior.

From the repository root:

```sh
docker build -t rem25-systemd-25521:host-fixture tools/systemd_fixture
docker run --rm --network none --cgroupns private rem25-systemd-25521:host-fixture python3 /opt/buddy/parser_fixture.py
```

Build requires network access and Ubuntu package repositories. The upstream
source commit is pinned; the Ubuntu image and package versions are not pinned,
so this is source reproducibility, not a byte-reproducible build. The report
records compiler and installed systemd executable SHA-256 for each run. Systemd
retains its upstream licenses in /opt/systemd-source; no upstream source or
binary is committed here. These scripts follow this repository's license.

The assertions cover:

- Empty OnFailure followed by an owned handler preserves the vendor handler,
  while Restart=no takes effect.
- A runtime full fragment still inherits the vendor unit-specific drop-in.
- A separate named unit has no handler when none is configured.
- A separate named unit inherits a generic service.d handler.

Each scenario verifies the vendor fragment remains byte-identical. Dump hashes
are evidence identifiers, not deterministic golden values: process metadata in
systemd dumps may vary.

The October 2 host run passed all four scenarios using GCC 13.3.0 and executable
SHA-256 f132ac4469673ce3aead14c70b286ebfab2d0e22f572d25d95040b7b7f54f4be.
Image SHA-256 was 22f2673ae3341f7ffde895aa9bcf89e9fa3d745829107239d45569c2005395cd.
The full report is retained in the coordinator's outputs directory.

Live-manager experiments could not allocate their own manager cgroup on this
Docker host's read-only cgroup v1 hierarchy. Scoped system-manager and private
user-manager attempts failed and their owned containers were removed. No host
cgroups were modified. This fixture therefore does not qualify job cancellation,
stop deadlines, failure-handler execution, service coupling, watchdog recovery,
or any tablet activation. E1 remains unapproved.
