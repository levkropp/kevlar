// Minimal dlopen test — dynamically linked against musl.
// Tests whether runtime library loading via dlopen() works.
//
// Build: musl-gcc -o test_dlopen test_dlopen.c -ldl
// (NOTE: no -static! Must be dynamically linked.)
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <stdarg.h>
#include <string.h>
#include <dlfcn.h>
#include <unistd.h>
#include <fcntl.h>
#include <dirent.h>

static void msg(const char *s) { write(1, s, strlen(s)); }
static void msgf(const char *fmt, ...) {
    char buf[256]; va_list ap;
    va_start(ap, fmt); int n = vsnprintf(buf, sizeof(buf), fmt, ap); va_end(ap);
    write(1, buf, n);
}

// Read auxv from /proc/self/auxv
static void dump_auxv(void) {
    int fd = open("/proc/self/auxv", O_RDONLY);
    if (fd < 0) { msg("DIAG: cannot open /proc/self/auxv\n"); return; }
    unsigned long buf[2];
    while (read(fd, buf, sizeof(buf)) == sizeof(buf)) {
        if (buf[0] == 0) break; // AT_NULL
        // AT_BASE=7, AT_PHDR=3, AT_ENTRY=9, AT_PHNUM=5
        if (buf[0] == 7) msgf("DIAG: AT_BASE=%#lx\n", buf[1]);
        if (buf[0] == 3) msgf("DIAG: AT_PHDR=%#lx\n", buf[1]);
        if (buf[0] == 9) msgf("DIAG: AT_ENTRY=%#lx\n", buf[1]);
    }
    close(fd);
}

int main(void) {
    msg("=== dlopen test ===\n");
    dump_auxv();

    // Test 1: dlopen libcrypto.so.3 (large library, 4.3MB)
    msg("TEST: dlopen libcrypto.so.3 ... ");
    void *h = dlopen("libcrypto.so.3", RTLD_NOW | RTLD_LOCAL);
    if (h) {
        msg("OK\n");
        void *sym = dlsym(h, "OPENSSL_version_major");
        if (sym) {
            unsigned int (*fn)(void) = (unsigned int (*)(void))sym;
            msgf("  OPENSSL_version_major() = %u\n", fn());
            msg("TEST_PASS dlopen_libcrypto\n");
        } else {
            msgf("  dlsym failed: %s\n", dlerror());
            msg("TEST_FAIL dlopen_libcrypto\n");
        }
        dlclose(h);
    } else {
        msgf("FAIL: %s\n", dlerror());
        msg("TEST_FAIL dlopen_libcrypto\n");
    }

    // Test 2: dlopen libssl.so.3
    msg("TEST: dlopen libssl.so.3 ... ");
    h = dlopen("libssl.so.3", RTLD_NOW | RTLD_LOCAL);
    if (h) {
        msg("OK\n");
        msg("TEST_PASS dlopen_libssl\n");
        dlclose(h);
    } else {
        msgf("FAIL: %s\n", dlerror());
        msg("TEST_FAIL dlopen_libssl\n");
    }

    // Test 3: dlopen a small .so (libz if available)
    msg("TEST: dlopen libz.so.1 ... ");
    h = dlopen("libz.so.1", RTLD_NOW | RTLD_LOCAL);
    if (h) {
        msg("OK\n");
        msg("TEST_PASS dlopen_libz\n");
        dlclose(h);
    } else {
        msgf("FAIL (ok if not installed): %s\n", dlerror());
    }

    // Test 4: Stress test - create many VMAs then dlopen
    // Python has ~100 VMAs when it calls import math. Test if many VMAs
    // causes the dlopen crash.
    msg("TEST: stress dlopen with many VMAs ... ");
    {
        #include <sys/mman.h>
        void *maps[100];
        int nmaps = 0;
        for (int i = 0; i < 100; i++) {
            maps[i] = mmap(NULL, 4096, PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANONYMOUS, -1, 0);
            if (maps[i] != MAP_FAILED) nmaps++;
        }
        msgf("created %d anonymous VMAs, now dlopen... ", nmaps);
        void *sh = dlopen("libcrypto.so.3", RTLD_NOW | RTLD_LOCAL);
        if (sh) {
            msg("OK\n");
            msg("TEST_PASS dlopen_stress\n");
            dlclose(sh);
        } else {
            msgf("FAIL: %s\n", dlerror());
            msg("TEST_FAIL dlopen_stress\n");
        }
        for (int i = 0; i < 100; i++) {
            if (maps[i] != MAP_FAILED) munmap(maps[i], 4096);
        }
    }

    // Test 5: Try loading libpython (the large library Python links against)
    msg("TEST: dlopen libpython3.12.so.1.0 ... ");
    h = dlopen("libpython3.12.so.1.0", RTLD_NOW | RTLD_GLOBAL);
    if (h) {
        msg("OK\n");
        // Now try loading a Python extension WITH libpython available
        void *mh = dlopen("/usr/lib/python3.12/lib-dynload/math.cpython-312-x86_64-linux-musl.so",
                          RTLD_NOW | RTLD_LOCAL);
        if (mh) {
            msg("TEST_PASS dlopen_math_with_libpython\n");
            dlclose(mh);
        } else {
            msgf("  math.so: %s\n", dlerror());
            msg("TEST_FAIL dlopen_math_with_libpython\n");
        }
        dlclose(h);
    } else {
        msgf("FAIL: %s\n", dlerror());
        msg("TEST_FAIL dlopen_libpython\n");
    }

    // Test 5: Try loading a Python extension module (the exact file that crashes)
    // Python uses RTLD_NOW | RTLD_LOCAL (same as our test above).
    // But Python's import also calls into the loaded module.
    // Find any .so in lib-dynload:
    {
        const char *dynload = "/usr/lib/python3.12/lib-dynload";
        DIR *dp = opendir(dynload);
        if (dp) {
            struct dirent *ep;
            while ((ep = readdir(dp))) {
                int nlen = strlen(ep->d_name);
                if (nlen > 3 && strcmp(ep->d_name + nlen - 3, ".so") == 0) {
                    char path[512];
                    snprintf(path, sizeof(path), "%s/%s", dynload, ep->d_name);
                    msgf("TEST: dlopen %s ... ", ep->d_name);
                    void *mh = dlopen(path, RTLD_NOW | RTLD_LOCAL);
                    if (mh) {
                        msgf("OK\n");
                        // Try finding PyInit_ symbol
                        char initname[256] = "PyInit_";
                        // Extract module name from filename (before first '.')
                        char modname[128];
                        strncpy(modname, ep->d_name, sizeof(modname));
                        char *dot = strchr(modname, '.');
                        if (dot) *dot = '\0';
                        strncat(initname, modname, sizeof(initname) - 8);
                        void *initsym = dlsym(mh, initname);
                        msgf("  %s: %s\n", initname, initsym ? "found" : "not found");
                        msgf("TEST_PASS dlopen_pyext_%s\n", modname);
                        dlclose(mh);
                    } else {
                        msgf("FAIL: %s\n", dlerror());
                        msgf("TEST_FAIL dlopen_pyext_%s\n", ep->d_name);
                    }
                    break; // just test first one
                }
            }
            closedir(dp);
        } else {
            msg("DIAG: /usr/lib/python3.12/lib-dynload not found (python3 not installed yet?)\n");
        }
    }

    // Test: Check if libpython RELR/JMPREL targets page 0 (where .gnu.hash lives)
    {
        int fd2 = open("/usr/lib/libpython3.12.so.1.0", O_RDONLY);
        if (fd2 >= 0) {
            // Read ELF header
            unsigned char ehdr[64];
            pread(fd2, ehdr, 64, 0);
            unsigned long e_phoff = *(unsigned long*)(ehdr+32);
            unsigned short e_phnum = *(unsigned short*)(ehdr+56);
            unsigned short e_phentsize = *(unsigned short*)(ehdr+54);

            // Find PT_DYNAMIC
            unsigned long dyn_off = 0, dyn_sz = 0;
            for (int i = 0; i < e_phnum; i++) {
                unsigned char phdr[56];
                pread(fd2, phdr, 56, e_phoff + i * e_phentsize);
                unsigned int p_type = *(unsigned int*)phdr;
                if (p_type == 2) { // PT_DYNAMIC
                    dyn_off = *(unsigned long*)(phdr+8);
                    dyn_sz = *(unsigned long*)(phdr+32);
                }
            }

            // Parse dynamic section for DT_RELR, DT_JMPREL, DT_RELA
            unsigned long relr_off=0, relr_sz=0, rela_off=0, rela_sz=0, jmprel_off=0, jmprel_sz=0;
            if (dyn_off > 0) {
                unsigned char *dynbuf = malloc(dyn_sz);
                pread(fd2, dynbuf, dyn_sz, dyn_off);
                for (unsigned long p = 0; p < dyn_sz; p += 16) {
                    long tag = *(long*)(dynbuf+p);
                    unsigned long val = *(unsigned long*)(dynbuf+p+8);
                    if (tag == 7) rela_off = val;
                    if (tag == 8) rela_sz = val;
                    if (tag == 36) relr_off = val;
                    if (tag == 35) relr_sz = val;
                    if (tag == 23) jmprel_off = val;
                    if (tag == 2) jmprel_sz = val;
                    if (tag == 0) break;
                }
                free(dynbuf);
            }

            msgf("DIAG: libpython RELA=%#lx/%lu RELR=%#lx/%lu JMPREL=%#lx/%lu\n",
                 rela_off, rela_sz, relr_off, relr_sz, jmprel_off, jmprel_sz);

            // Scan RELR entries for addresses < 0x1000
            if (relr_sz > 0) {
                unsigned char *relrbuf = malloc(relr_sz);
                pread(fd2, relrbuf, relr_sz, relr_off);
                // Process RELR exactly as musl does: addr increments after address entries
                unsigned long addr = 0;
                int page0_count = 0;
                size_t *where = 0;
                for (unsigned long i = 0; i < relr_sz; i += 8) {
                    unsigned long entry = *(unsigned long*)(relrbuf+i);
                    if (!(entry & 1)) {
                        // Address entry: write at entry, advance past it
                        where = (size_t*)(uintptr_t)entry;
                        if (entry < 0x1000) {
                            msgf("  RELR targets page 0: addr=%#lx\n", entry);
                            page0_count++;
                        }
                        where++; // musl does reloc_addr++
                    } else {
                        // Bitmap entry: check each bit
                        int j = 0;
                        for (unsigned long bitmap = entry; (bitmap >>= 1); j++) {
                            if (bitmap & 1) {
                                unsigned long target = (unsigned long)where + j*8;
                                if (target < 0x1000) {
                                    msgf("  RELR bitmap targets page 0: %#lx (from where=%p j=%d)\n",
                                         target, (void*)where, j);
                                    page0_count++;
                                }
                            }
                        }
                        where += 8*sizeof(size_t)/8 - 1; // = 63
                    }
                }
                msgf("DIAG: RELR entries targeting page 0: %d (total entries=%lu)\n",
                     page0_count, relr_sz/8);
                free(relrbuf);
            }

            // Scan RELA entries for targets in page 0
            if (rela_sz > 0) {
                int rp0 = 0;
                for (unsigned long off = 0; off < rela_sz; off += 24) {
                    unsigned long r_off;
                    pread(fd2, &r_off, 8, rela_off + off);
                    if (r_off < 0x1000) {
                        unsigned long r_info;
                        pread(fd2, &r_info, 8, rela_off + off + 8);
                        msgf("  RELA targets page 0: r_off=%#lx type=%lu sym=%lu\n",
                             r_off, r_info & 0xffffffffUL, r_info >> 32);
                        rp0++;
                        if (rp0 > 20) { msg("  (truncated)\n"); break; }
                    }
                }
                msgf("DIAG: RELA entries targeting page 0: %d (total=%lu)\n",
                     rp0, rela_sz/24);
            }

            // Also scan JMPREL (which is RELA format)
            if (jmprel_sz > 0) {
                unsigned char *jbuf = malloc(jmprel_sz);
                pread(fd2, jbuf, jmprel_sz, jmprel_off);
                int jp0 = 0;
                for (unsigned long i = 0; i < jmprel_sz; i += 24) {
                    unsigned long r_off = *(unsigned long*)(jbuf+i);
                    if (r_off < 0x1000) {
                        msgf("  JMPREL targets page 0: r_off=%#lx\n", r_off);
                        jp0++;
                    }
                }
                msgf("DIAG: JMPREL entries targeting page 0: %d\n", jp0);
                free(jbuf);
            }

            close(fd2);
        }
    }

    msg("=== dlopen test done ===\n");
    return 0;
}
