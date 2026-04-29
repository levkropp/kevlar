# Build ext4.ko / jbd2.ko / mbcache.ko against Ubuntu 26.04's linux-7.0.0-14
# kernel source so the binary ABI matches what our kABI compat layer expects
# (struct layouts disasm-verified against this exact build).
#
# Usage (from repo root):
#   docker build -t kevlar-kabi-modules tools/docker-kabi-modules
#   docker run --rm -v "$PWD/build/kabi-modules:/output" kevlar-kabi-modules
#
# Output: build/kabi-modules/{ext4.ko, jbd2.ko, mbcache.ko}
FROM ubuntu:26.04

ENV DEBIAN_FRONTEND=noninteractive

# Enable source repos (Ubuntu 24.04+ uses deb822 format).
RUN sed -i 's/Types: deb$/Types: deb deb-src/' \
    /etc/apt/sources.list.d/ubuntu.sources

RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        bc \
        bison \
        flex \
        kmod \
        cpio \
        rsync \
        libelf-dev \
        libssl-dev \
        dwarves \
        zstd \
        xz-utils \
        dpkg-dev \
        ca-certificates \
        wget \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

COPY build.sh /build/build.sh
RUN chmod +x /build/build.sh

CMD ["/build/build.sh"]
