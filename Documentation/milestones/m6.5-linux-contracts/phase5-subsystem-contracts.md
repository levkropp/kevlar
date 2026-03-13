# Phase 5: Subsystem Contracts

**Duration:** ~3-4 days
**Prerequisite:** Phases 1-4
**Goal:** Validate kernel subsystem contracts—/proc layout, /sys internals, device nodes, DRM stubs.

## Scope

Kernel subsystems expose interfaces via /proc, /sys, and device nodes. Complex programs (systemd, udev, GPU drivers) depend on:
- **/proc layout:** /proc/[pid]/stat, /proc/[pid]/maps, /proc/cpuinfo format and field semantics
- **/sys hierarchy:** /sys/devices, /sys/class, /sys/module layout and attribute semantics
- **Device nodes:** /dev/null, /dev/zero, /dev/urandom permissions and behavior
- **DRM subsystem:** /dev/dri/card*, /dev/dri/renderD128 device numbers, ioctl dispatch (stub)
- **Filesystem permissions:** /proc and /sys file permissions match Linux exactly

These are not POSIX; they're Linux-specific implementation details.

## Contracts to Validate

### 1. /proc/cpuinfo Format

**Contract:** /proc/cpuinfo reports CPU information in exact Linux format. glibc and system tools parse this.

**Test:** `testing/contracts/subsystems/proc_cpuinfo.c`
```c
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>

int main() {
    // Read /proc/cpuinfo
    int fd = open("/proc/cpuinfo", O_RDONLY);
    if (fd < 0) {
        printf("ERROR: /proc/cpuinfo not found\n");
        return 1;
    }

    char buf[4096];
    ssize_t n = read(fd, buf, sizeof(buf));
    close(fd);

    if (n <= 0) {
        printf("ERROR: /proc/cpuinfo read failed\n");
        return 1;
    }

    buf[n] = '\0';

    // Check for required fields
    const char *required[] = {
        "processor\t:",
        "vendor_id\t:",
        "cpu family\t:",
        "model\t\t:",
        "stepping\t:",
        "flags\t\t:",
    };

    int found_all = 1;
    for (int i = 0; i < sizeof(required)/sizeof(required[0]); i++) {
        if (strstr(buf, required[i]) == NULL) {
            printf("ERROR: missing field: %s\n", required[i]);
            found_all = 0;
        }
    }

    if (found_all) {
        printf("proc_cpuinfo format ok\n");
        return 0;
    } else {
        printf("proc_cpuinfo:\n%s\n", buf);
        return 1;
    }
}
```

**Why:** Tools like `lscpu`, glibc, and CPUID libraries parse /proc/cpuinfo. Wrong format breaks feature detection.

### 2. /proc/[pid]/stat Format

**Contract:** /proc/[pid]/stat contains process statistics in exact Linux format (space-separated fields).

**Test:** `testing/contracts/subsystems/proc_pid_stat.c`
```c
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>

int main() {
    // Read /proc/self/stat
    int fd = open("/proc/self/stat", O_RDONLY);
    if (fd < 0) {
        printf("ERROR: /proc/self/stat not found\n");
        return 1;
    }

    char buf[1024];
    ssize_t n = read(fd, buf, sizeof(buf));
    close(fd);

    if (n <= 0) {
        printf("ERROR: /proc/self/stat read failed\n");
        return 1;
    }

    buf[n] = '\0';

    // Parse: pid (comm) state ppid ...
    // Example: "1 (init) S 0 1 1 0 -1 4194304 ..."
    int pid, ppid;
    char comm[256], state;

    int parsed = sscanf(buf, "%d (%255[^)]) %c %d",
                        &pid, comm, &state, &ppid);

    if (parsed != 4) {
        printf("ERROR: /proc/self/stat parse failed: %s\n", buf);
        return 1;
    }

    // Verify state is valid
    if (state != 'S' && state != 'R' && state != 'D') {
        printf("ERROR: invalid state: %c\n", state);
        return 1;
    }

    printf("proc_pid_stat format ok (pid=%d, comm=%s, state=%c)\n", pid, comm, state);
    return 0;
}
```

**Why:** `ps` and other tools parse /proc/[pid]/stat. Wrong format breaks process introspection.

### 3. /proc/[pid]/maps Format

**Contract:** /proc/[pid]/maps shows memory mappings in exact Linux format.

**Test:** `testing/contracts/subsystems/proc_pid_maps.c`
```c
#include <stdio.h>
#include <fcntl.h>
#include <unistd.h>
#include <string.h>

int main() {
    // Read /proc/self/maps
    int fd = open("/proc/self/maps", O_RDONLY);
    if (fd < 0) {
        printf("ERROR: /proc/self/maps not found\n");
        return 1;
    }

    char buf[4096];
    ssize_t n = read(fd, buf, sizeof(buf));
    close(fd);

    if (n <= 0) {
        printf("ERROR: /proc/self/maps read failed\n");
        return 1;
    }

    buf[n] = '\0';

    // Parse first line: address perms offset dev inode pathname
    // Example: "55f07e000-55f08e000 r-xp 00000000 08:02 123456    /bin/bash"
    unsigned long start, end;
    char perms[5], dev[10];
    int parsed = sscanf(buf, "%lx-%lx %4s %*s %9s",
                        &start, &end, perms, dev);

    if (parsed != 4) {
        printf("ERROR: /proc/self/maps parse failed: %s\n", buf);
        return 1;
    }

    if (strlen(perms) != 4) {
        printf("ERROR: perms should be 4 chars (r/w/x/p), got %s\n", perms);
        return 1;
    }

    printf("proc_pid_maps format ok\n");
    return 0;
}
```

**Why:** Debuggers, profilers, and memory analysis tools parse /proc/[pid]/maps. Wrong format breaks address space introspection.

### 4. /proc File Permissions

**Contract:** /proc files have specific ownership and permissions that reflect security policy.

**Test:** `testing/contracts/subsystems/proc_permissions.c`
```c
#include <stdio.h>
#include <sys/stat.h>
#include <unistd.h>

int main() {
    struct stat st;

    // /proc/cpuinfo should be world-readable
    if (stat("/proc/cpuinfo", &st) < 0) {
        printf("ERROR: /proc/cpuinfo not found\n");
        return 1;
    }

    if (!(st.st_mode & S_IROTH)) {
        printf("ERROR: /proc/cpuinfo not world-readable\n");
        return 1;
    }

    // /proc/self/maps should be readable by owner
    if (stat("/proc/self/maps", &st) < 0) {
        printf("ERROR: /proc/self/maps not found\n");
        return 1;
    }

    if (!(st.st_mode & S_IRUSR)) {
        printf("ERROR: /proc/self/maps not readable by owner\n");
        return 1;
    }

    printf("proc_permissions ok\n");
    return 0;
}
```

**Why:** Incorrect permissions expose sensitive information (memory maps, cmdlines) or prevent legitimate access.

### 5. /sys/devices Hierarchy

**Contract:** /sys/devices, /sys/class, /sys/module exist and have correct layout.

**Test:** `testing/contracts/subsystems/sys_hierarchy.c`
```c
#include <stdio.h>
#include <unistd.h>
#include <dirent.h>

int main() {
    const char *paths[] = {
        "/sys/devices",
        "/sys/class",
        "/sys/module",
        "/sys/bus",
    };

    for (int i = 0; i < sizeof(paths)/sizeof(paths[0]); i++) {
        DIR *d = opendir(paths[i]);
        if (!d) {
            printf("WARNING: %s not found\n", paths[i]);
            // Don't fail; /sys might be minimal in tests
            continue;
        }
        closedir(d);
    }

    printf("sys_hierarchy ok (basic)\n");
    return 0;
}
```

**Why:** udev, systemd, and other system tools expect /sys hierarchy. Missing dirs cause tool failures.

### 6. Device Node Permissions

**Contract:** /dev nodes have correct major/minor numbers, ownership, and permissions.

**Test:** `testing/contracts/subsystems/dev_permissions.c`
```c
#include <stdio.h>
#include <sys/stat.h>
#include <sys/types.h>

int main() {
    struct stat st;

    // /dev/null should be character device, major=1, minor=3
    if (stat("/dev/null", &st) < 0) {
        printf("ERROR: /dev/null not found\n");
        return 1;
    }

    if (!S_ISCHR(st.st_mode)) {
        printf("ERROR: /dev/null is not a character device\n");
        return 1;
    }

    int major = major(st.st_rdev);
    int minor = minor(st.st_rdev);

    if (major != 1 || minor != 3) {
        printf("ERROR: /dev/null has wrong major/minor (%d:%d, expected 1:3)\n", major, minor);
        return 1;
    }

    // /dev/zero should be major=1, minor=5
    if (stat("/dev/zero", &st) < 0) {
        printf("ERROR: /dev/zero not found\n");
        return 1;
    }

    major = major(st.st_rdev);
    minor = minor(st.st_rdev);

    if (major != 1 || minor != 5) {
        printf("ERROR: /dev/zero has wrong major/minor (%d:%d, expected 1:5)\n", major, minor);
        return 1;
    }

    printf("dev_permissions ok\n");
    return 0;
}
```

**Why:** Incorrect device numbers break ioctl dispatch and device access.

### 7. DRM Device Existence

**Contract:** /dev/dri/card0, /dev/dri/renderD128 exist (even if not functional).

**Test:** `testing/contracts/subsystems/drm_devices.c`
```c
#include <stdio.h>
#include <unistd.h>
#include <sys/stat.h>

int main() {
    // For now, just check existence
    // Functional DRM testing comes in Phase 6

    // /dev/dri might exist
    if (stat("/dev/dri", &st) == 0) {
        printf("drm_devices: /dev/dri exists (good)\n");
    } else {
        printf("drm_devices: /dev/dri not found (expected for M6.5)\n");
    }

    // /dev/dri/card0 might not exist yet
    // That's OK; it comes in M10

    printf("drm_devices ok (stub)\n");
    return 0;
}
```

**Why:** GPU drivers need /dev/dri structure. For M6.5, just stub it.

## Implementation Plan

1. **Write tests** (all 7 test files)
2. **Compile** with musl-gcc -static
3. **Run harness**
4. **Document divergences**
5. **Fix each divergence:**
   - /proc/cpuinfo: Ensure format matches Linux exactly (field names, tabs, newlines)
   - /proc/[pid]/stat: Ensure format is space-separated, state field is valid
   - /proc/[pid]/maps: Ensure format matches Linux (address, perms, offset, dev, inode, pathname)
   - /proc permissions: Ensure files are readable by intended audience
   - /sys hierarchy: Ensure directories exist (or stub minimally)
   - Device nodes: Ensure major/minor numbers are correct
   - DRM devices: Stub for now

## Testing Phases

**Phase 5a (1 day):** Write tests, run on Linux and Kevlar

**Phase 5b (2 days):** Fix format divergences (/proc, /sys, device nodes)

**Phase 5c (1 day):** Verify all formats match Linux exactly, regression test

## Success Criteria

- [ ] /proc/cpuinfo format test PASS
- [ ] /proc/[pid]/stat format test PASS
- [ ] /proc/[pid]/maps format test PASS
- [ ] /proc file permissions test PASS
- [ ] /sys hierarchy test PASS (or documented limitations)
- [ ] Device node major/minor test PASS
- [ ] DRM devices stub works (doesn't crash)
- [ ] No M6 regressions

## Known Limitations

1. **DRM devices:** Not functional in M6.5. Stubs are just placeholders.

2. **/sys hierarchy:** Kevlar's /sys is minimal. Document which areas are not implemented.

3. **Device permissions:** Might differ from Linux (e.g., /dev/null owner might be root:root vs different). Document divergences.

4. **Hotplug:** /sys/devices might not populate on device hotplug. That's OK for M6.5.

## Contract Documentation

As each contract is validated, add to `docs/contracts.md`:

```markdown
## Subsystem Contracts

### /proc/cpuinfo Format
- ASCII file with processor information (model, flags, etc.)
- Fields separated by colons and tabs
- Tests: testing/contracts/subsystems/proc_cpuinfo.c

### /proc/[pid]/stat Format
- Space-separated fields: pid (comm) state ppid pgrp session tty_nr ...
- Exactly one line per process
- Tests: testing/contracts/subsystems/proc_pid_stat.c

### /proc/[pid]/maps Format
- Address ranges with permissions, offset, device, inode, pathname
- One mapping per line
- Tests: testing/contracts/subsystems/proc_pid_maps.c

### Device Node Major/Minor Numbers
- Character devices: major 1 (mem), minor 3 (/dev/null), 5 (/dev/zero), 9 (/dev/urandom)
- Permissions: 666 (world-readable/writable)
- Tests: testing/contracts/subsystems/dev_permissions.c

### /sys Hierarchy
- /sys/devices, /sys/class, /sys/module, /sys/bus directories exist
- Status: Minimal implementation in Kevlar
- Tests: testing/contracts/subsystems/sys_hierarchy.c

### DRM Devices
- /dev/dri/card*, /dev/dri/renderD* exist (but not functional in M6.5)
- Major 226, minor varies
- Status: Stubs only (M10 requirement)
- Tests: testing/contracts/subsystems/drm_devices.c
```

This spec helps M8-M9 authors understand the /proc, /sys, and device interfaces available for systemd, udev, and other system software.
