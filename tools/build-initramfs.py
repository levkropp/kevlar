#!/usr/bin/env python3
"""Native Linux initramfs builder for Kevlar (no Docker required).

Compiles all test binaries directly with musl-gcc/gcc and downloads
external packages (BusyBox, curl, dropbear, bash) from source with
caching.  Subsequent builds only recompile changed .c files.

Usage:
    python3 tools/build-initramfs.py build/testing.initramfs
    python3 tools/build-initramfs.py --clean build/testing.initramfs
    python3 tools/build-initramfs.py --arch arm64 build/testing.arm64.initramfs

    When ARCH=arm64 is set in the environment (by `make ARCH=arm64`), arm64
    mode is selected automatically.

Prerequisites (Arch):   pacman -S musl gcc e2fsprogs
Prerequisites (Ubuntu): apt install musl-tools build-essential linux-libc-dev e2fsprogs
ARM64 mode: no cross-compiler needed; downloads pre-built Alpine aarch64 binaries.
"""
import argparse
import io
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CACHE = ROOT / "build" / "native-cache"
LOCAL_BIN = CACHE / "local-bin"   # Compiled test binaries (x86_64)
LOCAL_LIB = CACHE / "local-lib"   # Compiled shared libs (x86_64)
EXT_BIN = CACHE / "ext-bin"       # External package binaries (x86_64)
EXT_BIN_ARM64 = CACHE / "ext-bin-arm64"  # External package binaries (aarch64)
ROOTFS = ROOT / "build" / "initramfs-rootfs"

ALPINE_MIRROR_ARM64 = "https://dl-cdn.alpinelinux.org/alpine/v3.21/main/aarch64"

# ─── Utilities ────────────────────────────────────────────────────────────

def log(tag, msg):
    print(f"  \033[1;96m{tag:>8s}\033[0m  {msg}", file=sys.stderr)

def run(cmd, **kw):
    kw.setdefault("check", True)
    return subprocess.run(cmd, **kw)

def download(url, dest):
    dest = Path(dest)
    if dest.exists():
        return dest
    dest.parent.mkdir(parents=True, exist_ok=True)
    log("DL", Path(url).name)
    req = urllib.request.Request(url, headers={"User-Agent": "kevlar-build/1.0"})
    with urllib.request.urlopen(req) as resp, open(dest, "wb") as f:
        shutil.copyfileobj(resp, f)
    return dest

def needs_rebuild(src, dst):
    """True if dst doesn't exist or any src is newer."""
    dst = Path(dst)
    if not dst.exists():
        return True
    dst_mtime = dst.stat().st_mtime
    if isinstance(src, (str, Path)):
        src = [src]
    return any(Path(s).stat().st_mtime > dst_mtime for s in src)


# ─── Compilation ──────────────────────────────────────────────────────────

def compile_one(cc, src, out, flags):
    """Compile a single C file.  Returns (out, ok, error_msg)."""
    src = Path(src)
    out = Path(out)
    if not needs_rebuild(src, out):
        return str(out), True, None
    out.parent.mkdir(parents=True, exist_ok=True)
    cmd = [cc] + flags + ["-o", str(out), str(src)]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        return str(out), False, r.stderr
    return str(out), True, None


def compile_all_local():
    """Compile all local test binaries in parallel."""
    log("CC", "local test binaries")
    LOCAL_BIN.mkdir(parents=True, exist_ok=True)
    LOCAL_LIB.mkdir(parents=True, exist_ok=True)

    jobs = []

    # ── musl-gcc static binaries ──
    musl_bins = [
        ("benchmarks/bench.c",           "bench",            []),
        ("benchmarks/fork_micro.c",      "fork_micro",       []),
        ("tests/test.c",                 "test",             []),
        ("testing/mini_systemd.c",       "mini-systemd",     []),
        ("testing/mini_threads.c",       "mini-threads",     ["-pthread"]),
        ("testing/mini_storage.c",       "mini-storage",     []),
        ("testing/mini_systemd_v3.c",    "mini-systemd-v3",  []),
        ("testing/mini_cgroups_ns.c",    "mini-cgroups-ns",  []),
        ("testing/test_ext2_rw.c",       "test-ext2-rw",     []),
        ("testing/test_ext4_mknod.c",    "test-ext4-mknod",  []),
        ("testing/boot_alpine.c",        "boot-alpine",      []),
        ("testing/test_alpine_apk.c",    "test-alpine-apk",  []),
        ("testing/test_cgroups_hang.c",  "test-cgroups-hang", []),
        ("testing/test_openrc_boot.c",  "test-openrc-boot",  []),
        ("testing/test_sigchld_pipe.c", "test-sigchld-pipe", []),
        ("testing/test_gcc_build.c",   "test-gcc-build",   []),
        ("testing/test_pipe_crash.c",    "test-pipe-crash",  []),
        ("testing/bb_minimal.c",         "bb-minimal",       []),
        ("testing/test_gcc_alpine.c",     "test-gcc-alpine", []),
        ("testing/test_static_pipe.c",    "test-static-pipe", []),
        ("testing/test_dynamic_exec.c",   "test-dyn-exec",    []),
        ("testing/test_dynamic_pipe.c",   "test-dyn-pipe",    []),
        ("testing/test_vfork_pipe.c",     "test-vfork-pipe",  []),
        ("testing/test_alpine_shell.c",   "test-ash-pipe",    []),
        ("testing/test_ash_pipe2.c",      "test-ash-pipe2",   []),
        ("testing/test_login_flow.c",     "test-login-flow",  []),
        ("testing/test_libz.c",           "test-libz",        []),
        ("testing/test_apk_db.c",          "test-apk-db",      []),
        ("testing/test_ext4_dir.c",         "test-ext4-dir",    []),
        ("testing/test_apk_update.c",      "test-apk-update",  []),
        ("testing/test_apk_write.c",       "test-apk-write",   []),
        ("testing/test_apk_trace.c",       "test-apk-trace",   []),
        ("testing/test_apk_interactive.c", "test-apk-inter",   []),
        ("testing/busybox_suite.c",      "busybox-suite",    []),
        ("testing/dd_diag.c",            "dd-diag",          []),
        ("testing/test_net.c",           "test-net",         []),
        ("testing/test_ssh_dropbear.c",  "test-ssh-dropbear", []),
        ("testing/test_nginx.c",         "test-nginx",        []),
        ("testing/test_build_tools.c",   "test-build-tools",  []),
        ("testing/test_alpine.c",        "test-alpine",      []),
        ("testing/fork_exec_stress.c",   "fork-exec-stress", []),
        ("testing/disk_hello.c",         "disk_hello",       []),
        ("testing/test_clock.c",         "test-clock",       []),
    ]
    for src_rel, name, extra in musl_bins:
        src = ROOT / src_rel
        out = LOCAL_BIN / name
        jobs.append(("musl-gcc", src, out,
                      ["-static", "-O2", "-Wall", "-Wno-unused-result"] + extra))

    # ── Contract tests (musl-gcc static) ──
    for src in sorted(ROOT.glob("testing/contracts/*/*.c")):
        name = "contract-" + src.stem
        out = LOCAL_BIN / name
        jobs.append(("musl-gcc", src, out,
                      ["-static", "-O1", "-Wall", "-Wno-unused-result"]))

    # ── gcc static (glibc) ──
    gcc_static = [
        ("testing/hello_glibc.c",   "hello-glibc",          []),
        ("testing/mini_threads.c",  "mini-threads-glibc",   ["-pthread"]),
    ]
    for src_rel, name, extra in gcc_static:
        src = ROOT / src_rel
        out = LOCAL_BIN / name
        jobs.append(("gcc", src, out,
                      ["-static", "-O2", "-Wall", "-Wno-unused-result"] + extra))

    # ── gcc dynamic (glibc) ──
    gcc_dyn = [
        ("testing/hello_dynamic_glibc.c", "hello-dynamic-glibc", []),
        ("testing/hello_multilib.c",      "hello-multilib",      ["-lm", "-lpthread"]),
        ("testing/hello_libsystemd.c",    "hello-libsystemd",    ["-ldl"]),
        ("testing/hello_manylibs.c",      "hello-manylibs",
         ["-lm", "-lpthread", "-ldl", "-lrt"]),
    ]
    for src_rel, name, extra in gcc_dyn:
        src = ROOT / src_rel
        out = LOCAL_BIN / name
        jobs.append(("gcc", src, out,
                      ["-O2", "-Wall", "-Wno-unused-result"] + extra))

    # ── Shared library: libtlstest.so ──
    jobs.append(("gcc", ROOT / "testing" / "libtlstest.c",
                 LOCAL_LIB / "libtlstest.so",
                 ["-shared", "-fPIC"]))

    # Run compilation jobs in parallel
    failed = []
    with ThreadPoolExecutor(max_workers=os.cpu_count() or 4) as pool:
        futures = {}
        for cc, src, out, flags in jobs:
            f = pool.submit(compile_one, cc, src, out, flags)
            futures[f] = (cc, src, out)
        for f in as_completed(futures):
            out_path, ok, err = f.result()
            if not ok:
                cc, src, out = futures[f]
                log("FAIL", f"{src.name}: {err.strip()}")
                failed.append(str(src))

    # ── TLS test binaries (depend on libtlstest.so) ──
    tls_lib = LOCAL_LIB / "libtlstest.so"
    if tls_lib.exists():
        tls_bins = [
            ("testing/hello_tls.c", "hello-tls",
             ["-L", str(LOCAL_LIB), "-ltlstest", "-Wl,-rpath,/lib"]),
            ("testing/hello_tls_many.c", "hello-tls-many",
             ["-L", str(LOCAL_LIB), "-ltlstest", "-lm", "-lpthread", "-ldl",
              "-Wl,-rpath,/lib"]),
        ]
        for src_rel, name, extra in tls_bins:
            src = ROOT / src_rel
            out = LOCAL_BIN / name
            _, ok, err = compile_one("gcc", src, out,
                                     ["-O2", "-Wall", "-Wno-unused-result"] + extra)
            if not ok:
                log("FAIL", f"{src.name}: {err.strip()}")
                failed.append(str(src))

    # ── musl dynamic hello ──
    hello_dyn_src = CACHE / "hello_dynamic.c"
    hello_dyn_src.parent.mkdir(parents=True, exist_ok=True)
    hello_dyn_src.write_text(
        '#include <unistd.h>\n'
        'int main(){const char m[]="hello from dynamic linking!\\n";'
        'write(1,m,sizeof(m)-1);return 0;}\n')
    out = LOCAL_BIN / "hello-dynamic"
    _, ok, err = compile_one("musl-gcc", hello_dyn_src, out, ["-O2"])
    if not ok:
        log("FAIL", f"hello-dynamic: {err}")

    # ── Static musl diagnostic tools ──
    for src_name, bin_name in [
        ("dyntest.c", "dyntest"),
        ("test_ext4_comprehensive.c", "test-ext4"),
    ]:
        src = ROOT / "testing" / src_name
        if src.exists():
            out = LOCAL_BIN / bin_name
            _, ok, err = compile_one("musl-gcc", src, out, ["-static", "-O2"])
            if not ok:
                log("FAIL", f"{bin_name}: {err}")

    # ── Dynamically-linked dlopen test (for dlopen crash investigation) ──
    dlopen_src = ROOT / "testing" / "test_dlopen.c"
    if dlopen_src.exists():
        dlopen_out = LOCAL_BIN / "test_dlopen"
        _, ok, err = compile_one("musl-gcc", dlopen_src, dlopen_out, ["-O2", "-ldl"])
        if not ok:
            log("WARN", f"test_dlopen: {err} (non-fatal)")

    if failed:
        log("WARN", f"{len(failed)} compilation(s) failed")
    return len(failed) == 0


# ─── External Package Builders ────────────────────────────────────────────

def build_busybox():
    """Build BusyBox 1.37.0 with musl static linking."""
    out = EXT_BIN / "busybox"
    if out.exists():
        return out

    tarball = download(
        "https://busybox.net/downloads/busybox-1.37.0.tar.bz2",
        CACHE / "src" / "busybox-1.37.0.tar.bz2")

    bdir = CACHE / "build-busybox"
    if bdir.exists():
        shutil.rmtree(bdir)
    bdir.mkdir(parents=True)

    log("BUILD", "busybox 1.37.0")
    run(["tar", "xf", str(tarball), "--strip-components=1", "-C", str(bdir)],
        capture_output=True)

    # musl-ar and musl-strip wrappers
    wrappers = bdir / "_wrappers"
    wrappers.mkdir()
    for alias, real_name in [("musl-ar", "ar"), ("musl-strip", "strip")]:
        real = shutil.which(real_name)
        if real:
            os.symlink(real, wrappers / alias)

    env = os.environ.copy()
    env["PATH"] = str(wrappers) + ":" + env.get("PATH", "")

    run(["make", "defconfig"], cwd=bdir, env=env, capture_output=True)

    config = (bdir / ".config").read_text()
    for old, new in {
        '# CONFIG_STATIC is not set': 'CONFIG_STATIC=y',
        'CONFIG_CROSS_COMPILER_PREFIX=""': 'CONFIG_CROSS_COMPILER_PREFIX="musl-"',
        'CONFIG_NANDWRITE=y': '# CONFIG_NANDWRITE is not set',
        'CONFIG_NANDDUMP=y': '# CONFIG_NANDDUMP is not set',
        'CONFIG_UBIATTACH=y': '# CONFIG_UBIATTACH is not set',
        'CONFIG_UBIDETACH=y': '# CONFIG_UBIDETACH is not set',
        'CONFIG_UBIMKVOL=y': '# CONFIG_UBIMKVOL is not set',
        'CONFIG_UBIRMVOL=y': '# CONFIG_UBIRMVOL is not set',
        'CONFIG_UBIRSVOL=y': '# CONFIG_UBIRSVOL is not set',
        'CONFIG_UBIUPDATEVOL=y': '# CONFIG_UBIUPDATEVOL is not set',
        'CONFIG_UBIRENAME=y': '# CONFIG_UBIRENAME is not set',
        'CONFIG_TC=y': '# CONFIG_TC is not set',
        # Enable standalone shell: ash dispatches BusyBox applets internally
        # (NOFORK = run in same process, NOEXEC = fork but skip exec).
        # Requires working /proc/self/exe (readlink → /bin/busybox).
        '# CONFIG_FEATURE_PREFER_APPLETS is not set': 'CONFIG_FEATURE_PREFER_APPLETS=y',
        '# CONFIG_FEATURE_SH_STANDALONE is not set': 'CONFIG_FEATURE_SH_STANDALONE=y',
        '# CONFIG_FEATURE_SH_NOFORK is not set': 'CONFIG_FEATURE_SH_NOFORK=y',
    }.items():
        config = config.replace(old, new)
    (bdir / ".config").write_text(config)

    nproc = os.cpu_count() or 1
    run(["make", f"-j{nproc}"], cwd=bdir, env=env,
        stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)

    out.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(bdir / "busybox_unstripped", out)
    os.chmod(out, 0o755)
    shutil.rmtree(bdir)
    return out


def build_curl():
    """Build curl 8.10.1 with musl static linking."""
    out = EXT_BIN / "curl"
    if out.exists():
        return out

    tarball = download(
        "https://curl.se/download/curl-8.10.1.tar.xz",
        CACHE / "src" / "curl-8.10.1.tar.xz")

    bdir = CACHE / "build-curl"
    if bdir.exists():
        shutil.rmtree(bdir)
    bdir.mkdir(parents=True)

    log("BUILD", "curl 8.10.1")
    run(["tar", "xf", str(tarball), "--strip-components=1", "-C", str(bdir)],
        capture_output=True)

    run(["./configure", "CC=musl-gcc",
         "--without-ssl", "--without-libpsl",
         "--disable-shared", "--disable-pthreads",
         "--disable-threaded-resolver", "--disable-rtsp",
         "--disable-alt-svc", "--disable-libcurl-option",
         "--disable-telnet", "--disable-gopher",
         "--disable-dict", "--disable-file",
         "--disable-ftp", "--disable-tftp",
         "--disable-imap", "--disable-pop3",
         "--disable-smtp", "--disable-mqtt",
         "--disable-unix-sockets", "--disable-ldap"],
        cwd=bdir, capture_output=True)

    nproc = os.cpu_count() or 1
    run(["make", f"-j{nproc}", "curl_LDFLAGS=-all-static"], cwd=bdir,
        stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)

    out.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(bdir / "src" / "curl", out)
    os.chmod(out, 0o755)
    shutil.rmtree(bdir)
    return out


def build_dropbear():
    """Build dropbear 2024.85 SSH server."""
    out_db = EXT_BIN / "dropbear"
    out_key = EXT_BIN / "dropbearkey"
    out_dbc = EXT_BIN / "dbclient"
    if out_db.exists() and out_key.exists() and out_dbc.exists():
        return out_db, out_key

    tarball = download(
        "https://matt.ucc.asn.au/dropbear/releases/dropbear-2024.85.tar.bz2",
        CACHE / "src" / "dropbear-2024.85.tar.bz2")

    bdir = CACHE / "build-dropbear"
    if bdir.exists():
        shutil.rmtree(bdir)
    bdir.mkdir(parents=True)

    log("BUILD", "dropbear 2024.85")
    run(["tar", "xf", str(tarball), "--strip-components=1", "-C", str(bdir)],
        capture_output=True)

    # Apply patches
    patch = ROOT / "testing" / "dropbear" / "accept-empty-password-root-login.patch"
    localopts = ROOT / "testing" / "dropbear" / "localoptions.h"
    if patch.exists():
        run(["patch", "--ignore-whitespace", "-p1", "-i", str(patch)],
            cwd=bdir, capture_output=True)
    if localopts.exists():
        shutil.copy2(localopts, bdir / "localoptions.h")

    run(["./configure", "CC=musl-gcc", "--enable-static",
         "--disable-largefile", "--disable-zlib",
         "--disable-syslog", "--disable-wtmp",
         "--disable-wtmpx", "--disable-utmp",
         "--disable-utmpx", "--disable-loginfunc"],
        cwd=bdir, capture_output=True)

    nproc = os.cpu_count() or 1
    run(["make", f"-j{nproc}"], cwd=bdir,
        stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)

    EXT_BIN.mkdir(parents=True, exist_ok=True)
    shutil.copy2(bdir / "dropbear", out_db)
    shutil.copy2(bdir / "dropbearkey", out_key)
    if (bdir / "dbclient").exists():
        shutil.copy2(bdir / "dbclient", out_dbc)
        os.chmod(out_dbc, 0o755)
    os.chmod(out_db, 0o755)
    os.chmod(out_key, 0o755)
    shutil.rmtree(bdir)
    return out_db, out_key


def build_bash():
    """Build bash 5.2.37 with musl static linking."""
    out = EXT_BIN / "bash"
    if out.exists():
        return out

    tarball = download(
        "https://ftp.gnu.org/gnu/bash/bash-5.2.37.tar.gz",
        CACHE / "src" / "bash-5.2.37.tar.gz")

    bdir = CACHE / "build-bash"
    if bdir.exists():
        shutil.rmtree(bdir)
    bdir.mkdir(parents=True)

    log("BUILD", "bash 5.2.37")
    run(["tar", "xf", str(tarball), "--strip-components=1", "-C", str(bdir)],
        capture_output=True)

    # GCC 15+ defaults to C23 which breaks bash's K&R function definitions.
    run(["./configure", "CC=musl-gcc", "CFLAGS=-O2 -std=gnu17",
         "--without-bash-malloc", "--enable-static-link",
         "--disable-nls", "--without-curses", "--disable-readline"],
        cwd=bdir, capture_output=True)

    nproc = os.cpu_count() or 1
    run(["make", f"-j{nproc}",
         "LDFLAGS=-static -Wl,--allow-multiple-definition"],
        cwd=bdir, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)

    out.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(bdir / "bash", out)
    os.chmod(out, 0o755)
    shutil.rmtree(bdir)
    return out


def harvest_host_systemd():
    """Copy the host's systemd binary + all shared library deps into ext-bin.

    Works on any system with /usr/lib/systemd/systemd installed.
    Returns True if successful.
    """
    import re

    host_bin = Path("/usr/lib/systemd/systemd")
    if not host_bin.exists():
        return False

    EXT_BIN.mkdir(parents=True, exist_ok=True)
    systemd_libs_dir = EXT_BIN / "systemd-libs"
    systemd_libs_dir.mkdir(parents=True, exist_ok=True)

    # Collect all shared library dependencies recursively via ldd.
    seen = set()
    queue = [str(host_bin)]
    lib_paths = []
    while queue:
        binary = queue.pop(0)
        if binary in seen:
            continue
        seen.add(binary)
        try:
            r = run(["ldd", binary], capture_output=True, text=True, check=False)
        except Exception:
            continue
        for line in r.stdout.splitlines():
            m = re.search(r'=> (/\S+)', line)
            if m:
                path = m.group(1)
                if path not in seen:
                    queue.append(path)
            m = re.search(r'^\s*(/\S+)', line)
            if m and '=>' not in line and 'vdso' not in line:
                path = m.group(1)
                if path not in seen:
                    queue.append(path)

    # Copy systemd binary.
    shutil.copy2(host_bin, EXT_BIN / "systemd")
    os.chmod(EXT_BIN / "systemd", 0o755)

    # Copy all library dependencies.
    for path in sorted(seen):
        if path == str(host_bin):
            continue
        p = Path(path)
        if p.exists() and p.is_file():
            dest = systemd_libs_dir / p.name
            if not dest.exists():
                shutil.copy2(p, dest)
            # Keep symlink names too (e.g. libcrypt.so.2 → libcrypt.so.2.0.0)
            lib_paths.append((p, dest))

    # Copy additional systemd helpers if present on host.
    for extra in ["systemd-journald", "systemctl"]:
        src = Path("/usr/lib/systemd") / extra
        if src.exists():
            shutil.copy2(src, EXT_BIN / extra)
            os.chmod(EXT_BIN / extra, 0o755)

    host_ver_r = run([str(host_bin), "--version"], capture_output=True, text=True, check=False)
    ver_line = host_ver_r.stdout.split('\n')[0].strip() if host_ver_r.stdout else "unknown"
    log("HARVEST", f"systemd from host ({ver_line})")
    return True


def build_systemd():
    """Get systemd binaries — try from-source build first, fall back to host harvest.

    Returns True if systemd binaries are available, False otherwise.
    """
    systemd_bin = EXT_BIN / "systemd"

    # Check if we already have a cached binary (from any method).
    if systemd_bin.exists():
        return True

    # Method 1: Build from source (requires meson < 1.0 for v245).
    meson = shutil.which("meson")
    if meson:
        try:
            r = run(["meson", "--version"], capture_output=True, text=True)
            ver = r.stdout.strip()
            major = int(ver.split(".")[0])
            if major < 1:
                return _build_systemd_from_source()
            else:
                log("SKIP", f"systemd from-source (meson {ver} too new for v245)")
        except (subprocess.CalledProcessError, ValueError):
            pass

    # Method 2: Harvest the host's systemd binary + shared libs.
    if harvest_host_systemd():
        return True

    log("SKIP", "systemd (no meson < 1.0, no host systemd)")
    return False


def _build_systemd_from_source():
    """Build systemd v245 from source (requires meson < 1.0)."""
    systemd_bin = EXT_BIN / "systemd"
    libsystemd = EXT_BIN / "libsystemd-shared-245.so"

    tarball = download(
        "https://github.com/systemd/systemd-stable/archive/refs/tags/v245.7.tar.gz",
        CACHE / "src" / "systemd-v245.7.tar.gz")

    bdir = CACHE / "build-systemd"
    if bdir.exists():
        shutil.rmtree(bdir)
    bdir.mkdir(parents=True)

    log("BUILD", "systemd v245.7")
    run(["tar", "xf", str(tarball), "--strip-components=1", "-C", str(bdir)],
        capture_output=True)

    try:
        run(["meson", "setup", "builddir",
             "-Dstatic-libsystemd=true",
             "-Dlink-systemctl-shared=false",
             "-Dpam=false", "-Dselinux=false", "-Dapparmor=false",
             "-Daudit=false", "-Dseccomp=false", "-Dutmp=false",
             "-Dgcrypt=false", "-Dp11kit=false", "-Dgnutls=false",
             "-Dopenssl=false", "-Dtpm=false",
             "-Dzlib=false", "-Dbzip2=false",
             "-Dlz4=false", "-Dxz=false", "-Dpolkit=false",
             "-Dblkid=false", "-Dkmod=false", "-Didn=false",
             "-Dresolve=false", "-Dnetworkd=false", "-Dtimesyncd=false",
             "-Dlogind=false", "-Dmachined=false", "-Dimportd=false",
             "-Dhomed=false", "-Dhostnamed=false", "-Dlocaled=false",
             "-Dtimedated=false", "-Dcoredump=false", "-Dfirstboot=false",
             "-Drandomseed=false", "-Dhwdb=false", "-Drfkill=false",
             "-Dhibernate=false", "-Dportabled=false", "-Duserdb=false",
             "-Defi=false", "-Dgnu-efi=false",
             "-Dman=false", "-Dhtml=false",
             "-Dtests=false", "-Dinstall-tests=false",
             "-Ddefault-hierarchy=unified"],
            cwd=bdir, capture_output=True)

        nproc = os.cpu_count() or 1
        run(["ninja", "-C", "builddir", "systemd"],
            cwd=bdir, capture_output=True)

        EXT_BIN.mkdir(parents=True, exist_ok=True)
        shutil.copy2(bdir / "builddir" / "systemd", systemd_bin)
        shutil.copy2(
            bdir / "builddir" / "src" / "shared" / "libsystemd-shared-245.so",
            libsystemd)
        os.chmod(systemd_bin, 0o755)
        os.chmod(libsystemd, 0o755)

        for extra in ["systemd-journald", "systemctl"]:
            p = bdir / "builddir" / extra
            if p.exists():
                shutil.copy2(p, EXT_BIN / extra)
                os.chmod(EXT_BIN / extra, 0o755)

        shutil.rmtree(bdir)
        return True
    except subprocess.CalledProcessError as e:
        log("WARN", f"systemd build failed: {e}")
        if bdir.exists():
            shutil.rmtree(bdir)
        return False


# ─── Alpine Package Downloads ─────────────────────────────────────────────

ALPINE_MIRROR = "https://dl-cdn.alpinelinux.org/alpine/v3.21/main/x86_64"

def parse_apkindex():
    """Download and parse APKINDEX to get package→filename mapping."""
    index_path = download(
        f"{ALPINE_MIRROR}/APKINDEX.tar.gz",
        CACHE / "src" / "APKINDEX.tar.gz")

    packages = {}
    with tarfile.open(index_path, "r:gz") as tar:
        for member in tar.getmembers():
            if member.name == "APKINDEX":
                f = tar.extractfile(member)
                content = f.read().decode("utf-8")
                current = {}
                for line in content.split("\n"):
                    if line == "":
                        if "P" in current and "V" in current:
                            packages[current["P"]] = current["V"]
                        current = {}
                    elif ":" in line:
                        key, _, val = line.partition(":")
                        current[key] = val
                break
    return packages


def download_alpine_pkg(pkg_name, version, dest_dir):
    """Download and extract an Alpine .apk package."""
    filename = f"{pkg_name}-{version}.apk"
    apk_path = download(f"{ALPINE_MIRROR}/{filename}",
                        CACHE / "src" / filename)

    dest_dir = Path(dest_dir)
    dest_dir.mkdir(parents=True, exist_ok=True)

    # APK files are gzipped tarballs; extract non-metadata files
    run(["tar", "xzf", str(apk_path), "-C", str(dest_dir),
         "--exclude=.PKGINFO", "--exclude=.SIGN.*", "--exclude=.pre-install",
         "--exclude=.post-install", "--exclude=.trigger"],
        capture_output=True)
    return dest_dir


def download_alpine_packages():
    """Download apk-tools-static and OpenRC from Alpine repos."""
    log("DL", "Alpine packages")
    try:
        pkgs = parse_apkindex()
    except Exception as e:
        log("WARN", f"Cannot fetch Alpine packages: {e}")
        return {}

    alpine_dir = CACHE / "alpine-pkgs"
    results = {}

    for pkg in ["apk-tools-static", "openrc", "busybox-openrc"]:
        if pkg not in pkgs:
            log("WARN", f"Alpine package {pkg} not found in index")
            continue
        dest = alpine_dir / pkg
        if dest.exists():
            results[pkg] = dest
            continue
        try:
            download_alpine_pkg(pkg, pkgs[pkg], dest)
            results[pkg] = dest
        except Exception as e:
            log("WARN", f"Failed to download {pkg}: {e}")

    return results


# ─── Find glibc runtime libraries ────────────────────────────────────────

def find_glibc_libs():
    """Find glibc shared libraries needed by dynamic test binaries."""
    lib_names = ["libc.so.6", "libm.so.6", "libpthread.so.0",
                 "libdl.so.2", "librt.so.1"]
    # Search paths: Arch (/usr/lib), Ubuntu (/lib/x86_64-linux-gnu)
    search = ["/usr/lib", "/lib/x86_64-linux-gnu", "/lib64"]
    found = {}
    for name in lib_names:
        for d in search:
            p = Path(d) / name
            if p.exists():
                found[name] = p
                break
    # Dynamic linker
    for p in ["/lib64/ld-linux-x86-64.so.2", "/usr/lib/ld-linux-x86-64.so.2"]:
        if Path(p).exists():
            found["ld-linux-x86-64.so.2"] = Path(p)
            break
    return found


def find_musl_libc():
    """Find musl libc.so for dynamic linking test."""
    for p in ["/usr/lib/musl/lib/libc.so",
              "/usr/lib/x86_64-linux-musl/libc.so",
              "/lib/ld-musl-x86_64.so.1"]:
        if Path(p).exists():
            return Path(p)
    return None


# ─── Rootfs Assembly ──────────────────────────────────────────────────────

def assemble_rootfs(have_systemd, alpine_pkgs):
    """Assemble the complete initramfs rootfs directory."""
    log("ROOTFS", "assembling")

    if ROOTFS.exists():
        shutil.rmtree(ROOTFS)

    # Create directory structure
    for d in ["bin", "sbin", "usr/bin", "usr/sbin",
              "etc", "etc/network", "dev", "proc", "sys", "tmp", "mnt",
              "var/www/html", "run", "var/log/journal",
              "lib", "lib64", "lib/x86_64-linux-gnu",
              "etc/systemd/system/multi-user.target.wants",
              "usr/lib/systemd/system", "usr/lib/systemd",
              "etc/runlevels/sysinit", "etc/runlevels/boot",
              "etc/runlevels/default", "etc/runlevels/shutdown",
              "etc/runlevels/nonetwork"]:
        (ROOTFS / d).mkdir(parents=True, exist_ok=True)

    # var/run → run
    os.symlink("/run", str(ROOTFS / "var" / "run"))

    # ── External binaries ──
    ext_copy = {
        "busybox":     "bin/busybox",
        "curl":        "bin/curl",
        "dropbear":    "bin/dropbear",
        "dropbearkey": "bin/dropbearkey",
        "dbclient":    "bin/dbclient",
        "bash":        "bin/bash",
    }
    for src_name, dest_rel in ext_copy.items():
        src = EXT_BIN / src_name
        if src.exists():
            dest = ROOTFS / dest_rel
            shutil.copy2(src, dest)
            os.chmod(dest, 0o755)

    # ── systemd binaries ──
    if have_systemd:
        systemd_bin = EXT_BIN / "systemd"
        if systemd_bin.exists():
            shutil.copy2(systemd_bin, ROOTFS / "usr/lib/systemd/systemd")
            os.chmod(ROOTFS / "usr/lib/systemd/systemd", 0o755)

        # v245 from-source build: single shared lib
        libsystemd_245 = EXT_BIN / "libsystemd-shared-245.so"
        if libsystemd_245.exists():
            (ROOTFS / "lib" / "systemd").mkdir(parents=True, exist_ok=True)
            shutil.copy2(libsystemd_245, ROOTFS / "lib/systemd/libsystemd-shared-245.so")
            shutil.copy2(libsystemd_245,
                         ROOTFS / "lib/x86_64-linux-gnu/libsystemd-shared-245.so")

        # Host-harvested systemd: all deps collected in systemd-libs/
        systemd_libs_dir = EXT_BIN / "systemd-libs"
        if systemd_libs_dir.exists():
            for lib_file in systemd_libs_dir.iterdir():
                if not lib_file.is_file():
                    continue
                name = lib_file.name
                if name == "ld-linux-x86-64.so.2":
                    dest = ROOTFS / "lib64" / name
                elif "libsystemd-" in name:
                    # systemd private libs go to /usr/lib/systemd/
                    dest = ROOTFS / "usr" / "lib" / "systemd" / name
                else:
                    dest = ROOTFS / "lib/x86_64-linux-gnu" / name
                dest.parent.mkdir(parents=True, exist_ok=True)
                if not dest.exists():
                    shutil.copy2(lib_file, dest)
                # Also symlink into /usr/lib/ for Arch-style distros where
                # the dynamic linker's default search path is /usr/lib.
                usr_lib_dest = ROOTFS / "usr" / "lib" / name
                if not usr_lib_dest.exists() and "libsystemd-" not in name:
                    os.symlink(f"/lib/x86_64-linux-gnu/{name}", str(usr_lib_dest))

        for extra in ["systemd-journald", "systemctl"]:
            src = EXT_BIN / extra
            if src.exists():
                shutil.copy2(src, ROOTFS / "usr/lib/systemd" / extra)
                os.chmod(ROOTFS / "usr/lib/systemd" / extra, 0o755)

        # systemd runtime libs (from host glibc) — covers the non-harvested case
        glibc = find_glibc_libs()
        for name, src_path in glibc.items():
            if name == "ld-linux-x86-64.so.2":
                dest = ROOTFS / "lib64" / name
            else:
                dest = ROOTFS / "lib/x86_64-linux-gnu" / name
            if not dest.exists():
                shutil.copy2(src_path, dest)

    # ── Local test binaries ──
    if LOCAL_BIN.exists():
        for f in LOCAL_BIN.iterdir():
            if f.is_file():
                dest = ROOTFS / "bin" / f.name
                shutil.copy2(f, dest)
                os.chmod(dest, 0o755)

    # ── Shared libraries ──
    # libtlstest.so
    tls_lib = LOCAL_LIB / "libtlstest.so"
    if tls_lib.exists():
        shutil.copy2(tls_lib, ROOTFS / "lib" / "libtlstest.so")

    # musl dynamic linker
    musl_libc = find_musl_libc()
    if musl_libc:
        shutil.copy2(musl_libc, ROOTFS / "lib" / "ld-musl-x86_64.so.1")

    # Patched musl disabled (dlopen bug fixed)

    # glibc dynamic libs (for hello-dynamic-glibc, hello-multilib, etc.)
    glibc = find_glibc_libs()
    for name, src_path in glibc.items():
        if name == "ld-linux-x86-64.so.2":
            dest = ROOTFS / "lib64" / name
        else:
            dest = ROOTFS / "lib/x86_64-linux-gnu" / name
        if not dest.exists():
            shutil.copy2(src_path, dest)

    # ── BusyBox symlinks ──
    bb = ROOTFS / "bin" / "busybox"
    if bb.exists():
        r = run([str(bb), "--list-full"], capture_output=True, text=True)
        for line in r.stdout.strip().split("\n"):
            applet = line.strip()
            if not applet:
                continue
            dest = ROOTFS / applet
            dest.parent.mkdir(parents=True, exist_ok=True)
            if not dest.exists():
                os.symlink("/bin/busybox", str(dest))

    # ── Alpine packages (apk.static, OpenRC) ──
    apk_pkg = alpine_pkgs.get("apk-tools-static")
    if apk_pkg:
        apk_static = apk_pkg / "sbin" / "apk.static"
        if apk_static.exists():
            shutil.copy2(apk_static, ROOTFS / "bin" / "apk.static")
            os.chmod(ROOTFS / "bin" / "apk.static", 0o755)

    openrc_pkg = alpine_pkgs.get("openrc")
    if openrc_pkg:
        # Copy OpenRC binaries
        for name in ["openrc", "openrc-run", "rc-update", "rc-service"]:
            src = openrc_pkg / "sbin" / name
            if src.exists():
                shutil.copy2(src, ROOTFS / "sbin" / name)
                os.chmod(ROOTFS / "sbin" / name, 0o755)
        # Copy OpenRC libraries
        for name in ["libeinfo.so.1", "librc.so.1"]:
            for search in [openrc_pkg / "usr" / "lib", openrc_pkg / "lib"]:
                src = search / name
                if src.exists():
                    shutil.copy2(src, ROOTFS / "lib" / name)
                    break
        # Copy OpenRC runtime
        src_libexec = openrc_pkg / "usr" / "libexec" / "rc"
        if src_libexec.exists():
            dest_libexec = ROOTFS / "usr" / "libexec" / "rc"
            if dest_libexec.exists():
                shutil.rmtree(dest_libexec)
            shutil.copytree(src_libexec, dest_libexec, symlinks=True)
        # Copy init scripts and config
        for subdir in ["init.d", "rc.conf", "conf.d"]:
            src = openrc_pkg / "etc" / subdir
            if src.exists():
                dest = ROOTFS / "etc" / subdir
                if src.is_dir():
                    if dest.exists():
                        shutil.rmtree(dest)
                    shutil.copytree(src, dest, symlinks=True)
                else:
                    shutil.copy2(src, dest)

    bb_openrc = alpine_pkgs.get("busybox-openrc")
    if bb_openrc:
        # Copy runlevel configs
        src_runlevels = bb_openrc / "etc" / "runlevels"
        if src_runlevels.exists():
            dest_runlevels = ROOTFS / "etc" / "runlevels"
            if dest_runlevels.exists():
                shutil.rmtree(dest_runlevels)
            shutil.copytree(src_runlevels, dest_runlevels, symlinks=True)

    # ── Config files from testing/etc/ ──
    config_files = ["resolv.conf", "group", "passwd", "shadow",
                    "profile", "inittab", "hostname", "issue", "banner"]
    for name in config_files:
        src = ROOT / "testing" / "etc" / name
        if src.exists():
            shutil.copy2(src, ROOTFS / "etc" / name)

    # Network interfaces
    src = ROOT / "testing" / "etc" / "network" / "interfaces"
    if src.exists():
        (ROOTFS / "etc" / "network").mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, ROOTFS / "etc" / "network" / "interfaces")

    # ── systemd config files ──
    units_dir = ROOT / "testing" / "systemd" / "units"
    if units_dir.exists():
        for unit in units_dir.iterdir():
            shutil.copy2(unit, ROOTFS / "etc" / "systemd" / "system" / unit.name)

    os_release = ROOT / "testing" / "systemd" / "os-release"
    if os_release.exists():
        shutil.copy2(os_release, ROOTFS / "etc" / "os-release")

    fstab = ROOT / "testing" / "systemd" / "fstab"
    if fstab.exists():
        shutil.copy2(fstab, ROOTFS / "etc" / "fstab")

    # kevlar-getty.service symlink
    getty = ROOTFS / "etc" / "systemd" / "system" / "kevlar-getty.service"
    wants = ROOTFS / "etc" / "systemd" / "system" / "multi-user.target.wants"
    if getty.exists():
        link = wants / "kevlar-getty.service"
        if not link.exists():
            os.symlink("/etc/systemd/system/kevlar-getty.service", str(link))

    # ── Other testing files ──
    for name in ["debug_init.sh", "test_apk_update.sh", "test_m10_apk.sh", "test_ktrace_apk.sh"]:
        ("testing/test_apk_write.c",       "test-apk-write",   []),
        ("testing/test_apk_trace.c",       "test-apk-trace",   []),
        ("testing/test_apk_interactive.c", "test-apk-inter",   []),
        src = ROOT / "testing" / name
        if src.exists():
            shutil.copy2(src, ROOTFS / name)
            os.chmod(ROOTFS / name, 0o755)

    integration = ROOT / "testing" / "integration_tests"
    if integration.exists():
        dest = ROOTFS / "integration_tests"
        if dest.exists():
            shutil.rmtree(dest)
        shutil.copytree(integration, dest)

    www = ROOT / "testing" / "var" / "www" / "html" / "index.html"
    if www.exists():
        shutil.copy2(www, ROOTFS / "var" / "www" / "html" / "index.html")

    # ── Override resolv.conf and generate machine-id ──
    # Use QEMU's user-mode network DNS forwarder (10.0.2.3).
    # Direct DNS (e.g. 1.1.1.1) may not work from QEMU guest.
    (ROOTFS / "etc" / "resolv.conf").write_text("nameserver 10.0.2.3\n")
    (ROOTFS / "etc" / "machine-id").write_text(os.urandom(16).hex() + "\n")


# ─── ARM64 Toolchain ──────────────────────────────────────────────────────

MUSL_CC_ARM64_URL = "https://musl.cc/aarch64-linux-musl-cross.tgz"
TOOLCHAIN_ARM64 = CACHE / "toolchain-aarch64"


def fetch_musl_cc_toolchain():
    """Download and cache the musl.cc aarch64 cross-compiler.

    Returns path to aarch64-linux-musl-gcc, or None if unavailable.
    First checks if a system cross-compiler is already installed.
    """
    # Prefer system-installed compiler (faster, no 108 MB download)
    for candidate in ["aarch64-linux-musl-gcc", "aarch64-linux-gnu-gcc"]:
        if shutil.which(candidate):
            log("CC", f"using system cross-compiler: {candidate}")
            return candidate

    gcc = TOOLCHAIN_ARM64 / "bin" / "aarch64-linux-musl-gcc"
    if gcc.exists():
        return str(gcc)

    log("DL", "musl.cc aarch64 cross-compiler (~108 MB, one-time download)")
    tarball = download(MUSL_CC_ARM64_URL, CACHE / "src" / "aarch64-linux-musl-cross.tgz")

    if TOOLCHAIN_ARM64.exists():
        shutil.rmtree(TOOLCHAIN_ARM64)
    TOOLCHAIN_ARM64.mkdir(parents=True)
    log("BUILD", "extracting aarch64 cross-compiler")
    run(["tar", "xzf", str(tarball), "--strip-components=1", "-C", str(TOOLCHAIN_ARM64)],
        capture_output=True)

    if not gcc.exists():
        log("WARN", "musl.cc toolchain extracted but aarch64-linux-musl-gcc not found")
        return None
    return str(gcc)


def compile_all_local_arm64(cc):
    """Cross-compile test binaries for aarch64. Returns list of output paths."""
    log("CC", f"arm64 test binaries ({cc})")
    local_arm64 = CACHE / "local-bin-arm64"
    local_arm64.mkdir(parents=True, exist_ok=True)

    # Binaries to cross-compile: (src_rel, output_name, extra_flags)
    jobs = [
        ("tests/test.c",               "test",              []),
        ("benchmarks/bench.c",         "bench",             []),
        ("testing/busybox_suite.c",    "busybox-suite",     []),
        ("testing/mini_storage.c",     "mini-storage",      []),
        ("testing/mini_threads.c",     "mini-threads",      ["-pthread"]),
        ("testing/fork_exec_stress.c", "fork-exec-stress",  []),
        ("testing/dd_diag.c",          "dd-diag",           []),
        ("testing/test_net.c",         "test-net",          []),
    ]
    # Contract tests
    for src in sorted(ROOT.glob("testing/contracts/*/*.c")):
        jobs.append((str(src.relative_to(ROOT)), "contract-" + src.stem, []))

    # -no-pie: aarch64-linux-musl-gcc defaults to static-pie (ET_DYN), but
    # Kevlar's ARM64 ELF loader handles ET_EXEC (plain static) correctly.
    base_flags = ["-static", "-no-pie", "-O2", "-Wall", "-Wno-unused-result"]

    failed = []
    built = []
    with ThreadPoolExecutor(max_workers=os.cpu_count() or 4) as pool:
        futures = {}
        for src_rel, name, extra in jobs:
            src = ROOT / src_rel if not src_rel.startswith("/") else Path(src_rel)
            out = local_arm64 / name
            f = pool.submit(compile_one, cc, src, out, base_flags + extra)
            futures[f] = (src, name)
        for f in as_completed(futures):
            out_path, ok, err = f.result()
            src, name = futures[f]
            if ok:
                built.append(Path(out_path))
            else:
                log("WARN", f"arm64 {name}: {(err or '').strip()[:120]}")
                failed.append(name)

    if failed:
        log("WARN", f"{len(failed)} arm64 binary(s) failed to compile")
    log("CC", f"arm64: {len(built)} binaries compiled")
    return built


# ─── ARM64 Builders ───────────────────────────────────────────────────────

def fetch_arm64_alpine_pkg(pkg_name):
    """Download an Alpine aarch64 APK and cache it. Returns extracted dir."""
    cached_dir = CACHE / "alpine-pkgs-arm64" / pkg_name
    if cached_dir.exists():
        return cached_dir

    # Fetch APKINDEX to resolve the current version.
    index_path = download(
        f"{ALPINE_MIRROR_ARM64}/APKINDEX.tar.gz",
        CACHE / "src" / "APKINDEX.arm64.tar.gz")
    version = None
    with tarfile.open(index_path, "r:gz") as tar:
        for member in tar.getmembers():
            if member.name == "APKINDEX":
                content = tar.extractfile(member).read().decode()
                cur = {}
                for line in content.split("\n"):
                    if not line:
                        if cur.get("P") == pkg_name:
                            version = cur.get("V")
                            break
                        cur = {}
                    elif ":" in line:
                        k, _, v = line.partition(":")
                        cur[k] = v
                break

    if not version:
        log("WARN", f"ARM64 Alpine package {pkg_name} not found in index")
        return None

    filename = f"{pkg_name}-{version}.apk"
    apk_path = download(f"{ALPINE_MIRROR_ARM64}/{filename}",
                        CACHE / "src" / f"arm64-{filename}")
    cached_dir.mkdir(parents=True, exist_ok=True)
    run(["tar", "xzf", str(apk_path), "-C", str(cached_dir),
         "--exclude=.PKGINFO", "--exclude=.SIGN.*",
         "--exclude=.pre-install", "--exclude=.post-install",
         "--exclude=.trigger"],
        capture_output=True)
    log("ARM64", f"cached {pkg_name} {version}")
    return cached_dir


def build_arm64_packages():
    """Download pre-built aarch64 Alpine packages for the ARM64 initramfs."""
    EXT_BIN_ARM64.mkdir(parents=True, exist_ok=True)
    results = {}

    # busybox-static: provides a fully static /bin/busybox
    pkg = fetch_arm64_alpine_pkg("busybox-static")
    if pkg:
        src = pkg / "bin" / "busybox.static"
        if src.exists():
            dst = EXT_BIN_ARM64 / "busybox"
            shutil.copy2(src, dst)
            os.chmod(dst, 0o755)
            results["busybox"] = dst

    # apk-tools-static: needed for `apk update` / Alpine tests
    pkg = fetch_arm64_alpine_pkg("apk-tools-static")
    if pkg:
        src = pkg / "sbin" / "apk.static"
        if src.exists():
            dst = EXT_BIN_ARM64 / "apk.static"
            shutil.copy2(src, dst)
            os.chmod(dst, 0o755)
            results["apk.static"] = dst

    return results


def assemble_rootfs_arm64(arm64_bins, local_arm64_bins=None):
    """Assemble a minimal aarch64 initramfs rootfs with BusyBox + test config."""
    log("ROOTFS", "assembling (arm64)")

    if ROOTFS.exists():
        shutil.rmtree(ROOTFS)

    for d in ["bin", "sbin", "usr/bin", "usr/sbin",
              "etc", "etc/network", "dev", "proc", "sys", "tmp", "mnt",
              "var/www/html", "run", "lib"]:
        (ROOTFS / d).mkdir(parents=True, exist_ok=True)

    os.symlink("/run", str(ROOTFS / "var" / "run"))

    # BusyBox binary
    bb_src = arm64_bins.get("busybox")
    if bb_src and bb_src.exists():
        dst = ROOTFS / "bin" / "busybox"
        shutil.copy2(bb_src, dst)
        os.chmod(dst, 0o755)
        # BusyBox applet symlinks — run the arm64 binary under qemu-user if
        # not on arm64 host; on build machines we just list a known-good set.
        try:
            r = subprocess.run([str(dst), "--list-full"],
                               capture_output=True, text=True, timeout=5)
            applets = r.stdout.strip().split("\n") if r.returncode == 0 else []
        except Exception:
            applets = []
        if not applets:
            # Fallback: hardcode the most important applets
            applets = [
                "bin/sh", "bin/ash", "bin/cat", "bin/echo", "bin/ls",
                "bin/mkdir", "bin/mount", "bin/umount", "bin/ps",
                "bin/kill", "bin/sleep", "bin/test", "bin/true",
                "bin/false", "bin/grep", "bin/sed", "bin/awk",
                "bin/head", "bin/tail", "bin/wc", "bin/cut",
                "sbin/init", "sbin/halt", "sbin/reboot",
                "usr/bin/env", "usr/bin/id",
            ]
        for applet in applets:
            applet = applet.strip()
            if not applet:
                continue
            dest = ROOTFS / applet
            dest.parent.mkdir(parents=True, exist_ok=True)
            if not dest.exists():
                os.symlink("/bin/busybox", str(dest))
    else:
        log("WARN", "ARM64 BusyBox not found — initramfs will have no shell")

    # apk.static
    apk_src = arm64_bins.get("apk.static")
    if apk_src and apk_src.exists():
        dst = ROOTFS / "bin" / "apk.static"
        shutil.copy2(apk_src, dst)
        os.chmod(dst, 0o755)

    # Config files from testing/etc/
    for name in ["resolv.conf", "group", "passwd", "shadow",
                 "profile", "inittab", "hostname", "issue", "banner"]:
        src = ROOT / "testing" / "etc" / name
        if src.exists():
            shutil.copy2(src, ROOTFS / "etc" / name)

    src_net = ROOT / "testing" / "etc" / "network" / "interfaces"
    if src_net.exists():
        shutil.copy2(src_net, ROOTFS / "etc" / "network" / "interfaces")

    # ── Cross-compiled arm64 test binaries ──
    if local_arm64_bins:
        for f in local_arm64_bins:
            if f.is_file():
                dest = ROOTFS / "bin" / f.name
                # dest may be a BusyBox symlink; unlink before copying
                if dest.exists() or dest.is_symlink():
                    dest.unlink()
                shutil.copy2(f, dest)
                os.chmod(dest, 0o755)

    # Always use QEMU's user-mode DNS forwarder
    (ROOTFS / "etc" / "resolv.conf").write_text("nameserver 10.0.2.3\n")
    (ROOTFS / "etc" / "machine-id").write_text(os.urandom(16).hex() + "\n")

    # Minimal /init that launches /bin/sh (BusyBox ash)
    init_script = ROOTFS / "init"
    init_script.write_text(
        "#!/bin/sh\n"
        "mount -t proc proc /proc\n"
        "mount -t sysfs sysfs /sys\n"
        "exec /bin/sh\n"
    )
    os.chmod(str(init_script), 0o755)


# ─── Main ─────────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(
        description="Native Linux initramfs builder for Kevlar")
    parser.add_argument("outfile", help="Output CPIO archive path")
    parser.add_argument("--clean", action="store_true",
                        help="Clean cached external packages and rebuild all")
    parser.add_argument("--clean-local", action="store_true",
                        help="Clean only local test binaries (fast rebuild)")
    parser.add_argument("--skip-externals", action="store_true",
                        help="Skip building external packages (use cache only)")
    parser.add_argument("--arch", default=None,
                        help="Target architecture: x64 or arm64 "
                             "(default: $ARCH env var, fallback x64)")
    args = parser.parse_args()

    arch = args.arch or os.environ.get("ARCH", "x64")

    if args.clean:
        if CACHE.exists():
            log("CLEAN", "all caches")
            shutil.rmtree(CACHE)
    elif args.clean_local:
        if LOCAL_BIN.exists():
            log("CLEAN", "local binaries")
            shutil.rmtree(LOCAL_BIN)
        if LOCAL_LIB.exists():
            shutil.rmtree(LOCAL_LIB)

    # ── ARM64 path ────────────────────────────────────────────────────────
    if arch == "arm64":
        log("ARCH", "arm64 — downloading pre-built Alpine aarch64 binaries")
        arm64_bins = build_arm64_packages()
        local_arm64_bins = []
        if not args.skip_externals:
            cc = fetch_musl_cc_toolchain()
            if cc:
                local_arm64_bins = compile_all_local_arm64(cc)
            else:
                log("WARN", "no aarch64 cross-compiler found; skipping test binary compilation")
        assemble_rootfs_arm64(arm64_bins, local_arm64_bins)
        log("CPIO", args.outfile)
        sys.path.insert(0, str(ROOT / "tools"))
        from docker2initramfs import create_cpio_archive
        create_cpio_archive(ROOTFS, args.outfile)
        log("DONE", args.outfile)
        return

    # ── x86_64 path ───────────────────────────────────────────────────────

    # Check prerequisites
    for tool in ["musl-gcc", "gcc"]:
        if not shutil.which(tool):
            print(f"Error: {tool} not found. Install musl-tools / build-essential.",
                  file=sys.stderr)
            sys.exit(1)

    # Build external packages (cached).  BusyBox is required; others are
    # optional — a download/build failure won't block the overall build.
    if not args.skip_externals:
        log("BUILD", "external packages (cached)")
        build_busybox()  # required: provides /bin/sh
        for name, fn in [("curl", build_curl), ("dropbear", build_dropbear),
                         ("bash", build_bash)]:
            try:
                fn()
            except Exception as e:
                log("WARN", f"{name} build failed: {e}")
        have_systemd = build_systemd()
        alpine_pkgs = download_alpine_packages()
    else:
        have_systemd = (EXT_BIN / "systemd").exists()
        alpine_pkgs = {}
        for pkg in ["apk-tools-static", "openrc", "busybox-openrc"]:
            d = CACHE / "alpine-pkgs" / pkg
            if d.exists():
                alpine_pkgs[pkg] = d

    # Build local test binaries (incremental)
    compile_all_local()

    # Assemble rootfs
    assemble_rootfs(have_systemd, alpine_pkgs)

    # Create CPIO archive
    log("CPIO", args.outfile)
    sys.path.insert(0, str(ROOT / "tools"))
    from docker2initramfs import create_cpio_archive
    create_cpio_archive(ROOTFS, args.outfile)
    log("DONE", args.outfile)


if __name__ == "__main__":
    main()
