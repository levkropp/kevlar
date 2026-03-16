#!/usr/bin/env python3
"""Native Linux initramfs builder for Kevlar (no Docker required).

Compiles all test binaries directly with musl-gcc/gcc and downloads
external packages (BusyBox, curl, dropbear, bash) from source with
caching.  Subsequent builds only recompile changed .c files.

Usage:
    python3 tools/build-initramfs.py build/testing.initramfs
    python3 tools/build-initramfs.py --clean build/testing.initramfs

Prerequisites (Arch):   pacman -S musl gcc e2fsprogs
Prerequisites (Ubuntu): apt install musl-tools build-essential linux-libc-dev e2fsprogs
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
LOCAL_BIN = CACHE / "local-bin"   # Compiled test binaries
LOCAL_LIB = CACHE / "local-lib"   # Compiled shared libs
EXT_BIN = CACHE / "ext-bin"       # External package binaries
ROOTFS = ROOT / "build" / "initramfs-rootfs"

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
        ("testing/busybox_suite.c",      "busybox-suite",    []),
        ("testing/dd_diag.c",            "dd-diag",          []),
        ("testing/test_net.c",           "test-net",         []),
        ("testing/test_alpine.c",        "test-alpine",      []),
        ("testing/disk_hello.c",         "disk_hello",       []),
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
    if out_db.exists() and out_key.exists():
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


def build_systemd():
    """Build systemd v245 from source (optional, requires meson < 1.0).

    Returns True if systemd binaries are available, False otherwise.
    """
    systemd_bin = EXT_BIN / "systemd"
    libsystemd = EXT_BIN / "libsystemd-shared-245.so"
    if systemd_bin.exists() and libsystemd.exists():
        return True

    # Check meson version — systemd v245 needs meson < 1.0
    meson = shutil.which("meson")
    if not meson:
        log("SKIP", "systemd (meson not found)")
        return False

    try:
        r = run(["meson", "--version"], capture_output=True, text=True)
        ver = r.stdout.strip()
        major = int(ver.split(".")[0])
        if major >= 1:
            log("SKIP", f"systemd (meson {ver} too new, needs < 1.0)")
            return False
    except (subprocess.CalledProcessError, ValueError):
        log("SKIP", "systemd (cannot determine meson version)")
        return False

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
             "-Dopenssl=false", "-Dcurl=false", "-Dtpm=false",
             "-Dzlib=false", "-Dbzip2=false", "-Dzstd=false",
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

        # Also grab systemd-journald and systemctl if built
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
              "usr/lib/systemd/system", "usr/lib/systemd"]:
        (ROOTFS / d).mkdir(parents=True, exist_ok=True)

    # var/run → run
    os.symlink("/run", str(ROOTFS / "var" / "run"))

    # ── External binaries ──
    ext_copy = {
        "busybox":     "bin/busybox",
        "curl":        "bin/curl",
        "dropbear":    "bin/dropbear",
        "dropbearkey": "bin/dropbearkey",
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
        libsystemd = EXT_BIN / "libsystemd-shared-245.so"
        if systemd_bin.exists():
            shutil.copy2(systemd_bin, ROOTFS / "usr/lib/systemd/systemd")
            os.chmod(ROOTFS / "usr/lib/systemd/systemd", 0o755)
        if libsystemd.exists():
            shutil.copy2(libsystemd, ROOTFS / "lib/systemd/libsystemd-shared-245.so")
            # Also copy to standard search path
            shutil.copy2(libsystemd,
                         ROOTFS / "lib/x86_64-linux-gnu/libsystemd-shared-245.so")
        for extra in ["systemd-journald", "systemctl"]:
            src = EXT_BIN / extra
            if src.exists():
                shutil.copy2(src, ROOTFS / "usr/lib/systemd" / extra)
                os.chmod(ROOTFS / "usr/lib/systemd" / extra, 0o755)

        # systemd runtime libs (from host glibc)
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
    for name in ["debug_init.sh", "test_apk_update.sh"]:
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
    (ROOTFS / "etc" / "resolv.conf").write_text("nameserver 1.1.1.1\n")
    (ROOTFS / "etc" / "machine-id").write_text(os.urandom(16).hex() + "\n")


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
    args = parser.parse_args()

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
