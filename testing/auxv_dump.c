// Walk our own auxv from the initial stack and print each entry.
// Static musl — runs before any dynamic linker, so shows what the kernel
// actually delivered.
#define _GNU_SOURCE
#include <stdio.h>
#include <stdint.h>
#include <elf.h>
#include <unistd.h>

// Musl installs __environ in a global; the auxv follows envp+NULL.
extern char **__environ;

static const char *auxv_name(unsigned long t) {
    switch (t) {
        case AT_NULL: return "AT_NULL";
        case AT_IGNORE: return "AT_IGNORE";
        case AT_EXECFD: return "AT_EXECFD";
        case AT_PHDR: return "AT_PHDR";
        case AT_PHENT: return "AT_PHENT";
        case AT_PHNUM: return "AT_PHNUM";
        case AT_PAGESZ: return "AT_PAGESZ";
        case AT_BASE: return "AT_BASE";
        case AT_FLAGS: return "AT_FLAGS";
        case AT_ENTRY: return "AT_ENTRY";
        case AT_NOTELF: return "AT_NOTELF";
        case AT_UID: return "AT_UID";
        case AT_EUID: return "AT_EUID";
        case AT_GID: return "AT_GID";
        case AT_EGID: return "AT_EGID";
        case AT_CLKTCK: return "AT_CLKTCK";
        case AT_PLATFORM: return "AT_PLATFORM";
        case AT_HWCAP: return "AT_HWCAP";
        case AT_FPUCW: return "AT_FPUCW";
        case AT_DCACHEBSIZE: return "AT_DCACHEBSIZE";
        case AT_ICACHEBSIZE: return "AT_ICACHEBSIZE";
        case AT_UCACHEBSIZE: return "AT_UCACHEBSIZE";
        case AT_SECURE: return "AT_SECURE";
        case AT_BASE_PLATFORM: return "AT_BASE_PLATFORM";
        case AT_RANDOM: return "AT_RANDOM";
        case AT_HWCAP2: return "AT_HWCAP2";
        case AT_EXECFN: return "AT_EXECFN";
        case AT_SYSINFO: return "AT_SYSINFO";
        case AT_SYSINFO_EHDR: return "AT_SYSINFO_EHDR";
        case 51: return "AT_MINSIGSTKSZ";
        default: return "AT_???";
    }
}

int main(int argc, char **argv, char **envp) {
    // Find end of envp (the NULL sentinel). auxv starts right after.
    char **e = envp;
    while (*e) e++;
    e++;  // skip NULL
    Elf64_auxv_t *av = (Elf64_auxv_t *)e;

    printf("AUXV_DUMP pid=%d argc=%d\n", (int)getpid(), argc);
    for (int i = 0; i < argc; i++) {
        printf("argv[%d] = \"%s\"\n", i, argv[i]);
    }
    int env_count = 0;
    for (char **p = envp; *p; p++) env_count++;
    printf("envp_count = %d\n", env_count);
    // Dump first few env vars to prove envp is intact.
    for (int i = 0; i < env_count && i < 3; i++) {
        printf("envp[%d] = \"%s\"\n", i, envp[i]);
    }

    for (int i = 0; av[i].a_type != AT_NULL; i++) {
        unsigned long t = av[i].a_type;
        unsigned long v = av[i].a_un.a_val;
        const char *name = auxv_name(t);
        // For AT_RANDOM / AT_PLATFORM / AT_EXECFN print the pointed-to data
        // so we can see if the pointer is valid.
        if (t == AT_RANDOM) {
            unsigned char *p = (unsigned char *)v;
            if (p) {
                printf("%s = %p [%02x %02x %02x %02x %02x %02x %02x %02x ...]\n",
                       name, p,
                       p[0], p[1], p[2], p[3], p[4], p[5], p[6], p[7]);
            } else {
                printf("%s = NULL\n", name);
            }
        } else if (t == AT_PLATFORM || t == AT_EXECFN) {
            char *s = (char *)v;
            printf("%s = %p (\"%s\")\n", name, s, s ? s : "(null)");
        } else {
            printf("%s = 0x%lx (%lu)\n", name, v, v);
        }
    }
    printf("AUXV_DUMP END\n");
    return 0;
}
