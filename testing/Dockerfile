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
#  glibc hello world (static)
#
FROM ubuntu:20.04 AS hello_glibc
RUN apt-get update && apt-get install -qy build-essential
COPY testing/hello_glibc.c /hello_glibc.c
RUN gcc -static -O2 -o /hello-glibc /hello_glibc.c
# Dynamic glibc hello for testing the glibc dynamic linker.
COPY testing/hello_dynamic_glibc.c /hello_dynamic_glibc.c
RUN gcc -O2 -o /hello-dynamic-glibc /hello_dynamic_glibc.c
# Multi-library dynamic test (libc + libm + libpthread).
COPY testing/hello_multilib.c /hello_multilib.c
RUN gcc -O2 -o /hello-multilib /hello_multilib.c -lm -lpthread
# Test loading libsystemd-shared specifically (the crash culprit).
COPY testing/hello_libsystemd.c /hello_libsystemd.c
RUN gcc -O2 -o /hello-libsystemd /hello_libsystemd.c -ldl
# TLS test: shared library with __thread variable.
COPY testing/libtlstest.c /libtlstest.c
COPY testing/hello_tls.c /hello_tls.c
COPY testing/hello_tls_many.c /hello_tls_many.c
RUN gcc -shared -fPIC -o /libtlstest.so /libtlstest.c
RUN gcc -O2 -o /hello-tls /hello_tls.c -L/ -ltlstest -Wl,-rpath,/lib
RUN gcc -O2 -o /hello-tls-many /hello_tls_many.c -L/ -ltlstest -lm -lpthread -ldl -Wl,-rpath,/lib
# Many-libs test (libc + libm + libpthread + librt + libuuid).
COPY testing/hello_manylibs.c /hello_manylibs.c
RUN gcc -O2 -o /hello-manylibs /hello_manylibs.c -lm -lpthread -ldl -lrt

#
#  glibc pthreads test (static, reuses mini_threads.c)
#
FROM ubuntu:20.04 AS mini_threads_glibc
RUN apt-get update && apt-get install -qy build-essential linux-libc-dev
COPY testing/mini_threads.c /mini_threads.c
RUN gcc -static -O2 -pthread -o /mini-threads-glibc /mini_threads.c

#
#  M9 Phase 3.1: systemd v245 binaries (from Ubuntu 20.04 package)
#
FROM ubuntu:20.04 AS systemd_build
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -qy \
    build-essential meson ninja-build pkg-config python3 python3-jinja2 \
    libcap-dev libmount-dev gperf gettext m4 \
    curl xsltproc docbook-xsl docbook-xml
RUN mkdir /build && curl -fsSL \
    https://github.com/systemd/systemd-stable/archive/refs/tags/v245.7.tar.gz \
    | tar xz --strip-components=1 -C /build
WORKDIR /build
# Minimal meson config: disable everything non-essential.
RUN meson setup builddir \
    -Dstatic-libsystemd=true \
    -Dlink-systemctl-shared=false \
    -Dpam=false -Dselinux=false -Dapparmor=false -Daudit=false \
    -Dseccomp=false -Dutmp=false -Dgcrypt=false -Dp11kit=false \
    -Dgnutls=false -Dopenssl=false -Dcurl=false \
    -Dtpm=false -Dzlib=false -Dbzip2=false -Dzstd=false -Dlz4=false \
    -Dxz=false -Dpolkit=false -Dblkid=false -Dkmod=false -Didn=false \
    -Dresolve=false -Dnetworkd=false -Dtimesyncd=false -Dlogind=false \
    -Dmachined=false -Dimportd=false -Dhomed=false -Dhostnamed=false \
    -Dlocaled=false -Dtimedated=false -Dcoredump=false -Dfirstboot=false \
    -Drandomseed=false -Dhwdb=false -Drfkill=false -Dhibernate=false \
    -Dportabled=false -Duserdb=false \
    -Defi=false -Dgnu-efi=false \
    -Dman=false -Dhtml=false \
    -Dtests=false -Dinstall-tests=false \
    -Ddefault-hierarchy=unified
# Build just the core binaries.
RUN ninja -C builddir systemd
# Try to get a static binary; if not fully static, copy with its deps.
RUN mkdir -p /systemd-root/usr/lib/systemd /systemd-root/lib/x86_64-linux-gnu /systemd-root/lib64 /systemd-root/lib/systemd \
    && cp builddir/systemd /systemd-root/usr/lib/systemd/systemd \
    && cp builddir/src/shared/libsystemd-shared-245.so /systemd-root/lib/systemd/ \
    && (ldd /systemd-root/usr/lib/systemd/systemd /systemd-root/lib/systemd/libsystemd-shared-245*.so 2>/dev/null \
       | grep -o '/lib[^ ]*' | sort -u | while read lib; do \
         mkdir -p "/systemd-root$(dirname $lib)"; \
         cp -nL "$lib" "/systemd-root$lib" 2>/dev/null || true; \
       done || true) \
    && cp -L /lib64/ld-linux-x86-64.so.2 /systemd-root/lib64/ld-linux-x86-64.so.2 2>/dev/null || true \
    && (cp builddir/systemd-journald /systemd-root/usr/lib/systemd/systemd-journald 2>/dev/null || true) \
    && (cp builddir/systemctl /systemd-root/usr/lib/systemd/systemctl 2>/dev/null || true)

#
#  M9 Phase 2 integration test (systemd init-sequence validation)
#
FROM ubuntu:20.04 AS mini_systemd_v3
RUN apt-get update && apt-get install -qy musl-tools linux-libc-dev
RUN ln -sf /usr/include/linux /usr/include/x86_64-linux-musl/linux && \
    ln -sf /usr/include/asm-generic /usr/include/x86_64-linux-musl/asm-generic && \
    ln -sf /usr/include/x86_64-linux-gnu/asm /usr/include/x86_64-linux-musl/asm
COPY testing/mini_systemd_v3.c /mini_systemd_v3.c
RUN musl-gcc -static -O2 -o /mini-systemd-v3 /mini_systemd_v3.c

#
#  M8 Phase 4 integration test (cgroups + namespaces)
#
FROM ubuntu:20.04 AS mini_cgroups_ns
RUN apt-get update && apt-get install -qy musl-tools linux-libc-dev
RUN ln -sf /usr/include/linux /usr/include/x86_64-linux-musl/linux && \
    ln -sf /usr/include/asm-generic /usr/include/x86_64-linux-musl/asm-generic && \
    ln -sf /usr/include/x86_64-linux-gnu/asm /usr/include/x86_64-linux-musl/asm
COPY testing/mini_cgroups_ns.c /mini_cgroups_ns.c
RUN musl-gcc -static -O2 -o /mini-cgroups-ns /mini_cgroups_ns.c

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
COPY testing/contracts/subsystems/proc_fd.c /contracts/
COPY testing/contracts/subsystems/cgroup_basic.c /contracts/
COPY testing/contracts/subsystems/ns_uts.c /contracts/
COPY testing/contracts/subsystems/mountinfo.c /contracts/
COPY testing/contracts/subsystems/waitid_basic.c /contracts/
COPY testing/contracts/subsystems/syscall_gaps.c /contracts/
COPY testing/contracts/programs/busybox_basic.c /contracts/
COPY testing/contracts/programs/glibc_stubs.c /contracts/
COPY testing/contracts/scheduling/getpriority.c /contracts/
COPY testing/contracts/scheduling/nice_values.c /contracts/
COPY testing/contracts/scheduling/sched_yield.c /contracts/
COPY testing/contracts/scheduling/sched_fairness.c /contracts/
COPY testing/contracts/scheduling/futex_requeue.c /contracts/
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
COPY --from=mini_cgroups_ns /mini-cgroups-ns /bin/mini-cgroups-ns
COPY --from=mini_systemd_v3 /mini-systemd-v3 /bin/mini-systemd-v3
# systemd v245 binaries + glibc runtime
COPY --from=systemd_build /systemd-root/usr/lib/systemd/ /usr/lib/systemd/
COPY --from=systemd_build /systemd-root/lib/ /lib/
COPY --from=systemd_build /systemd-root/lib64/ /lib64/
# Also copy libsystemd-shared into the standard ld.so search path.
COPY --from=systemd_build /systemd-root/lib/systemd/libsystemd-shared-245.so /lib/x86_64-linux-gnu/libsystemd-shared-245.so
COPY --from=hello_glibc /hello-glibc /bin/hello-glibc
COPY --from=hello_glibc /hello-dynamic-glibc /bin/hello-dynamic-glibc
COPY --from=hello_glibc /hello-multilib /bin/hello-multilib
COPY --from=hello_glibc /hello-libsystemd /bin/hello-libsystemd
COPY --from=hello_glibc /libtlstest.so /lib/libtlstest.so
COPY --from=hello_glibc /hello-tls /bin/hello-tls
COPY --from=hello_glibc /hello-tls-many /bin/hello-tls-many
COPY --from=hello_glibc /hello-manylibs /bin/hello-manylibs
COPY --from=mini_threads_glibc /mini-threads-glibc /bin/mini-threads-glibc
COPY --from=contracts /contracts/contract-* /bin/

RUN ["/bin/busybox", "--install", "-s"]
RUN mkdir -p /etc /dev /tmp /mnt /var/www/html /run /var/log/journal
# systemd configuration
RUN mkdir -p /etc/systemd/system/multi-user.target.wants \
    && mkdir -p /usr/lib/systemd/system
COPY testing/etc/banner /etc/banner
COPY testing/etc/group /etc/group
COPY testing/etc/passwd /etc/passwd
COPY testing/etc/profile /etc/profile
COPY testing/systemd/units/default.target /etc/systemd/system/default.target
COPY testing/systemd/units/kevlar-getty.service /etc/systemd/system/kevlar-getty.service
COPY testing/systemd/os-release /etc/os-release
COPY testing/systemd/fstab /etc/fstab
RUN head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n' > /etc/machine-id
RUN ln -sf /etc/systemd/system/kevlar-getty.service /etc/systemd/system/multi-user.target.wants/kevlar-getty.service
ADD testing/integration_tests /integration_tests
ADD testing/var/www/html/index.html /var/www/html/index.html

CMD ["/bin/sh"]
