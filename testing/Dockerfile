#
#  Busybox
#
FROM ubuntu:20.04 AS busybox
RUN apt-get update && apt-get install -qy build-essential curl sed
RUN apt-get install -qy musl-tools linux-libc-dev
# Make Linux headers available to musl-gcc
RUN ln -sf /usr/include/linux /usr/include/x86_64-linux-musl/linux && \
    ln -sf /usr/include/asm-generic /usr/include/x86_64-linux-musl/asm-generic && \
    ln -sf /usr/include/x86_64-linux-gnu/asm /usr/include/x86_64-linux-musl/asm
RUN curl -fsSL --output tarball.tar.bz2 "https://busybox.net/downloads/busybox-1.36.1.tar.bz2"
RUN mkdir /build && tar xf tarball.tar.bz2 --strip-components=1 -C /build
WORKDIR /build
RUN ln -s /usr/bin/ar /usr/bin/musl-ar \
    && ln -s /usr/bin/strip /usr/bin/musl-strip \
    && make defconfig \
    && sed -i 's/^# CONFIG_STATIC is not set/CONFIG_STATIC=y/' .config \
    && sed -i 's/^CONFIG_CROSS_COMPILER_PREFIX=""/CONFIG_CROSS_COMPILER_PREFIX="musl-"/' .config \
    && sed -i 's/^CONFIG_NANDWRITE=y/# CONFIG_NANDWRITE is not set/' .config \
    && sed -i 's/^CONFIG_NANDDUMP=y/# CONFIG_NANDDUMP is not set/' .config \
    && sed -i 's/^CONFIG_UBIATTACH=y/# CONFIG_UBIATTACH is not set/' .config \
    && sed -i 's/^CONFIG_UBIDETACH=y/# CONFIG_UBIDETACH is not set/' .config \
    && sed -i 's/^CONFIG_UBIMKVOL=y/# CONFIG_UBIMKVOL is not set/' .config \
    && sed -i 's/^CONFIG_UBIRMVOL=y/# CONFIG_UBIRMVOL is not set/' .config \
    && sed -i 's/^CONFIG_UBIRSVOL=y/# CONFIG_UBIRSVOL is not set/' .config \
    && sed -i 's/^CONFIG_UBIUPDATEVOL=y/# CONFIG_UBIUPDATEVOL is not set/' .config \
    && sed -i 's/^CONFIG_UBIRENAME=y/# CONFIG_UBIRENAME is not set/' .config
RUN make -j$(nproc)

#
#  Curl
#
FROM ubuntu:20.04 AS curl
RUN apt-get update && apt-get install -qy build-essential curl sed
RUN apt-get install -qy musl-tools
RUN curl -fsSL --output tarball.tar.xz "https://curl.se/download/curl-8.10.1.tar.xz"
RUN mkdir /build && tar xf tarball.tar.xz --strip-components=1 -C /build
WORKDIR /build
RUN ./configure CC=musl-gcc --without-ssl --without-libpsl --disable-shared --disable-pthreads --disable-threaded-resolver --disable-rtsp --disable-alt-svc --disable-libcurl-option --disable-telnet --disable-gopher --disable-dict --disable-file --disable-ftp --disable-tftp --disable-imap --disable-pop3 --disable-smtp --disable-mqtt --disable-unix-sockets --disable-ldap
RUN make -j$(nproc) curl_LDFLAGS=-all-static

#
#  Dropbear SSH server
#
FROM ubuntu:20.04 AS dropbear
RUN apt-get update && apt-get install -qy build-essential curl sed
RUN apt-get install -qy musl-tools
RUN curl -fsSL --output tarball.tar.bz2 "https://matt.ucc.asn.au/dropbear/releases/dropbear-2024.85.tar.bz2"
RUN mkdir /build && tar xf tarball.tar.bz2 --strip-components=1 -C /build
ADD testing/dropbear/accept-empty-password-root-login.patch /build/accept-empty-password-root-login.patch
ADD testing/dropbear/localoptions.h /build/localoptions.h
WORKDIR /build
RUN sh -c 'patch --ignore-whitespace -p1 < accept-empty-password-root-login.patch'
RUN ./configure CC=musl-gcc --enable-static --disable-largefile --disable-zlib --disable-syslog --disable-wtmp --disable-wtmpx --disable-utmp --disable-utmpx --disable-loginfunc
RUN make -j$(nproc)

#
#  Dynamic linking test binary (musl)
#
FROM ubuntu:20.04 AS dyntest
RUN apt-get update && apt-get install -qy musl-tools
RUN echo '#include <unistd.h>\nint main(){const char m[]="hello from dynamic linking!\\n";write(1,m,sizeof(m)-1);return 0;}' > /hello.c
RUN musl-gcc -o /hello-dynamic /hello.c
RUN cp /usr/lib/x86_64-linux-musl/libc.so /ld-musl-x86_64.so.1

#
#  Bash (static musl)
#
FROM ubuntu:20.04 AS bash
RUN apt-get update && apt-get install -qy build-essential curl musl-tools
RUN curl -fsSL --output tarball.tar.gz "https://ftp.gnu.org/gnu/bash/bash-5.2.37.tar.gz"
RUN mkdir /build && tar xf tarball.tar.gz --strip-components=1 -C /build
WORKDIR /build
# Static musl build: disable everything that needs pthreads, curses, or dlopen.
RUN CC=musl-gcc ./configure \
        --without-bash-malloc \
        --enable-static-link \
        --disable-nls \
        --without-curses \
        --disable-readline
RUN make -j$(nproc) LDFLAGS="-static -Wl,--allow-multiple-definition"
# Keep symbols for debugging; strip later once stable.
# RUN strip bash

#
#  Micro-benchmark suite (musl static)
#
FROM ubuntu:20.04 AS benchsuite
RUN apt-get update && apt-get install -qy musl-tools linux-libc-dev
RUN ln -sf /usr/include/linux /usr/include/x86_64-linux-musl/linux && \
    ln -sf /usr/include/asm-generic /usr/include/x86_64-linux-musl/asm-generic && \
    ln -sf /usr/include/x86_64-linux-gnu/asm /usr/include/x86_64-linux-musl/asm
COPY benchmarks/bench.c /bench.c
RUN musl-gcc -static -O2 -o /bench /bench.c

#
#  Syscall correctness test suite (musl static)
#
FROM ubuntu:20.04 AS testsuite
RUN apt-get update && apt-get install -qy musl-tools linux-libc-dev
RUN ln -sf /usr/include/linux /usr/include/x86_64-linux-musl/linux && \
    ln -sf /usr/include/asm-generic /usr/include/x86_64-linux-musl/asm-generic && \
    ln -sf /usr/include/x86_64-linux-gnu/asm /usr/include/x86_64-linux-musl/asm
COPY tests/test.c /test.c
RUN musl-gcc -static -O2 -o /test /test.c

#
#  M4 integration test (mini-systemd)
#
FROM ubuntu:20.04 AS mini_systemd
RUN apt-get update && apt-get install -qy musl-tools linux-libc-dev
RUN ln -sf /usr/include/linux /usr/include/x86_64-linux-musl/linux && \
    ln -sf /usr/include/asm-generic /usr/include/x86_64-linux-musl/asm-generic && \
    ln -sf /usr/include/x86_64-linux-gnu/asm /usr/include/x86_64-linux-musl/asm
COPY testing/mini_systemd.c /mini_systemd.c
RUN musl-gcc -static -O2 -o /mini-systemd /mini_systemd.c

#
#  Disk hello binary (tiny static binary for exec-from-disk test)
#
FROM ubuntu:20.04 AS disk_hello
RUN apt-get update && apt-get install -qy musl-tools
COPY testing/disk_hello.c /disk_hello.c
RUN musl-gcc -static -O2 -o /disk_hello /disk_hello.c

#
#  Ext2 disk image for Phase 7 integration tests (16 MiB)
#
FROM ubuntu:20.04 AS disk_image
RUN apt-get update && apt-get install -qy e2fsprogs
COPY --from=disk_hello /disk_hello /disk_root/hello
RUN printf 'hello from ext2\n' > /disk_root/greeting.txt && \
    mkdir -p /disk_root/subdir && \
    printf 'nested file\n' > /disk_root/subdir/nested.txt && \
    ln -s greeting.txt /disk_root/link.txt && \
    chmod +x /disk_root/hello && \
    dd if=/dev/zero of=/disk.img bs=1M count=16 2>/dev/null && \
    mke2fs -t ext2 -d /disk_root /disk.img

#
#  M6 Phase 3 threading integration test (static musl + pthreads)
#
FROM ubuntu:20.04 AS mini_threads
RUN apt-get update && apt-get install -qy musl-tools linux-libc-dev
RUN ln -sf /usr/include/linux /usr/include/x86_64-linux-musl/linux && \
    ln -sf /usr/include/asm-generic /usr/include/x86_64-linux-musl/asm-generic && \
    ln -sf /usr/include/x86_64-linux-gnu/asm /usr/include/x86_64-linux-musl/asm
COPY testing/mini_threads.c /mini_threads.c
RUN musl-gcc -static -O2 -pthread -o /mini-threads /mini_threads.c

#
#  M5 Phase 7 integration test (mini-storage)
#
FROM ubuntu:20.04 AS mini_storage
RUN apt-get update && apt-get install -qy musl-tools linux-libc-dev
RUN ln -sf /usr/include/linux /usr/include/x86_64-linux-musl/linux && \
    ln -sf /usr/include/asm-generic /usr/include/x86_64-linux-musl/asm-generic && \
    ln -sf /usr/include/x86_64-linux-gnu/asm /usr/include/x86_64-linux-musl/asm
COPY testing/mini_storage.c /mini_storage.c
RUN musl-gcc -static -O2 -o /mini-storage /mini_storage.c

#
#  M6.5 Contract Tests
#
FROM ubuntu:20.04 AS contracts
RUN apt-get update && apt-get install -qy musl-tools linux-libc-dev
RUN ln -sf /usr/include/linux /usr/include/x86_64-linux-musl/linux && \
    ln -sf /usr/include/asm-generic /usr/include/x86_64-linux-musl/asm-generic && \
    ln -sf /usr/include/x86_64-linux-gnu/asm /usr/include/x86_64-linux-musl/asm
RUN mkdir /contracts
COPY testing/contracts/vm/fork_cow.c /contracts/
COPY testing/contracts/vm/mprotect_basic.c /contracts/
COPY testing/contracts/vm/mmap_anon.c /contracts/
COPY testing/contracts/vm/mmap_hint_flags.c /contracts/
COPY testing/contracts/vm/brk_basic.c /contracts/
COPY testing/contracts/vm/demand_paging.c /contracts/
COPY testing/contracts/vm/mmap_file.c /contracts/
COPY testing/contracts/vm/tlb_shootdown.c /contracts/
COPY testing/contracts/signals/mask_semantics.c /contracts/
COPY testing/contracts/signals/sa_restart.c /contracts/
COPY testing/contracts/signals/delivery_order.c /contracts/
COPY testing/contracts/signals/handler_context.c /contracts/
COPY testing/contracts/subsystems/dev_null_zero.c /contracts/
COPY testing/contracts/subsystems/proc_self.c /contracts/
COPY testing/contracts/subsystems/proc_mount.c /contracts/
COPY testing/contracts/subsystems/proc_global.c /contracts/
COPY testing/contracts/subsystems/proc_pid.c /contracts/
COPY testing/contracts/subsystems/proc_maps.c /contracts/
COPY testing/contracts/programs/busybox_basic.c /contracts/
COPY testing/contracts/scheduling/getpriority.c /contracts/
COPY testing/contracts/scheduling/nice_values.c /contracts/
COPY testing/contracts/scheduling/sched_yield.c /contracts/
COPY testing/contracts/scheduling/sched_fairness.c /contracts/
RUN for src in /contracts/*.c; do \
        name=$(basename ${src%.c}); \
        musl-gcc -static -O1 -Wall -Wno-unused-result -o /contracts/contract-${name} $src; \
    done

#
#  Initramfs
#
FROM busybox AS empty_dir
RUN mkdir /empty_dir

FROM scratch

# Directories to be populated by "busybox --install -s".
COPY --from=empty_dir /empty_dir /bin
COPY --from=empty_dir /empty_dir /sbin
COPY --from=empty_dir /empty_dir /usr/bin
COPY --from=empty_dir /empty_dir /usr/sbin

COPY --from=busybox  /build/busybox_unstripped /bin/busybox
COPY --from=dropbear /build/dropbear /build/dropbearkey /bin/
COPY --from=curl     /build/src/curl /bin
COPY --from=dyntest  /hello-dynamic /bin/hello-dynamic
COPY --from=dyntest  /ld-musl-x86_64.so.1 /lib/ld-musl-x86_64.so.1
COPY --from=bash       /build/bash /bin/bash
COPY --from=benchsuite /bench /bin/bench
COPY --from=testsuite  /test /bin/test
COPY --from=mini_systemd /mini-systemd /bin/mini-systemd
COPY --from=mini_storage /mini-storage /bin/mini-storage
COPY --from=mini_threads /mini-threads /bin/mini-threads
COPY --from=contracts /contracts/contract-* /bin/

RUN ["/bin/busybox", "--install", "-s"]
RUN mkdir -p /etc /dev /tmp /mnt /var/www/html
COPY testing/etc/banner /etc/banner
COPY testing/etc/group /etc/group
COPY testing/etc/passwd /etc/passwd
COPY testing/etc/profile /etc/profile
ADD testing/integration_tests /integration_tests
ADD testing/var/www/html/index.html /var/www/html/index.html

CMD ["/bin/sh"]
