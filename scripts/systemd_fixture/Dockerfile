FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    systemd systemd-sysv dbus python3 meson ninja-build build-essential pkg-config \
    gperf libcap-dev libmount-dev libseccomp-dev libselinux1-dev libpam0g-dev \
    libaudit-dev libblkid-dev libkmod-dev libcryptsetup-dev libssl-dev git ca-certificates
RUN git clone --depth 1 --branch v255.21 https://github.com/systemd/systemd-stable.git /opt/systemd-source \
    && test "$(git -C /opt/systemd-source rev-parse HEAD)" = 70500d37992a01d3275b1c414c3ed161d6f91f9e
RUN apt-get install -y --no-install-recommends python3-jinja2
RUN meson setup /opt/systemd-build /opt/systemd-source --prefix=/usr \
    -Dmode=release -Dman=false -Dtests=false -Dnetworkd=false -Dresolve=false \
    -Dtimesyncd=false -Ddefault-hierarchy=legacy \
    && ninja -C /opt/systemd-build -j 4 \
    && ninja -C /opt/systemd-build install \
    && systemctl --version | head -n 1
# Only this disposable container may run its own manager. No host mounts/device
# mappings are part of the fixture; its later runner must require private cgroups.
ENV container=docker
COPY parser_fixture.py /opt/buddy/parser_fixture.py
CMD ["python3", "/opt/buddy/parser_fixture.py"]
