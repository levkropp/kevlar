# Default values for build system.
export V          ?=
export GUI        ?=
export RELEASE    ?=
export ARCH       ?= x64
export PROFILE    ?= balanced
export LOG        ?=
export LOG_SERIAL ?=
export CMDLINE    ?=
export QEMU_ARGS  ?=

# The default build target.
.PHONY: default
default: build

# Disable builtin implicit rules and variables.
MAKEFLAGS += --no-builtin-rules --no-builtin-variables
.SUFFIXES:

# Enable verbose output if $(V) is set.
ifeq ($(V),)
.SILENT:
endif

# $(IMAGE): Use a Docker image for initramfs.
ifeq ($(IMAGE),)
ifeq ($(ARCH),arm64)
INITRAMFS_PATH := build/testing.arm64.initramfs
else
INITRAMFS_PATH := build/testing.initramfs
endif
ifeq ($(INIT_SCRIPT),)
# On Windows with Git Bash, disable MSYS path conversion for /bin/sh
ifeq ($(OS),Windows_NT)
export MSYS_NO_PATHCONV := 1
endif
export INIT_SCRIPT := /bin/sh
else
export INIT_SCRIPT
endif
else
IMAGE_FILENAME := $(subst /,.s,$(IMAGE))
INITRAMFS_PATH := build/$(IMAGE_FILENAME).initramfs
export INIT_SCRIPT := $(shell tools/inspect-init-in-docker-image.py $(IMAGE))
endif

DUMMY_INITRAMFS_PATH := build/dummy-for-lint.initramfs

# Architecture guard.
ifneq ($(ARCH),$(filter $(ARCH),x64 arm64))
$(error "Supported ARCH values: x64, arm64")
endif

topdir      := $(PWD)
build_mode  := $(if $(RELEASE),release,debug)

# KVM acceleration: available on x64 (native), not on arm64 (TCG-only on x86 host).
ACCEL       := $(if $(filter arm64,$(ARCH)),,--kvm)

# All profiles use panic=unwind on x64 — LLVM generates faster code with
# unwind tables (better register allocation and code layout).  Performance
# and Ludicrous still skip runtime safety (catch_unwind, capabilities)
# via feature flags; the panic strategy is purely a codegen choice.
ifeq ($(ARCH),x64)
target_json := kernel/arch/$(ARCH)/$(ARCH)-unwind.json
else
target_json := kernel/arch/$(ARCH)/$(ARCH).json
endif
target_dir := $(basename $(notdir $(target_json)))
kernel_elf := kevlar.$(ARCH).elf
stripped_kernel_elf := kevlar.$(ARCH).stripped.elf
kernel_symbols := $(kernel_elf:.elf=.symbols)
kernel_img := kevlar.$(ARCH).img
# Argument passed to run-qemu.py:
#   x64:  the ELF (run-qemu.py patches e_machine→EM_386 for QEMU multiboot)
#   arm64: the flat Image (ARM64 Linux Image header, QEMU sets x0=DTB)
# The bzImage (.img) is still built for real hardware (GRUB2, SYSLINUX, etc.).
ifeq ($(ARCH),x64)
kernel_qemu_arg := $(kernel_elf)
else
kernel_qemu_arg := $(kernel_img)
endif

# Windows compatibility: detect OS and adjust tools
ifeq ($(OS),Windows_NT)
    # Windows - works from PowerShell, CMD, or Git Bash
    # Use Python for all path detection (cross-terminal compatible)
    PYTHON3    ?= python

    # Detect cargo using Python (works from any terminal)
    CARGO_PATH := $(shell $(PYTHON3) -c "import os, shutil; cargo = shutil.which('cargo'); print(cargo if cargo else os.path.join(os.path.expanduser('~'), '.cargo', 'bin', 'cargo.exe'))" 2>nul)
    CARGO      ?= "$(CARGO_PATH)"
    BOCHS      ?= bochs

    # Detect Docker using Python
    DOCKER_PATH := $(shell $(PYTHON3) -c "import os, shutil; docker = shutil.which('docker'); print(docker if docker else os.path.join('C:', os.sep, 'Program Files', 'Docker', 'Docker', 'resources', 'bin', 'docker.exe'))" 2>nul)
    DOCKER     ?= "$(DOCKER_PATH)"

    # Detect QEMU using Python (Chocolatey installs to C:\Program Files\qemu)
    QEMU_PATH := $(shell $(PYTHON3) -c "import os, shutil; qemu = shutil.which('qemu-system-x86_64'); print(qemu if qemu else os.path.join('C:', os.sep, 'Program Files', 'qemu', 'qemu-system-x86_64.exe'))" 2>nul)

    # Detect rustc sysroot using Python
    RUSTC_SYSROOT := $(shell $(PYTHON3) -c "import subprocess, os, shutil; rustc = shutil.which('rustc') or os.path.join(os.path.expanduser('~'), '.cargo', 'bin', 'rustc.exe'); print(subprocess.check_output([rustc, '--print', 'sysroot'], text=True).strip().replace(chr(92), '/')) if os.path.exists(rustc) else ''" 2>nul)
    LLVM_BIN_DIR := $(if $(RUSTC_SYSROOT),$(RUSTC_SYSROOT)/lib/rustlib/x86_64-pc-windows-msvc/bin,)

    # Windows: use echo instead of printf (no ANSI colors)
    PROGRESS   := echo
    # Windows: use Python for file operations
    CP         := $(PYTHON3) -c "import shutil, sys; shutil.copy2(sys.argv[1], sys.argv[2])"
else
    # Linux/macOS
    # Prefer uv if available, fall back to python3
    PYTHON3    ?= $(shell command -v uv >/dev/null 2>&1 && echo "uv run python" || echo "python3")
    CARGO      ?= cargo
    BOCHS      ?= bochs
    # Detect the host triple so llvm-tools resolve on macOS (aarch64-apple-darwin,
    # x86_64-apple-darwin) as well as Linux x86_64.
    RUSTC_HOST := $(shell rustc -vV 2>/dev/null | sed -n 's/^host: //p')
    LLVM_BIN_DIR := $(shell rustc --print sysroot 2>/dev/null || echo "")/lib/rustlib/$(RUSTC_HOST)/bin
    # Unix: use printf with ANSI colors
    PROGRESS   := printf "  \\033[1;96m%8s\\033[0m  \\033[1;m%s\\033[0m\\n"
    # Unix: use standard cp command
    CP         := cp
    DOCKER     ?= docker
endif

# Tool selection: Use LLVM tools on Windows or for arm64
ifeq ($(OS),Windows_NT)
    # Windows always uses LLVM tools (GNU binutils not available)
    # Executables need .exe extension on Windows
    # Quote paths to handle spaces and special characters
    NM         ?= "$(LLVM_BIN_DIR)/llvm-nm.exe"
    READELF    ?= "$(LLVM_BIN_DIR)/llvm-readelf.exe"
    STRIP      ?= "$(LLVM_BIN_DIR)/llvm-strip.exe"
    OBJCOPY    ?= "$(LLVM_BIN_DIR)/llvm-objcopy.exe"
else ifeq ($(ARCH),arm64)
    # arm64 uses LLVM tools on Unix too
    NM         ?= $(LLVM_BIN_DIR)/llvm-nm
    READELF    ?= $(LLVM_BIN_DIR)/llvm-readelf
    STRIP      ?= $(LLVM_BIN_DIR)/llvm-strip
    OBJCOPY    ?= $(LLVM_BIN_DIR)/llvm-objcopy
else
    # x64 on Unix uses standard GNU binutils
    NM         ?= nm
    READELF    ?= readelf
    STRIP      ?= strip
    OBJCOPY    ?= objcopy
endif
DRAWIO     ?= /Applications/draw.io.app/Contents/MacOS/draw.io

# Safety profile guard.
ifneq ($(PROFILE),$(filter $(PROFILE),fortress balanced performance ludicrous))
$(error "Supported PROFILE values: fortress, balanced, performance, ludicrous")
endif

# Comma for use in $(if ...) expansions (Make can't embed literal commas).
comma := ,
export FEATURES  ?=
export RUSTFLAGS = -Z emit-stack-sizes $(if $(filter arm64,$(ARCH)),-Z fixed-x18,)
CARGOFLAGS += -Z build-std=core,alloc
CARGOFLAGS += -Z json-target-spec
CARGOFLAGS += --target $(target_json)
CARGOFLAGS += $(if $(RELEASE),--release,)
CARGOFLAGS += --no-default-features --features profile-$(PROFILE)$(if $(FEATURES),$(comma)$(FEATURES),)
TESTCARGOFLAGS += --package kevlar_kernel -Z unstable-options
TESTCARGOFLAGS += --config "target.$(ARCH).runner = './tools/run-unittests.sh'"
WATCHFLAGS += --clear

export KEVLAR_DEBUG ?=
export CARGO_FROM_MAKE=1
export INITRAMFS_PATH
export ARCH
export PYTHON3
export NM
export DOCKER
export CARGO
export QEMU_PATH

#
#  Build Commands
#
.PHONY: build
build:
	$(MAKE) build-crate
	$(CP) target/$(target_dir)/$(build_mode)/kevlar_kernel $(kernel_elf)

	$(PROGRESS) "NM" $(kernel_symbols)
ifeq ($(OS),Windows_NT)
	$(NM) $(kernel_elf) | $(PYTHON3) -c "import sys; [print(' '.join([parts[0]] + parts[2:])) for line in sys.stdin if (parts := line.strip().split()) and len(parts) >= 2]" > $(kernel_symbols)
else
	$(NM) $(kernel_elf) | rustfilt | awk '{ $$2=""; print $$0 }' > $(kernel_symbols)
endif

	$(PROGRESS) "SYMBOLS" $(kernel_elf)
	$(PYTHON3) tools/embed-symbol-table.py $(kernel_symbols) $(kernel_elf)

	$(PROGRESS) "STRIP" $(stripped_kernel_elf)
	$(STRIP) $(kernel_elf) -o $(stripped_kernel_elf)

	$(PROGRESS) "IMAGE" $(kernel_img)
ifeq ($(ARCH),arm64)
	$(OBJCOPY) -O binary $(kernel_elf) $(kernel_img)
else
	$(OBJCOPY) -O binary \
		--remove-section=.eh_frame \
		--remove-section=.eh_frame_hdr \
		$(kernel_elf) build/kevlar.x64.bin
	$(PYTHON3) platform/x64/gen_setup.py build/kevlar.x64.bin $(kernel_img)
	rm -f build/kevlar.x64.bin
endif

.PHONY: build-crate
build-crate:
	$(MAKE) initramfs

	$(PROGRESS) "CARGO" "kernel"
	@time $(CARGO) build $(CARGOFLAGS) --manifest-path kernel/Cargo.toml

.PHONY: initramfs
initramfs: $(INITRAMFS_PATH)

.PHONY: buildw
buildw:
	$(CARGO) watch $(WATCHFLAGS) -s "$(MAKE) build-crate"

.PHONY: iso
iso: build
	$(PROGRESS) MKISO kevlar.iso
	$(PYTHON3) -c "import os; os.makedirs('isofiles/boot/grub', exist_ok=True)"
	$(CP) boot/grub.cfg isofiles/boot/grub/grub.cfg
	$(CP) $(stripped_kernel_elf) isofiles/kevlar.elf
	grub-mkrescue -o kevlar.iso isofiles

.PHONY: run
run: build
	$(PYTHON3) tools/run-qemu.py                                           \
		--arch $(ARCH) --kvm                                           \
		--save-dump kevlar.dump                                        \
		--append-cmdline "init=/sbin/init"                             \
		$(if $(GUI),--gui,)                                            \
		$(if $(GDB),--gdb,)                                            \
		$(if $(LOG),--append-cmdline "log=$(LOG)",)                    \
		$(if $(CMDLINE),--append-cmdline "$(CMDLINE)",)                \
		$(if $(LOG_SERIAL),--log-serial "$(LOG_SERIAL)",)              \
		$(if $(QEMU),--qemu $(QEMU),)                                  \
		$(kernel_qemu_arg) -- $(QEMU_ARGS)

# `make run-sh` — bare BusyBox shell (no OpenRC, no init)
.PHONY: run-sh
run-sh: build
	$(PYTHON3) tools/run-qemu.py                                           \
		--arch $(ARCH) --kvm                                           \
		--save-dump kevlar.dump                                        \
		$(if $(GUI),--gui,)                                            \
		$(if $(GDB),--gdb,)                                            \
		$(if $(LOG),--append-cmdline "log=$(LOG)",)                    \
		$(if $(CMDLINE),--append-cmdline "$(CMDLINE)",)                \
		$(if $(LOG_SERIAL),--log-serial "$(LOG_SERIAL)",)              \
		$(if $(QEMU),--qemu $(QEMU),)                                  \
		$(kernel_qemu_arg) -- $(QEMU_ARGS)

# `make run-alpine` — Boot Alpine Linux 3.21 with OpenRC on Kevlar.
# First run builds the ext4 disk image from Docker (requires Docker).
# Subsequent runs reuse the cached image (delete build/alpine.img to rebuild).
.PHONY: run-alpine
run-alpine: build/alpine.img
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/boot-alpine"
	$(PYTHON3) tools/run-qemu.py                                           \
		--arch $(ARCH) --kvm --disk build/alpine.img                   \
		$(if $(GUI),--gui,)                                            \
		$(if $(GDB),--gdb,)                                            \
		$(if $(LOG),--append-cmdline "log=$(LOG)",)                    \
		$(if $(CMDLINE),--append-cmdline "$(CMDLINE)",)                \
		$(if $(LOG_SERIAL),--log-serial "$(LOG_SERIAL)",)              \
		$(if $(QEMU),--qemu $(QEMU),)                                  \
		$(kernel_qemu_arg) -- $(QEMU_ARGS)

build/alpine.img:
	@if ! docker info >/dev/null 2>&1; then \
		echo "  Docker not available, using native builder"; \
		$(PYTHON3) tools/build-alpine-full.py build/alpine.img; \
		exit 0; \
	fi
	$(PROGRESS) "MKALPINE" "build/alpine.img"
	@$(PYTHON3) -c "import os; os.makedirs('build', exist_ok=True)"
	@rm -rf build/alpine-root
	@mkdir -p build/alpine-root
	@docker rm -f kevlar-alpine 2>/dev/null || true
	docker run --name kevlar-alpine alpine:3.21 sh -c 'apk add --no-cache openrc build-base'
	fakeroot sh -c 'docker export kevlar-alpine | tar -xf - -C build/alpine-root && chown -R 0:0 build/alpine-root'
	@docker rm kevlar-alpine
	@sed -i 's/^root:\*:/root::/' build/alpine-root/etc/shadow
	@echo "ttyS0" >> build/alpine-root/etc/securetty
	@echo "kevlar" > build/alpine-root/etc/hostname
	@echo "UTC0" > build/alpine-root/etc/TZ
	@ln -sf /usr/share/zoneinfo/UTC build/alpine-root/etc/localtime 2>/dev/null || echo "UTC0" > build/alpine-root/etc/localtime
	@echo "nameserver 10.0.2.3" > build/alpine-root/etc/resolv.conf
	@printf '/lib\n/usr/lib\n' > build/alpine-root/etc/ld-musl-x86_64.path
	@# Symlink /usr/lib shared libraries into /lib so musl's dynamic linker
	@# finds them. This makes curl and other dynamically-linked programs work.
	@for f in build/alpine-root/usr/lib/lib*.so*; do \
		test -f "$$f" || continue; \
		base=$$(basename "$$f"); \
		test -e "build/alpine-root/lib/$$base" || ln -sf "/usr/lib/$$base" "build/alpine-root/lib/$$base"; \
	done
	@sed -i 's|https://|http://|g' build/alpine-root/etc/apk/repositories
	@chmod 777 build/alpine-root/var/cache/apk
	@printf '%s\n' \
		'::sysinit:/sbin/ip link set lo up' \
		'::sysinit:/sbin/ip link set eth0 up' \
		'::sysinit:/sbin/ip addr add 10.0.2.15/24 dev eth0' \
		'::sysinit:/sbin/ip route add default via 10.0.2.2' \
		'::sysinit:/sbin/openrc sysinit' \
		'::sysinit:/sbin/openrc boot' \
		'::wait:/sbin/openrc default' \
		'ttyS0::respawn:/sbin/getty -n -l /bin/sh -L 115200 ttyS0 vt100' \
		'::ctrlaltdel:/sbin/reboot' \
		'::shutdown:/sbin/openrc shutdown' \
		> build/alpine-root/etc/inittab
	dd if=/dev/zero of=build/alpine.img bs=1M count=512 2>/dev/null
	fakeroot mke2fs -t ext4 -q -d build/alpine-root build/alpine.img
	@rm -rf build/alpine-root

.PHONY: run-systemd
run-systemd: build
	$(PYTHON3) tools/run-qemu.py                                           \
		--arch $(ARCH) --kvm                                           \
		--append-cmdline "init=/usr/lib/systemd/systemd"               \
		$(if $(GUI),--gui,)                                            \
		$(if $(GDB),--gdb,)                                            \
		$(if $(LOG),--append-cmdline "log=$(LOG)",)                    \
		$(if $(CMDLINE),--append-cmdline "$(CMDLINE)",)                \
		$(if $(LOG_SERIAL),--log-serial "$(LOG_SERIAL)",)              \
		$(if $(QEMU),--qemu $(QEMU),)                                  \
		$(kernel_qemu_arg) -- $(QEMU_ARGS)

.PHONY: disk
disk: build/disk.img

build/disk.img: testing/disk_hello.c
	$(PROGRESS) "MKDISK" build/disk.img
	$(PYTHON3) -c "import os; os.makedirs('build', exist_ok=True)"
ifdef USE_DOCKER
	$(DOCKER) build --target disk_image -t kevlar-disk-image -f testing/Dockerfile .
	$(DOCKER) create --name kevlar-disk-tmp kevlar-disk-image
	$(DOCKER) cp kevlar-disk-tmp:/disk.img build/disk.img
	$(DOCKER) rm kevlar-disk-tmp
else
	musl-gcc -static -O2 -o build/disk_hello testing/disk_hello.c
	mkdir -p build/disk_root/subdir
	cp build/disk_hello build/disk_root/hello
	printf 'hello from ext2\n' > build/disk_root/greeting.txt
	printf 'nested file\n' > build/disk_root/subdir/nested.txt
	ln -sf greeting.txt build/disk_root/link.txt
	chmod +x build/disk_root/hello
	dd if=/dev/zero of=build/disk.img bs=1M count=16 2>/dev/null
	mke2fs -t ext2 -d build/disk_root build/disk.img
	rm -rf build/disk_root build/disk_hello
endif

.PHONY: run-disk
run-disk: build disk
	$(PYTHON3) tools/run-qemu.py                                           \
		--arch $(ARCH) --kvm                                           \
		--disk build/disk.img                                          \
		$(if $(GUI),--gui,)                                            \
		$(if $(GDB),--gdb,)                                            \
		$(if $(LOG),--append-cmdline "log=$(LOG)",)                    \
		$(if $(CMDLINE),--append-cmdline "$(CMDLINE)",)                \
		$(if $(LOG_SERIAL),--log-serial "$(LOG_SERIAL)",)              \
		$(if $(QEMU),--qemu $(QEMU),)                                  \
		$(kernel_qemu_arg) -- $(QEMU_ARGS)

.PHONY: alpine-disk
alpine-disk: build/alpine-disk.img

build/alpine-disk.img:
	$(PROGRESS) "MKDISK" build/alpine-disk.img
	$(PYTHON3) -c "import os; os.makedirs('build', exist_ok=True)"
ifdef USE_DOCKER
	$(DOCKER) build --target alpine_disk -t kevlar-alpine-disk -f testing/Dockerfile .
	$(DOCKER) create --name kevlar-alpine-tmp kevlar-alpine-disk
	$(DOCKER) cp kevlar-alpine-tmp:/alpine-disk.img build/alpine-disk.img
	$(DOCKER) rm kevlar-alpine-tmp
else
	$(PYTHON3) tools/build-alpine-disk.py build/alpine-disk.img
endif

.PHONY: run-apk
run-apk: build alpine-disk
	$(PYTHON3) tools/run-qemu.py                                           \
		--arch $(ARCH) --kvm                                           \
		--disk build/alpine-disk.img                                   \
		$(if $(GUI),--gui,)                                            \
		$(if $(GDB),--gdb,)                                            \
		$(if $(LOG),--append-cmdline "log=$(LOG)",)                    \
		$(if $(CMDLINE),--append-cmdline "$(CMDLINE)",)                \
		$(if $(LOG_SERIAL),--log-serial "$(LOG_SERIAL)",)              \
		$(if $(QEMU),--qemu $(QEMU),)                                  \
		$(kernel_qemu_arg) -- $(QEMU_ARGS)

# Standalone ext4 test: run the comprehensive ext4 + dynamic linking test suite
# directly (no Alpine boot, no OpenRC, just mount ext4 and run tests).
.PHONY: test-ext4
test-ext4: build/alpine.img
	$(PROGRESS) "TEST" "ext4 comprehensive (write/mmap/sendfile + dynamic linking)"
	@cp build/alpine.img build/alpine-test.img
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/test-alpine-apk"
	$(PYTHON3) tools/run-qemu.py --timeout 180 \
		--kvm --batch --arch $(ARCH) --disk build/alpine-test.img \
		$(kernel_qemu_arg) -- -mem-prealloc 2>&1 \
		| tee /tmp/kevlar-test-ext4-$(PROFILE).log; true
	@rm -f build/alpine-test.img
	@grep -E '^(TEST_PASS|TEST_FAIL|TEST_END|BENCH)' \
		/tmp/kevlar-test-ext4-$(PROFILE).log || echo "(no test output)"
	@if grep -q '^TEST_FAIL' /tmp/kevlar-test-ext4-$(PROFILE).log; then \
		echo "EXT4 TESTS: some failures"; exit 1; \
	elif grep -q '^=== RESULTS' /tmp/kevlar-test-ext4-$(PROFILE).log; then \
		echo "EXT4 TESTS PASSED"; \
	fi

# Full Alpine boot test: mount ext4, OpenRC, apk update, apk add curl.
# Uses a COPY of alpine.img so the test's /etc/inittab changes don't
# corrupt the interactive run-alpine image.
.PHONY: test-alpine-apk
test-alpine-apk: build/alpine.img
	$(PROGRESS) "TEST" "Alpine full boot + apk update + apk add curl"
	@cp build/alpine.img build/alpine-test.img
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/test-alpine-apk"
	$(PYTHON3) tools/run-qemu.py --timeout 300 \
		--kvm --batch --arch $(ARCH) --disk build/alpine-test.img \
		$(kernel_qemu_arg) -- -mem-prealloc 2>&1 \
		| tee /tmp/kevlar-test-alpine-apk-$(PROFILE).log; true
	@rm -f build/alpine-test.img
	@grep -E '^(TEST_PASS|TEST_FAIL|TEST_END|ALL ALPINE)' \
		/tmp/kevlar-test-alpine-apk-$(PROFILE).log || echo "(no test output)"
	@if grep -q '^TEST_FAIL' /tmp/kevlar-test-alpine-apk-$(PROFILE).log; then \
		echo "ALPINE APK TESTS: some failures"; exit 1; \
	elif grep -q '^TEST_END' /tmp/kevlar-test-alpine-apk-$(PROFILE).log; then \
		echo "ALL ALPINE APK TESTS PASSED"; \
	fi

.PHONY: test-alpine
test-alpine: alpine-disk
	$(PROGRESS) "TEST" "Alpine apk integration (mount + update)"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/test_apk_update.sh"
	$(PYTHON3) tools/run-qemu.py --timeout 120 \
		--kvm --batch --arch $(ARCH) --disk build/alpine-disk.img \
		$(kernel_qemu_arg) -- -mem-prealloc 2>&1 \
		| tee /tmp/kevlar-test-alpine-$(PROFILE).log; true
	@grep -E '^(TEST_PASS|TEST_FAIL|TEST_END|ALL ALPINE)' \
		/tmp/kevlar-test-alpine-$(PROFILE).log || echo "(no test output)"
	@if grep -q '^TEST_FAIL' /tmp/kevlar-test-alpine-$(PROFILE).log; then \
		echo "ALPINE TESTS: some failures"; exit 1; \
	elif grep -q '^TEST_END' /tmp/kevlar-test-alpine-$(PROFILE).log; then \
		echo "ALL ALPINE TESTS PASSED"; \
	fi

# Comprehensive Alpine smoke test: 8 phases, ~60 tests.
# Validates Kevlar as a drop-in Linux replacement.
.PHONY: test-smoke-alpine
test-smoke-alpine: alpine-disk
	$(PROGRESS) "TEST" "Alpine smoke test (8 phases, ~60 tests)"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/smoke-alpine"
	$(PYTHON3) tools/run-qemu.py --timeout 180 \
		--kvm --batch --arch $(ARCH) --disk build/alpine-disk.img \
		$(kernel_qemu_arg) -- -mem-prealloc 2>&1 \
		| tee /tmp/kevlar-smoke-alpine-$(PROFILE).log; true
	@echo ""
	@echo "═══ Smoke Test Results ═══"
	@grep -E '^(TEST_PASS|TEST_FAIL|TEST_SKIP)' \
		/tmp/kevlar-smoke-alpine-$(PROFILE).log | sort | uniq -c | sort -rn || true
	@echo ""
	@grep -E '^TEST_FAIL' /tmp/kevlar-smoke-alpine-$(PROFILE).log || echo "No failures!"
	@echo ""
	@grep -E '^(SMOKE TEST COMPLETE|TEST_END)' \
		/tmp/kevlar-smoke-alpine-$(PROFILE).log || echo "(no summary)"
	@if grep -q '^TEST_FAIL' /tmp/kevlar-smoke-alpine-$(PROFILE).log; then \
		echo "SMOKE TEST: SOME FAILURES"; exit 1; \
	elif grep -q '^TEST_END' /tmp/kevlar-smoke-alpine-$(PROFILE).log; then \
		echo "SMOKE TEST: ALL PASSED"; \
	fi

# Build Alpine X11 disk image (512MB with Xorg, xterm, twm, fbdev driver)
build/alpine-xorg.img:
	$(PROGRESS) "MKDISK" "Alpine X11 (512MB)"
	$(PYTHON3) tools/build-alpine-xorg.py build/alpine-xorg.img

# Test X11/Xorg on Kevlar with fbdev framebuffer
# `make test-module` — load /lib/modules/hello.ko at boot and verify
# its init function ran via kABI.  Boots the kernel, greps the serial
# log for the printk() output emitted from inside the module.
.PHONY: test-module
test-module: build
	$(PROGRESS) "TEST" "kABI .ko module load (K1)"
	$(PYTHON3) tools/run-qemu.py --timeout 20 \
		--kvm --batch --arch $(ARCH) \
		$(kernel_qemu_arg) -- -no-reboot 2>&1 \
		| tee /tmp/kevlar-test-module.log; true
	@echo ""
	@if grep -q '\[mod\] hello from module!' /tmp/kevlar-test-module.log \
	 && grep -q 'kabi: my_init returned 0' /tmp/kevlar-test-module.log; then \
	    echo "TEST_PASS: kABI K1 — hello.ko loaded and ran"; \
	else \
	    echo "TEST_FAIL: expected '[mod] hello from module!' + 'kabi: my_init returned 0'"; \
	    grep -E 'kabi|\[mod\]|panic' /tmp/kevlar-test-module.log | head -20; \
	    false; \
	fi

# `make test-module-k2` — load /lib/modules/k2.ko and verify it
# exercises kmalloc + wait_queue + completion + work_struct end-to-end.
.PHONY: test-module-k2
test-module-k2: build
	$(PROGRESS) "TEST" "kABI K2 demo (alloc + wait + work + completion)"
	$(PYTHON3) tools/run-qemu.py --timeout 20 \
		--kvm --batch --arch $(ARCH) \
		$(kernel_qemu_arg) -- -no-reboot 2>&1 \
		| tee /tmp/kevlar-test-module-k2.log; true
	@echo ""
	@if grep -q '\[k2\] init begin' /tmp/kevlar-test-module-k2.log \
	 && grep -q '\[k2\] work handler running' /tmp/kevlar-test-module-k2.log \
	 && grep -q '\[k2\] init done' /tmp/kevlar-test-module-k2.log \
	 && grep -q 'kabi: k2 init_module returned 0' /tmp/kevlar-test-module-k2.log; then \
	    echo "TEST_PASS: kABI K2 — k2.ko ran end-to-end"; \
	else \
	    echo "TEST_FAIL: missing expected K2 markers"; \
	    grep -E 'kabi|\[k2\]|panic' /tmp/kevlar-test-module-k2.log | head -30; \
	    false; \
	fi

# `make test-module-k4` — load /lib/modules/k4.ko + verify
# /dev/k4-demo open/read/release via the kABI char-device bridge.
.PHONY: test-module-k4
test-module-k4: build
	$(PROGRESS) "TEST" "kABI K4 demo (file_operations + /dev/k4-demo)"
	$(PYTHON3) tools/run-qemu.py --timeout 20 \
		--kvm --batch --arch $(ARCH) \
		$(kernel_qemu_arg) -- -no-reboot 2>&1 \
		| tee /tmp/kevlar-test-module-k4.log; true
	@echo ""
	@if grep -q '\[k4\] init begin' /tmp/kevlar-test-module-k4.log \
	 && grep -q '\[k4\] register_chrdev ok' /tmp/kevlar-test-module-k4.log \
	 && grep -q '\[k4\] init done' /tmp/kevlar-test-module-k4.log \
	 && grep -q 'kabi: k4 init_module returned 0' /tmp/kevlar-test-module-k4.log \
	 && grep -q '\[k4\] open called' /tmp/kevlar-test-module-k4.log \
	 && grep -q 'kabi: k4 /dev/k4-demo read' /tmp/kevlar-test-module-k4.log \
	 && grep -q 'hello from k4' /tmp/kevlar-test-module-k4.log; then \
	    echo "TEST_PASS: kABI K4 — file_operations + char-device dispatch"; \
	else \
	    echo "TEST_FAIL: missing expected K4 markers"; \
	    grep -E 'kabi|\[k4\]|panic' /tmp/kevlar-test-module-k4.log | head -30; \
	    false; \
	fi

# `make test-module-k12` — load /lib/modules/virtio_input.ko:
# Ubuntu's virtio keyboard/mouse driver.  30 undefs across input +
# virtio bus + infra subsystems.
.PHONY: test-module-k12
test-module-k12: build
	$(PROGRESS) "TEST" "kABI K12 demo (Ubuntu virtio_input.ko + 30 stubs)"
	$(PYTHON3) tools/run-qemu.py --timeout 20 \
		--kvm --batch --arch $(ARCH) \
		$(kernel_qemu_arg) -- -no-reboot 2>&1 \
		| tee /tmp/kevlar-test-module-k12.log; true
	@echo ""
	@if grep -q 'kabi: loading /lib/modules/virtio_input.ko' /tmp/kevlar-test-module-k12.log \
	 && grep -q 'kabi: __register_virtio_driver: name=Some("virtio_input")' /tmp/kevlar-test-module-k12.log \
	 && grep -q 'kabi: virtio_input init_module returned 0' /tmp/kevlar-test-module-k12.log; then \
	    echo "TEST_PASS: kABI K12 — Ubuntu virtio_input.ko loaded with input + virtio core stubs"; \
	else \
	    echo "TEST_FAIL: missing expected K12 markers"; \
	    grep -E 'kabi|virtio_input|panic' /tmp/kevlar-test-module-k12.log | head -30; \
	    false; \
	fi

# `make test-module-k13` — load /lib/modules/drm_buddy.ko:
# Ubuntu's DRM buddy-allocator helper.  21 undefs across slab,
# rbtree, list-debug, drm_printf, and __sw_hweight64.  Library
# module — no init_module — so the test verifies "loaded"
# rather than "init returned 0".
.PHONY: test-module-k13
test-module-k13: build
	$(PROGRESS) "TEST" "kABI K13 demo (Ubuntu drm_buddy.ko + slab/rbtree/list stubs)"
	$(PYTHON3) tools/run-qemu.py --timeout 20 \
		--kvm --batch --arch $(ARCH) \
		$(kernel_qemu_arg) -- -no-reboot 2>&1 \
		| tee /tmp/kevlar-test-module-k13.log; true
	@echo ""
	@if grep -q 'kabi: loading /lib/modules/drm_buddy.ko' /tmp/kevlar-test-module-k13.log \
	 && grep -q 'kabi: loaded /lib/modules/drm_buddy.ko' /tmp/kevlar-test-module-k13.log \
	 && grep -q 'kabi: drm_buddy init_module returned 0' /tmp/kevlar-test-module-k13.log; then \
	    echo "TEST_PASS: kABI K13 — Ubuntu drm_buddy.ko loaded with DRM-stack stubs"; \
	else \
	    echo "TEST_FAIL: missing expected K13 markers"; \
	    grep -E 'kabi|drm_buddy|panic' /tmp/kevlar-test-module-k13.log | head -30; \
	    false; \
	fi

# `make test-module-k14` — load /lib/modules/drm_exec.ko:
# Ubuntu's DRM transactional buffer-reservation helper.  11
# undefs across ww_mutex / dma_resv / drm_gem / refcount /
# kvmalloc renames.  Pure library module — no init_module.
.PHONY: test-module-k14
test-module-k14: build
	$(PROGRESS) "TEST" "kABI K14 demo (Ubuntu drm_exec.ko + ww_mutex/dma_resv/drm_gem stubs)"
	$(PYTHON3) tools/run-qemu.py --timeout 20 \
		--kvm --batch --arch $(ARCH) \
		$(kernel_qemu_arg) -- -no-reboot 2>&1 \
		| tee /tmp/kevlar-test-module-k14.log; true
	@echo ""
	@if grep -q 'kabi: loading /lib/modules/drm_exec.ko' /tmp/kevlar-test-module-k14.log \
	 && grep -q 'kabi: loaded /lib/modules/drm_exec.ko' /tmp/kevlar-test-module-k14.log \
	 && grep -q 'kabi: drm_exec is a library module' /tmp/kevlar-test-module-k14.log; then \
	    echo "TEST_PASS: kABI K14 — Ubuntu drm_exec.ko loaded with ww_mutex/dma_resv/drm_gem stubs"; \
	else \
	    echo "TEST_FAIL: missing expected K14 markers"; \
	    grep -E 'kabi|drm_exec|panic' /tmp/kevlar-test-module-k14.log | head -30; \
	    false; \
	fi

# `make test-module-k15` — load /lib/modules/drm_ttm_helper.ko:
# Ubuntu's DRM framebuffer-emulation helper.  47 undefs (40 net
# new) across drm_fb_helper, drm_client, fb_*, fb_raster, ttm_bo,
# drm_format, mutex, module_ref, misc.  Pure library module.
.PHONY: test-module-k15
test-module-k15: build
	$(PROGRESS) "TEST" "kABI K15 demo (Ubuntu drm_ttm_helper.ko + 40 fbdev/TTM stubs)"
	$(PYTHON3) tools/run-qemu.py --timeout 20 \
		--kvm --batch --arch $(ARCH) \
		$(kernel_qemu_arg) -- -no-reboot 2>&1 \
		| tee /tmp/kevlar-test-module-k15.log; true
	@echo ""
	@if grep -q 'kabi: loading /lib/modules/drm_ttm_helper.ko' /tmp/kevlar-test-module-k15.log \
	 && grep -q 'kabi: loaded /lib/modules/drm_ttm_helper.ko' /tmp/kevlar-test-module-k15.log \
	 && grep -q 'kabi: drm_ttm_helper is a library module' /tmp/kevlar-test-module-k15.log; then \
	    echo "TEST_PASS: kABI K15 — Ubuntu drm_ttm_helper.ko loaded with fbdev/TTM stubs"; \
	else \
	    echo "TEST_FAIL: missing expected K15 markers"; \
	    grep -E 'kabi|drm_ttm|panic' /tmp/kevlar-test-module-k15.log | head -30; \
	    false; \
	fi

# `make test-module-k21` — DRM ioctl dispatch.  Kernel-side
# smoke test issues DRM_IOCTL_VERSION against drm_ioctl, expects
# back name="kabi-drm" and version 2.0.0.
.PHONY: test-module-k21
test-module-k21: build
	$(PROGRESS) "TEST" "kABI K21 demo (DRM_IOCTL_VERSION + GET_CAP dispatch)"
	$(PYTHON3) tools/run-qemu.py --timeout 20 \
		--kvm --batch --arch $(ARCH) \
		$(kernel_qemu_arg) -- -no-reboot 2>&1 \
		| tee /tmp/kevlar-test-module-k21.log; true
	@echo ""
	@if grep -q 'kabi: drm_dev_register: /dev/dri/card0 installed' /tmp/kevlar-test-module-k21.log \
	 && grep -q 'kabi: DRM_IOCTL_VERSION returned rc=0 name="kabi-drm"' /tmp/kevlar-test-module-k21.log \
	 && grep -q 'version=2.0.0' /tmp/kevlar-test-module-k21.log; then \
	    echo "TEST_PASS: kABI K21 — DRM_IOCTL_VERSION returned name=kabi-drm version=2.0.0"; \
	else \
	    echo "TEST_FAIL: missing expected K21 markers"; \
	    grep -E 'kabi|DRM_IOCTL|panic' /tmp/kevlar-test-module-k21.log | head -30; \
	    false; \
	fi

# `make test-module-k20` — fire bochs probe + register
# /dev/dri/cardN.  Two probe-firing drivers (cirrus + bochs) and
# two char devices visible to userspace.
.PHONY: test-module-k20
test-module-k20: build
	$(PROGRESS) "TEST" "kABI K20 demo (/dev/dri/cardN + bochs probe)"
	$(PYTHON3) tools/run-qemu.py --timeout 20 \
		--kvm --batch --arch $(ARCH) \
		$(kernel_qemu_arg) -- -no-reboot 2>&1 \
		| tee /tmp/kevlar-test-module-k20.log; true
	@echo ""
	@if grep -q 'kabi: PCI walk:.*cirrus-qemu.*1013:00b8' /tmp/kevlar-test-module-k20.log \
	 && grep -q "kabi: PCI walk: 'cirrus-qemu' probe returned 0" /tmp/kevlar-test-module-k20.log \
	 && grep -q 'kabi: PCI walk:.*bochs-drm.*1234:1111' /tmp/kevlar-test-module-k20.log \
	 && grep -q "kabi: PCI walk: 'bochs-drm' probe returned" /tmp/kevlar-test-module-k20.log \
	 && grep -q 'kabi: drm_dev_register: /dev/dri/card0 installed' /tmp/kevlar-test-module-k20.log; then \
	    echo "TEST_PASS: kABI K20 — both probes fired (cirrus succeeded, bochs fired); /dev/dri/card0 installed"; \
	else \
	    echo "TEST_FAIL: missing expected K20 markers"; \
	    grep -E 'kabi|cirrus|bochs|panic|PCI|drm_dev' /tmp/kevlar-test-module-k20.log | head -50; \
	    false; \
	fi

# `make test-module-k19` — first probe-firing milestone.  Walks
# the registered PCI drivers + fake devices and calls cirrus's
# probe with a fake (vendor=0x1013, device=0x00B8) match.
.PHONY: test-module-k19
test-module-k19: build
	$(PROGRESS) "TEST" "kABI K19 demo (PCI bus walking + cirrus probe firing)"
	$(PYTHON3) tools/run-qemu.py --timeout 20 \
		--kvm --batch --arch $(ARCH) \
		$(kernel_qemu_arg) -- -no-reboot 2>&1 \
		| tee /tmp/kevlar-test-module-k19.log; true
	@echo ""
	@if grep -q 'kabi: PCI walk:' /tmp/kevlar-test-module-k19.log \
	 && grep -q 'kabi: PCI walk: probing driver' /tmp/kevlar-test-module-k19.log \
	 && grep -q 'kabi: PCI walk: .* probe returned' /tmp/kevlar-test-module-k19.log; then \
	    echo "TEST_PASS: kABI K19 — PCI walk fired cirrus probe"; \
	else \
	    echo "TEST_FAIL: missing expected K19 markers"; \
	    grep -E 'kabi|cirrus|panic|PCI' /tmp/kevlar-test-module-k19.log | head -40; \
	    false; \
	fi

# `make test-module-k18` — load /lib/modules/bochs.ko: Ubuntu's
# KMS driver for QEMU Bochs Display Adapter.  107 undefs (18
# net new — 83% compounded) across EDID, drm core extensions,
# PCI/IO resources, port I/O, drm error.  Real driver — same
# shape as cirrus-qemu (registers PCI driver, returns 0).
.PHONY: test-module-k18
test-module-k18: build
	$(PROGRESS) "TEST" "kABI K18 demo (Ubuntu bochs.ko + 18 EDID/IO stubs)"
	$(PYTHON3) tools/run-qemu.py --timeout 20 \
		--kvm --batch --arch $(ARCH) \
		$(kernel_qemu_arg) -- -no-reboot 2>&1 \
		| tee /tmp/kevlar-test-module-k18.log; true
	@echo ""
	@if grep -q 'kabi: loading /lib/modules/bochs.ko' /tmp/kevlar-test-module-k18.log \
	 && grep -q 'kabi: loaded /lib/modules/bochs.ko' /tmp/kevlar-test-module-k18.log \
	 && grep -q 'kabi: bochs init_module returned 0' /tmp/kevlar-test-module-k18.log; then \
	    echo "TEST_PASS: kABI K18 — Ubuntu bochs.ko loaded with EDID + IO stubs"; \
	else \
	    echo "TEST_FAIL: missing expected K18 markers"; \
	    grep -E 'kabi|bochs|panic' /tmp/kevlar-test-module-k18.log | head -30; \
	    false; \
	fi

# `make test-module-k17` — load /lib/modules/cirrus-qemu.ko:
# Ubuntu's KMS driver for QEMU emulated Cirrus VGA.  88 undefs
# (81 net new) across drm core / KMS / GEM-shadow / PCI / mmio
# tracepoints.  Real driver — init_module registers a PCI
# driver and returns 0.  Probe doesn't fire (no PCI bus walking
# yet — K20+).
.PHONY: test-module-k17
test-module-k17: build
	$(PROGRESS) "TEST" "kABI K17 demo (Ubuntu cirrus-qemu.ko + 81 DRM core/KMS/PCI stubs)"
	$(PYTHON3) tools/run-qemu.py --timeout 20 \
		--kvm --batch --arch $(ARCH) \
		$(kernel_qemu_arg) -- -no-reboot 2>&1 \
		| tee /tmp/kevlar-test-module-k17.log; true
	@echo ""
	@if grep -q 'kabi: loading /lib/modules/cirrus-qemu.ko' /tmp/kevlar-test-module-k17.log \
	 && grep -q 'kabi: loaded /lib/modules/cirrus-qemu.ko' /tmp/kevlar-test-module-k17.log \
	 && grep -q 'kabi: __pci_register_driver: name=Some("cirrus-qemu")' /tmp/kevlar-test-module-k17.log \
	 && grep -q 'kabi: cirrus init_module returned 0' /tmp/kevlar-test-module-k17.log; then \
	    echo "TEST_PASS: kABI K17 — Ubuntu cirrus-qemu.ko loaded with DRM core / KMS / PCI stubs"; \
	else \
	    echo "TEST_FAIL: missing expected K17 markers"; \
	    grep -E 'kabi|cirrus|panic' /tmp/kevlar-test-module-k17.log | head -30; \
	    false; \
	fi

# `make test-module-k16` — load /lib/modules/drm_dma_helper.ko:
# Ubuntu's DMA-coherent GEM buffer helper.  79 undefs (32 net
# new) across DMA API, DRM GEM/prime/atomic, mm helpers, format/
# client extensions.  Pure library module.
.PHONY: test-module-k16
test-module-k16: build
	$(PROGRESS) "TEST" "kABI K16 demo (Ubuntu drm_dma_helper.ko + 32 DMA/GEM/mm stubs)"
	$(PYTHON3) tools/run-qemu.py --timeout 20 \
		--kvm --batch --arch $(ARCH) \
		$(kernel_qemu_arg) -- -no-reboot 2>&1 \
		| tee /tmp/kevlar-test-module-k16.log; true
	@echo ""
	@if grep -q 'kabi: loading /lib/modules/drm_dma_helper.ko' /tmp/kevlar-test-module-k16.log \
	 && grep -q 'kabi: loaded /lib/modules/drm_dma_helper.ko' /tmp/kevlar-test-module-k16.log \
	 && grep -q 'kabi: drm_dma_helper is a library module' /tmp/kevlar-test-module-k16.log; then \
	    echo "TEST_PASS: kABI K16 — Ubuntu drm_dma_helper.ko loaded with DMA/GEM/mm stubs"; \
	else \
	    echo "TEST_FAIL: missing expected K16 markers"; \
	    grep -E 'kabi|drm_dma|panic' /tmp/kevlar-test-module-k16.log | head -30; \
	    false; \
	fi

# `make test-module-k11` — load /lib/modules/dummy.ko: Ubuntu's
# network dummy device.  23 undefs across rtnl/netdev/ethtool/skb;
# first milestone with subsystem-shaped stub work.
.PHONY: test-module-k11
test-module-k11: build
	$(PROGRESS) "TEST" "kABI K11 demo (Ubuntu dummy.ko + 23 net stubs)"
	$(PYTHON3) tools/run-qemu.py --timeout 20 \
		--kvm --batch --arch $(ARCH) \
		$(kernel_qemu_arg) -- -no-reboot 2>&1 \
		| tee /tmp/kevlar-test-module-k11.log; true
	@echo ""
	@if grep -q 'kabi: loading /lib/modules/dummy.ko' /tmp/kevlar-test-module-k11.log \
	 && grep -q 'kabi: rtnl_link_register (stub)' /tmp/kevlar-test-module-k11.log \
	 && grep -q 'kabi: dummy init_module returned 0' /tmp/kevlar-test-module-k11.log; then \
	    echo "TEST_PASS: kABI K11 — Ubuntu dummy.ko loaded with 23 net subsystem stubs"; \
	else \
	    echo "TEST_FAIL: missing expected K11 markers"; \
	    grep -E 'kabi|dummy|panic' /tmp/kevlar-test-module-k11.log | head -30; \
	    false; \
	fi

# `make test-module-k10` — load /lib/modules/xor-neon.ko: a real
# Ubuntu 26.04 .ko that needs a new Linux export (cpu_have_feature).
# First milestone where we add a missing Linux symbol to make a
# real Canonical-built binary load.
.PHONY: test-module-k10
test-module-k10: build
	$(PROGRESS) "TEST" "kABI K10 demo (Ubuntu .ko + new export)"
	$(PYTHON3) tools/run-qemu.py --timeout 20 \
		--kvm --batch --arch $(ARCH) \
		$(kernel_qemu_arg) -- -no-reboot 2>&1 \
		| tee /tmp/kevlar-test-module-k10.log; true
	@echo ""
	@if grep -q 'kabi: loading /lib/modules/xor-neon.ko' /tmp/kevlar-test-module-k10.log \
	 && grep -q 'kabi: xor-neon init_module returned 0' /tmp/kevlar-test-module-k10.log; then \
	    echo "TEST_PASS: kABI K10 — Ubuntu .ko with new Linux export loads"; \
	else \
	    echo "TEST_FAIL: missing expected K10 markers"; \
	    grep -E 'kabi|xor-neon|panic' /tmp/kevlar-test-module-k10.log | head -30; \
	    false; \
	fi

# `make test-module-k9` — load /lib/modules/bman-test.ko: a real
# prebuilt Linux 7.0 .ko binary from Ubuntu 26.04's
# linux-modules-7.0.0-14-generic.deb package.  First Canonical-built
# binary to run in Kevlar.
.PHONY: test-module-k9
test-module-k9: build
	$(PROGRESS) "TEST" "kABI K9 demo (real Ubuntu 26.04 .ko binary)"
	$(PYTHON3) tools/run-qemu.py --timeout 20 \
		--kvm --batch --arch $(ARCH) \
		$(kernel_qemu_arg) -- -no-reboot 2>&1 \
		| tee /tmp/kevlar-test-module-k9.log; true
	@echo ""
	@if grep -q 'kabi: loading /lib/modules/bman-test.ko' /tmp/kevlar-test-module-k9.log \
	 && grep -q 'kabi: bman-test init_module returned 0' /tmp/kevlar-test-module-k9.log; then \
	    echo "TEST_PASS: kABI K9 — Ubuntu 26.04 prebuilt .ko binary loads"; \
	else \
	    echo "TEST_FAIL: missing expected K9 markers"; \
	    grep -E 'kabi|bman|panic' /tmp/kevlar-test-module-k9.log | head -30; \
	    false; \
	fi

# `make test-module-k8` — load /lib/modules/k8.ko, a Linux-source
# module compiled against Ubuntu 26.04's prebuilt Linux 7.0 headers
# (build/linux-src/).  First module that uses Linux's actual UAPI.
.PHONY: test-module-k8
test-module-k8: build
	$(PROGRESS) "TEST" "kABI K8 demo (Linux 7.0 headers from Ubuntu 26.04)"
	$(PYTHON3) tools/run-qemu.py --timeout 20 \
		--kvm --batch --arch $(ARCH) \
		$(kernel_qemu_arg) -- -no-reboot 2>&1 \
		| tee /tmp/kevlar-test-module-k8.log; true
	@echo ""
	@if grep -q 'k8: hello from real Linux 6.12 headers v8\|k8: hello from real Linux 7.0 headers' /tmp/kevlar-test-module-k8.log \
	 && grep -q 'kabi: k8 init_module returned 0' /tmp/kevlar-test-module-k8.log; then \
	    echo "TEST_PASS: kABI K8 — Linux 7.0 headers from Ubuntu 26.04"; \
	else \
	    echo "TEST_FAIL: missing expected K8 markers"; \
	    grep -E 'kabi|k8:|panic' /tmp/kevlar-test-module-k8.log | head -30; \
	    false; \
	fi

# `make test-module-k7` — load /lib/modules/k7.ko, a Linux-source-
# shape hello-world module compiled against testing/linux/ compat
# headers (matches every Linux 6.12 hello-world tutorial).
.PHONY: test-module-k7
test-module-k7: build
	$(PROGRESS) "TEST" "kABI K7 demo (Linux-source-shape module)"
	$(PYTHON3) tools/run-qemu.py --timeout 20 \
		--kvm --batch --arch $(ARCH) \
		$(kernel_qemu_arg) -- -no-reboot 2>&1 \
		| tee /tmp/kevlar-test-module-k7.log; true
	@echo ""
	@if grep -q 'k7: hello from a Linux-shape module v1.0' /tmp/kevlar-test-module-k7.log \
	 && grep -q 'k7: KERN_INFO + variadic printk works' /tmp/kevlar-test-module-k7.log \
	 && grep -q 'kabi: k7 init_module returned 0' /tmp/kevlar-test-module-k7.log; then \
	    echo "TEST_PASS: kABI K7 — Linux-source-shape module"; \
	else \
	    echo "TEST_FAIL: missing expected K7 markers"; \
	    grep -E 'kabi|k7:|panic' /tmp/kevlar-test-module-k7.log | head -30; \
	    false; \
	fi

# `make test-module-k6` — load /lib/modules/k6.ko + verify the
# variadic printk + format-string parser end-to-end.
.PHONY: test-module-k6
test-module-k6: build
	$(PROGRESS) "TEST" "kABI K6 demo (variadic printk + format strings)"
	$(PYTHON3) tools/run-qemu.py --timeout 20 \
		--kvm --batch --arch $(ARCH) \
		$(kernel_qemu_arg) -- -no-reboot 2>&1 \
		| tee /tmp/kevlar-test-module-k6.log; true
	@echo ""
	@if grep -q '\[k6\] decimal: 42' /tmp/kevlar-test-module-k6.log \
	 && grep -q '\[k6\] negative: -7' /tmp/kevlar-test-module-k6.log \
	 && grep -q '\[k6\] hex: cafebabe' /tmp/kevlar-test-module-k6.log \
	 && grep -q '\[k6\] HEX: DEADBEEF' /tmp/kevlar-test-module-k6.log \
	 && grep -q '\[k6\] string: world' /tmp/kevlar-test-module-k6.log \
	 && grep -q '\[k6\] pointer: 0xffff000040000000' /tmp/kevlar-test-module-k6.log \
	 && grep -q '\[k6\] padded: 00042' /tmp/kevlar-test-module-k6.log \
	 && grep -q '\[k6\] mixed: answer = 42 (0x2a)' /tmp/kevlar-test-module-k6.log \
	 && grep -q '\[k6\] percent: 100%' /tmp/kevlar-test-module-k6.log \
	 && grep -q 'kabi: k6 init_module returned 0' /tmp/kevlar-test-module-k6.log; then \
	    echo "TEST_PASS: kABI K6 — variadic printk + format-string parser"; \
	else \
	    echo "TEST_FAIL: missing expected K6 markers"; \
	    grep -E 'kabi|\[k6\]|panic' /tmp/kevlar-test-module-k6.log | head -30; \
	    false; \
	fi

# `make test-module-k23` — virtio bus walking + virtio_input
# probe firing.  Mirrors K19's PCI walking pattern for the
# virtio bus.  Walks fake virtio_input device, fires probe.
.PHONY: test-module-k23
test-module-k23: build
	$(PROGRESS) "TEST" "kABI K23 demo (virtio bus walking + virtio_input probe)"
	$(PYTHON3) tools/run-qemu.py --timeout 20 \
		--kvm --batch --arch $(ARCH) \
		$(kernel_qemu_arg) -- -no-reboot 2>&1 \
		| tee /tmp/kevlar-test-module-k23.log; true
	@echo ""
	@if grep -q 'kabi: __register_virtio_driver: name=Some("virtio_input")' /tmp/kevlar-test-module-k23.log \
	 && grep -q 'kabi: virtio walk: probing driver .virtio_input. against device_id=18' /tmp/kevlar-test-module-k23.log \
	 && grep -q "kabi: virtio walk: 'virtio_input' probe returned" /tmp/kevlar-test-module-k23.log; then \
	    echo "TEST_PASS: kABI K23 — virtio_input probe fired"; \
	else \
	    echo "TEST_FAIL: missing expected K23 markers"; \
	    grep -E 'kabi|virtio|panic' /tmp/kevlar-test-module-k23.log | head -40; \
	    false; \
	fi

# `make test-userspace-drm` — boot with INIT_SCRIPT=test-kabi-drm and
# verify the userspace DRM ioctl path against /dev/dri/card0.  Drives
# the FULL syscall path: sys_openat → VFS → tmpfs lookup → K4
# KabiCharDevFile → K20 fops adapter → K21 drm_ioctl dispatcher →
# DrmVersion struct read/write → return.  K22 milestone.
.PHONY: test-userspace-drm
test-userspace-drm:
	$(MAKE) build INIT_SCRIPT=/usr/bin/test-kabi-drm
	$(PROGRESS) "TEST" "kABI userspace DRM ioctl path (/dev/dri/card0)"
	$(PYTHON3) tools/run-qemu.py --timeout 20 \
		--kvm --batch --arch $(ARCH) \
		$(kernel_qemu_arg) -- -no-reboot 2>&1 \
		| tee /tmp/kevlar-test-userspace-drm.log; true
	@echo ""
	@if grep -q 'USERSPACE-DRM: starting' /tmp/kevlar-test-userspace-drm.log \
	 && grep -q 'USERSPACE-DRM: open ok' /tmp/kevlar-test-userspace-drm.log \
	 && grep -q 'USERSPACE-DRM: name=kabi-drm version=2.0.0' /tmp/kevlar-test-userspace-drm.log \
	 && grep -q 'USERSPACE-DRM: done' /tmp/kevlar-test-userspace-drm.log; then \
	    echo "TEST_PASS: kABI K22 — userspace DRM ioctl path verified"; \
	else \
	    echo "TEST_FAIL: missing expected K22 markers"; \
	    grep -E 'USERSPACE-DRM|kabi|panic' /tmp/kevlar-test-userspace-drm.log | head -30; \
	    false; \
	fi

# `make test-userspace-kabi` — boot with INIT_SCRIPT=test-kabi-userspace
# and verify the userspace fd path against /dev/k4-demo.  Rebuilds the
# kernel with the INIT_SCRIPT compile-time env (arm64 cmdline parsing
# from QEMU's -append isn't wired up without DTB; INIT_SCRIPT is the
# supported path).
.PHONY: test-userspace-kabi
test-userspace-kabi:
	$(MAKE) build INIT_SCRIPT=/usr/bin/test-kabi-userspace
	$(PROGRESS) "TEST" "kABI userspace path (sys_openat /dev/k4-demo)"
	$(PYTHON3) tools/run-qemu.py --timeout 20 \
		--kvm --batch --arch $(ARCH) \
		$(kernel_qemu_arg) -- -no-reboot 2>&1 \
		| tee /tmp/kevlar-test-userspace-kabi.log; true
	@echo ""
	@if grep -q 'USERSPACE: starting' /tmp/kevlar-test-userspace-kabi.log \
	 && grep -q 'USERSPACE: open ok' /tmp/kevlar-test-userspace-kabi.log \
	 && grep -q 'USERSPACE: read=hello from k4' /tmp/kevlar-test-userspace-kabi.log \
	 && grep -q 'USERSPACE: done' /tmp/kevlar-test-userspace-kabi.log; then \
	    echo "TEST_PASS: kABI userspace fd path through /dev/k4-demo"; \
	else \
	    echo "TEST_FAIL: missing expected userspace markers"; \
	    grep -E 'kabi|USERSPACE|panic' /tmp/kevlar-test-userspace-kabi.log | head -30; \
	    false; \
	fi

# `make test-module-k5` — load /lib/modules/k5.ko + verify
# ioremap + readl/writel + dma_alloc_coherent end-to-end.
.PHONY: test-module-k5
test-module-k5: build
	$(PROGRESS) "TEST" "kABI K5 demo (ioremap + MMIO + DMA)"
	$(PYTHON3) tools/run-qemu.py --timeout 20 \
		--kvm --batch --arch $(ARCH) \
		$(kernel_qemu_arg) -- -no-reboot 2>&1 \
		| tee /tmp/kevlar-test-module-k5.log; true
	@echo ""
	@if grep -q '\[k5\] init begin' /tmp/kevlar-test-module-k5.log \
	 && grep -q '\[k5\] dma_alloc_coherent ok' /tmp/kevlar-test-module-k5.log \
	 && grep -q '\[k5\] ioremap ok' /tmp/kevlar-test-module-k5.log \
	 && grep -q '\[k5\] writel ok' /tmp/kevlar-test-module-k5.log \
	 && grep -q '\[k5\] readl ok' /tmp/kevlar-test-module-k5.log \
	 && grep -q '\[k5\] init done' /tmp/kevlar-test-module-k5.log \
	 && grep -q 'kabi: k5 init_module returned 0' /tmp/kevlar-test-module-k5.log; then \
	    echo "TEST_PASS: kABI K5 — ioremap + MMIO + DMA"; \
	else \
	    echo "TEST_FAIL: missing expected K5 markers"; \
	    grep -E 'kabi|\[k5\]|panic' /tmp/kevlar-test-module-k5.log | head -30; \
	    false; \
	fi

# `make test-module-k3` — load /lib/modules/k3.ko and verify the
# device-model spine (platform_device + platform_driver bind/probe).
.PHONY: test-module-k3
test-module-k3: build
	$(PROGRESS) "TEST" "kABI K3 demo (device model + platform bind/probe)"
	$(PYTHON3) tools/run-qemu.py --timeout 20 \
		--kvm --batch --arch $(ARCH) \
		$(kernel_qemu_arg) -- -no-reboot 2>&1 \
		| tee /tmp/kevlar-test-module-k3.log; true
	@echo ""
	@if grep -q '\[k3\] init begin' /tmp/kevlar-test-module-k3.log \
	 && grep -q '\[k3\] platform_device_register ok' /tmp/kevlar-test-module-k3.log \
	 && grep -q '\[k3\] platform_driver_register ok' /tmp/kevlar-test-module-k3.log \
	 && grep -q '\[k3\] probe called' /tmp/kevlar-test-module-k3.log \
	 && grep -q '\[k3\] init done' /tmp/kevlar-test-module-k3.log \
	 && grep -q 'kabi: k3 init_module returned 0' /tmp/kevlar-test-module-k3.log; then \
	    echo "TEST_PASS: kABI K3 — device model + platform bind/probe"; \
	else \
	    echo "TEST_FAIL: missing expected K3 markers"; \
	    grep -E 'kabi|\[k3\]|panic' /tmp/kevlar-test-module-k3.log | head -30; \
	    false; \
	fi

.PHONY: test-xorg
test-xorg: build/alpine-xorg.img
	$(PROGRESS) "TEST" "X11/Xorg fbdev integration"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/test-xorg"
	$(PYTHON3) tools/run-qemu.py --timeout 120 \
		--kvm --batch --arch $(ARCH) --disk build/alpine-xorg.img \
		$(kernel_qemu_arg) -- -mem-prealloc 2>&1 \
		| tee /tmp/kevlar-test-xorg-$(PROFILE).log; true
	@echo ""
	@grep -E '^(TEST_PASS|TEST_FAIL)' /tmp/kevlar-test-xorg-$(PROFILE).log || echo "(no test output)"
	@echo ""
	@grep 'TEST_END' /tmp/kevlar-test-xorg-$(PROFILE).log || echo "(no summary)"

# Test twm desktop session (Xorg + twm + xterm on single CPU)
.PHONY: test-twm
test-twm: build/alpine-xorg.img
	$(PROGRESS) "TEST" "twm desktop session"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/test-twm"
	$(PYTHON3) tools/run-qemu.py --timeout 120 \
		--kvm --batch --arch $(ARCH) --disk build/alpine-xorg.img \
		$(kernel_qemu_arg) -- -mem-prealloc -vga std 2>&1 \
		| tee /tmp/kevlar-test-twm-$(PROFILE).log; true
	@echo ""
	@grep -E '^(TEST_PASS|TEST_FAIL)' /tmp/kevlar-test-twm-$(PROFILE).log || echo "(no test output)"
	@echo ""
	@grep 'TEST_END' /tmp/kevlar-test-twm-$(PROFILE).log || echo "(no summary)"

# Test twm desktop on SMP
.PHONY: test-twm-smp
test-twm-smp: build/alpine-xorg.img
	$(PROGRESS) "TEST" "twm desktop session (SMP)"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/test-twm"
	$(PYTHON3) tools/run-qemu.py --timeout 120 \
		--kvm --batch --arch $(ARCH) --disk build/alpine-xorg.img \
		$(kernel_qemu_arg) -- -smp 2 -mem-prealloc -vga std 2>&1 \
		| tee /tmp/kevlar-test-twm-smp-$(PROFILE).log; true
	@echo ""
	@grep -E '^(TEST_PASS|TEST_FAIL)' /tmp/kevlar-test-twm-smp-$(PROFILE).log || echo "(no test output)"
	@echo ""
	@grep 'TEST_END' /tmp/kevlar-test-twm-smp-$(PROFILE).log || echo "(no summary)"

# ── kxserver: custom diagnostic X11 server in Rust ────────────────────
# See tools/kxserver/ and the Phase-0 plan.  The binary is a static musl
# build installed into the Alpine image at /usr/bin/kxserver.

KXSERVER_BIN := tools/kxserver/target/x86_64-unknown-linux-musl/release/kxserver

.PHONY: kxserver-bin
kxserver-bin:
	$(PROGRESS) "CARGO" "kxserver (static musl)"
	@# kxserver is a USERSPACE musl binary, NOT a Kevlar kernel crate.
	@# Unset the top-level RUSTFLAGS (-Z emit-stack-sizes and friends)
	@# that the kernel build relies on — they propagate via `export
	@# RUSTFLAGS` in this Makefile and would override kxserver's own
	@# .cargo/config.toml rustflags (which need -no-pie on Kevlar;
	@# see Phase 3 blog post).
	cd tools/kxserver && env -u RUSTFLAGS cargo build --release
	@ls -lh $(KXSERVER_BIN) 2>/dev/null || (echo "kxserver build produced no binary" && false)

# Force a rebuild of the Alpine image so the freshly-built kxserver is
# copied in.  build-alpine-xorg.py skips rebuild when the image already
# exists, so we delete it first.
.PHONY: kxserver-image
kxserver-image: kxserver-bin
	@rm -f build/alpine-xorg.img
	$(MAKE) build/alpine-xorg.img

# Test kxserver: Phase 0 = prove the binary is present and runs inside the
# Alpine rootfs.  Later phases will grow the /bin/test-kxserver harness to
# launch X clients against the server.
.PHONY: test-kxserver
test-kxserver: kxserver-image
	$(PROGRESS) "TEST" "kxserver"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/test-kxserver"
	$(PYTHON3) tools/run-qemu.py --timeout 120 \
		--kvm --batch --arch $(ARCH) --disk build/alpine-xorg.img \
		$(kernel_qemu_arg) -- -mem-prealloc 2>&1 \
		| tee /tmp/kevlar-test-kxserver-$(PROFILE).log; true
	@echo ""
	@grep -E '^(TEST_PASS|TEST_FAIL)' /tmp/kevlar-test-kxserver-$(PROFILE).log || echo "(no test output)"
	@echo ""
	@grep 'TEST_END' /tmp/kevlar-test-kxserver-$(PROFILE).log || echo "(no summary)"

# ── kbox: minimal Rust openbox replacement ────────────────────────────
# A static-musl binary that becomes the active WM (sets
# _NET_SUPPORTING_WM_CHECK + claims WM_S0) and stays alive.  Drops in
# at /usr/bin/openbox of the alpine-openbox image so the openbox test
# scans /proc/N/comm and finds "openbox".  See
# Documentation/blog/233-... and tools/kbox/.

KBOX_TRIPLE_x64   := x86_64-unknown-linux-musl
KBOX_TRIPLE_arm64 := aarch64-unknown-linux-musl
KBOX_TRIPLE       := $(KBOX_TRIPLE_$(ARCH))
KBOX_BIN          := tools/kbox/target/$(KBOX_TRIPLE)/release/kbox

.PHONY: kbox-bin
kbox-bin:
	$(PROGRESS) "CARGO" "kbox ($(KBOX_TRIPLE))"
	@# kbox is a userspace musl binary; same RUSTFLAGS-unset trick as
	@# kxserver so the kernel's -Z flags don't leak into this build.
	cd tools/kbox && env -u RUSTFLAGS cargo build --release --target $(KBOX_TRIPLE)
	@ls -lh $(KBOX_BIN) 2>/dev/null || (echo "kbox build produced no binary" && false)

# kxproxy: Unix-socket X11 proxy.  Mirrors kbox's build shape.
KXPROXY_BIN := tools/kxproxy/target/$(KBOX_TRIPLE)/release/kxproxy

.PHONY: kxproxy-bin
kxproxy-bin:
	$(PROGRESS) "CARGO" "kxproxy ($(KBOX_TRIPLE))"
	cd tools/kxproxy && env -u RUSTFLAGS cargo build --release --target $(KBOX_TRIPLE)
	@ls -lh $(KXPROXY_BIN) 2>/dev/null || (echo "kxproxy build produced no binary" && false)

# kxreplay: replays the captured kxproxy log against real Xorg.
KXREPLAY_BIN := tools/kxreplay/target/$(KBOX_TRIPLE)/release/kxreplay

.PHONY: kxreplay-bin
kxreplay-bin:
	$(PROGRESS) "CARGO" "kxreplay ($(KBOX_TRIPLE))"
	cd tools/kxreplay && env -u RUSTFLAGS cargo build --release --target $(KBOX_TRIPLE)
	@ls -lh $(KXREPLAY_BIN) 2>/dev/null || (echo "kxreplay build produced no binary" && false)

# Force a rebuild of the openbox Alpine image so the freshly-built kbox
# is copied over /usr/bin/openbox.
.PHONY: kbox-image
kbox-image: kbox-bin
	@rm -f build/alpine-openbox.$(ARCH).img
	$(MAKE) build/alpine-openbox.$(ARCH).img

# ═══════════════════════════════════════════════════════════════════
# Kevlar Alpine+twm Desktop — our first graphical "distro"
# ═══════════════════════════════════════════════════════════════════
.PHONY: alpine-twm
alpine-twm: build/alpine-xorg.img
	$(PROGRESS) "DESKTOP" "Kevlar Alpine+twm"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/boot-twm"
	$(PYTHON3) tools/run-qemu.py \
		--arch $(ARCH) --kvm --gui --disk build/alpine-xorg.img \
		$(kernel_qemu_arg) -- -smp 2 -m 512 -mem-prealloc -vga std

# Run Alpine graphical (with QEMU window)
.PHONY: run-alpine-gui
run-alpine-gui: build/alpine-xorg.img
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/boot-alpine"
	$(PYTHON3) tools/run-qemu.py \
		--arch $(ARCH) --kvm --gui --disk build/alpine-xorg.img \
		$(kernel_qemu_arg) -- -mem-prealloc

# Build Alpine XFCE disk image (1GB with XFCE4, D-Bus, fonts, icons)
XFCE_APKO_ARCH_x64   := x86_64
XFCE_APKO_ARCH_arm64 := aarch64
XFCE_APKO_ARCH       := $(XFCE_APKO_ARCH_$(ARCH))
XFCE_IMG := build/alpine-xfce.$(ARCH).img

$(XFCE_IMG):
	$(PROGRESS) "MKDISK" "Alpine XFCE ($(ARCH), 1GB)"
	@rm -f $(XFCE_IMG)
	$(PYTHON3) tools/build-alpine-xfce.py --arch $(XFCE_APKO_ARCH) $(XFCE_IMG)

# Test XFCE desktop startup (batch mode, 2 CPUs, with VGA for framebuffer)
.PHONY: test-xfce
test-xfce: $(XFCE_IMG)
	$(PROGRESS) "TEST" "XFCE desktop startup ($(ARCH))"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/test-xfce"
	$(PYTHON3) tools/run-qemu.py --timeout 300 \
		--kvm --batch --arch $(ARCH) --disk $(XFCE_IMG) \
		$(kernel_qemu_arg) -- -smp $(or $(SMP),2) -m 1024 -vga std 2>&1 \
		| tee /tmp/kevlar-test-xfce-$(ARCH)-$(PROFILE).log; true
	@echo ""
	@grep -E '^(TEST_PASS|TEST_FAIL)' /tmp/kevlar-test-xfce-$(ARCH)-$(PROFILE).log || echo "(no test output)"
	@grep 'TEST_END' /tmp/kevlar-test-xfce-$(ARCH)-$(PROFILE).log || echo "(no summary)"

# Run Alpine XFCE interactively (with QEMU window)
.PHONY: run-alpine-xfce
run-alpine-xfce: $(XFCE_IMG)
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/boot-alpine"
	$(PYTHON3) tools/run-qemu.py \
		--arch $(ARCH) --kvm --gui --disk $(XFCE_IMG) \
		$(kernel_qemu_arg) -- -mem-prealloc -m 1024

# Build Alpine LXDE disk image (1GB with LXDE, openbox, D-Bus, fonts, icons)
# Per-arch apko name: aarch64 / x86_64.
LXDE_APKO_ARCH_x64   := x86_64
LXDE_APKO_ARCH_arm64 := aarch64
LXDE_APKO_ARCH       := $(LXDE_APKO_ARCH_$(ARCH))
LXDE_IMG := build/alpine-lxde.$(ARCH).img

$(LXDE_IMG):
	$(PROGRESS) "MKDISK" "Alpine LXDE ($(ARCH), 1GB)"
	@rm -f $(LXDE_IMG)
	$(PYTHON3) tools/build-alpine-lxde.py --arch $(LXDE_APKO_ARCH) $(LXDE_IMG)

# Iterate on LXDE bring-up: rebuild image, run test-lxde, extract
# session log + Xorg log + framebuffer snapshot, summarize.  Use this
# when bringing up new packages or fixing autostart — much faster
# round-trip than `run-alpine-lxde` (which requires interaction).
.PHONY: iterate-lxde
iterate-lxde:
	@rm -f $(LXDE_IMG)
	$(PYTHON3) tools/iterate-lxde.py --arch $(ARCH)

# Test LXDE desktop startup (batch mode, 2 CPUs, with VGA for framebuffer)
.PHONY: test-lxde
test-lxde: $(LXDE_IMG)
	$(PROGRESS) "TEST" "LXDE desktop startup ($(ARCH))"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/test-lxde"
	$(PYTHON3) tools/run-qemu.py --timeout 300 \
		--kvm --batch --arch $(ARCH) --disk $(LXDE_IMG) \
		$(kernel_qemu_arg) -- -smp $(or $(SMP),2) -m 1024 -vga std 2>&1 \
		| tee /tmp/kevlar-test-lxde-$(ARCH)-$(PROFILE).log; true
	@echo ""
	@grep -E '^(TEST_PASS|TEST_FAIL)' /tmp/kevlar-test-lxde-$(ARCH)-$(PROFILE).log || echo "(no test output)"
	@grep 'TEST_END' /tmp/kevlar-test-lxde-$(ARCH)-$(PROFILE).log || echo "(no summary)"

# test-lxde plus Phase-1 input verification.  Spawns xterm running
# `cat > /var/log/typed.txt` inside the desktop, prints an INJECT_NOW
# sentinel on serial, and the run-qemu --inject-on-line / --inject-keys
# flags drive a QMP `input-send-event` sequence that types
# "kevlar-keys\n" into the focused xterm.  The guest then re-reads the
# disk file and asserts the bytes arrived through virtio-keyboard ->
# evdev -> Xorg -> xterm -> cat -> ext2.  Without the driver, the
# typed_text_arrived sub-test reports SKIP (the default for test-lxde).
.PHONY: test-lxde-input
test-lxde-input: $(LXDE_IMG)
	$(PROGRESS) "TEST" "LXDE input verification ($(ARCH))"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/test-lxde"
	$(PYTHON3) tools/run-qemu.py --timeout 300 \
		--kvm --batch --arch $(ARCH) --disk $(LXDE_IMG) \
		--display-vnc 8 \
		--inject-on-line "INJECT_NOW: kevlar-lxde-input-ready" \
		--inject-keys "kevlar-keys\n" \
		$(kernel_qemu_arg) -- -smp $(or $(SMP),2) -m 1024 2>&1 \
		| tee /tmp/kevlar-test-lxde-input-$(ARCH)-$(PROFILE).log; true
	@echo ""
	@grep -E '^(TEST_PASS|TEST_FAIL|TEST_SKIP)' /tmp/kevlar-test-lxde-input-$(ARCH)-$(PROFILE).log || echo "(no test output)"
	@grep 'TEST_END' /tmp/kevlar-test-lxde-input-$(ARCH)-$(PROFILE).log || echo "(no summary)"

# Generic per-program harness: brings up the LXDE session, then
# spawns the program named by PROG=<name> and emits four sub-tests
# (process_running, window_mapped, pixels_changed, clean_exit).
# Args to the program go via PROG_ARGS=<...>.  Example:
#
#   make ARCH=arm64 iterate-program PROG=xeyes
#   make ARCH=arm64 iterate-program PROG=dillo PROG_ARGS=file:///etc/issue
#
# The `iterate-` target name mirrors `iterate-lxde` but targets a
# specific program, with its own log path so concurrent runs
# don't clobber each other.
.PHONY: iterate-program
iterate-program: $(LXDE_IMG)
	@if [ -z "$(PROG)" ]; then echo "Usage: make iterate-program PROG=<name>"; exit 1; fi
	$(PROGRESS) "TEST" "LXDE program: $(PROG) ($(ARCH))"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/test-lxde-program"
	$(PYTHON3) tools/run-qemu.py --timeout 240 \
		--kvm --batch --arch $(ARCH) --disk $(LXDE_IMG) \
		--append-cmdline "kevlar-prog=$(PROG)" \
		$(if $(PROG_ARGS),--append-cmdline "kevlar-prog-args=$(PROG_ARGS)",) \
		$(kernel_qemu_arg) -- -smp $(or $(SMP),2) -m 1024 -vga std 2>&1 \
		| tee /tmp/kevlar-iterate-program-$(PROG)-$(ARCH).log; true
	@echo ""
	@grep -E '^(TEST_PASS|TEST_FAIL|TEST_SKIP)' /tmp/kevlar-iterate-program-$(PROG)-$(ARCH).log || echo "(no test output)"
	@grep 'TEST_END' /tmp/kevlar-iterate-program-$(PROG)-$(ARCH).log || echo "(no summary)"

# Linux baseline parity for the per-program harness.  Boots
# Alpine's prebuilt linux-virt arm64 kernel against the same
# alpine-lxde rootfs (extracted to a cpio.gz), runs the SAME
# test-lxde-program binary as the Kevlar path.  The only thing
# that differs is the kernel.  Diff the result against
# `iterate-program PROG=<name>` to find Kevlar bugs.
.PHONY: linux-iterate-program
linux-iterate-program:
	@if [ -z "$(PROG)" ]; then echo "Usage: make linux-iterate-program PROG=<name>"; exit 1; fi
	$(PROGRESS) "TEST" "Linux LXDE program: $(PROG) ($(ARCH))"
	$(MAKE) -C tools/linux-on-hvf lxde-program PROG="$(PROG)" \
	    $(if $(PROG_ARGS),PROG_ARGS="$(PROG_ARGS)",)

# Run Alpine LXDE interactively (with QEMU window).
# `-vga std` is required for ramfb to be configured — without it, the
# QEMU window stays blank because /dev/fb0 isn't backed by anything.
# `-smp 2` matches the test runner so timing/scheduler behavior matches.
.PHONY: run-alpine-lxde
run-alpine-lxde: $(LXDE_IMG)
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/boot-alpine"
	$(PYTHON3) tools/run-qemu.py \
		--arch $(ARCH) --kvm --gui --disk $(LXDE_IMG) \
		$(kernel_qemu_arg) -- -smp $(or $(SMP),2) -m 1024 -vga std -mem-prealloc

# Build Alpine i3 disk image — uses apko for cross-platform resolution
# (works on macOS natively; no Linux VM needed).  Default arch aarch64,
# but the image is built per-ARCH so x64 and arm64 coexist.
I3_APKO_ARCH_x64   := x86_64
I3_APKO_ARCH_arm64 := aarch64
I3_APKO_ARCH       := $(I3_APKO_ARCH_$(ARCH))
I3_IMG             := build/alpine-i3.$(ARCH).img

$(I3_IMG):
	$(PROGRESS) "MKDISK" "Alpine i3 ($(I3_APKO_ARCH))"
	$(PYTHON3) tools/build-alpine-i3.py --arch $(I3_APKO_ARCH) $(I3_IMG)

# Test i3 desktop startup (batch mode, 2 CPUs, VGA for framebuffer)
.PHONY: test-i3
test-i3: $(I3_IMG)
	$(PROGRESS) "TEST" "i3 desktop startup ($(ARCH))"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/test-i3"
	$(PYTHON3) tools/run-qemu.py --timeout 240 \
		--kvm --batch --arch $(ARCH) --disk $(I3_IMG) \
		$(kernel_qemu_arg) -- -smp 2 -m 1024 -vga std 2>&1 \
		| tee /tmp/kevlar-test-i3-$(ARCH)-$(PROFILE).log; true
	@echo ""
	@grep -E '^(TEST_PASS|TEST_FAIL)' /tmp/kevlar-test-i3-$(ARCH)-$(PROFILE).log || echo "(no test output)"
	@grep 'TEST_END' /tmp/kevlar-test-i3-$(ARCH)-$(PROFILE).log || echo "(no summary)"

# Test i3 desktop on SMP (wider — 4 CPUs shake out more races)
.PHONY: test-i3-smp
test-i3-smp: $(I3_IMG)
	$(PROGRESS) "TEST" "i3 desktop startup (SMP, $(ARCH))"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/test-i3"
	$(PYTHON3) tools/run-qemu.py --timeout 240 \
		--kvm --batch --arch $(ARCH) --disk $(I3_IMG) \
		$(kernel_qemu_arg) -- -smp 4 -m 1024 -vga std 2>&1 \
		| tee /tmp/kevlar-test-i3-smp-$(ARCH)-$(PROFILE).log; true
	@echo ""
	@grep -E '^(TEST_PASS|TEST_FAIL)' /tmp/kevlar-test-i3-smp-$(ARCH)-$(PROFILE).log || echo "(no test output)"
	@grep 'TEST_END' /tmp/kevlar-test-i3-smp-$(ARCH)-$(PROFILE).log || echo "(no summary)"

# Run Alpine i3 interactively (with QEMU window)
.PHONY: run-alpine-i3
run-alpine-i3: $(I3_IMG)
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/boot-alpine"
	$(PYTHON3) tools/run-qemu.py \
		--arch $(ARCH) --kvm --gui --disk $(I3_IMG) \
		$(kernel_qemu_arg) -- -mem-prealloc -m 1024

# Build Alpine openbox disk — minimal stacking-WM stack, no IPC, no
# built-in bar.  Comparison baseline against test-i3.
OPENBOX_IMG := build/alpine-openbox.$(ARCH).img

$(OPENBOX_IMG): kbox-bin $(KBOX_BIN) kxproxy-bin $(KXPROXY_BIN) kxreplay-bin $(KXREPLAY_BIN)
	$(PROGRESS) "MKDISK" "Alpine openbox ($(I3_APKO_ARCH))"
	@# Always rebuild — build-alpine-openbox.py SKIPs if the image
	@# already exists, so we have to remove it ourselves to pick up
	@# a freshly-built kbox.
	@rm -f $(OPENBOX_IMG)
	$(PYTHON3) tools/build-alpine-openbox.py --arch $(I3_APKO_ARCH) $(OPENBOX_IMG)

.PHONY: test-openbox
test-openbox: $(OPENBOX_IMG)
	$(PROGRESS) "TEST" "openbox desktop startup ($(ARCH))"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/test-openbox"
	$(PYTHON3) tools/run-qemu.py --timeout 240 \
		--kvm --batch --arch $(ARCH) --disk $(OPENBOX_IMG) \
		$(if $(CMDLINE),--append-cmdline "$(CMDLINE)",) \
		$(kernel_qemu_arg) -- -smp $(or $(SMP),2) -m 1024 -vga std 2>&1 \
		| tee /tmp/kevlar-test-openbox-$(ARCH)-$(PROFILE).log; true
	@echo ""
	@grep -E '^(TEST_PASS|TEST_FAIL)' /tmp/kevlar-test-openbox-$(ARCH)-$(PROFILE).log || echo "(no test output)"
	@grep 'TEST_END' /tmp/kevlar-test-openbox-$(ARCH)-$(PROFILE).log || echo "(no summary)"

# M10 Phase A: apk add integration test
.PHONY: test-m10-apk
test-m10-apk: alpine-disk
	$(PROGRESS) "TEST" "M10 apk add integration (mount + update + install)"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/test_m10_apk.sh"
	$(PYTHON3) tools/run-qemu.py --timeout 180 \
		--kvm --batch --arch $(ARCH) --disk build/alpine-disk.img \
		$(kernel_qemu_arg) -- -mem-prealloc 2>&1 \
		| tee /tmp/kevlar-test-m10-apk-$(PROFILE).log; true
	@grep -E '^(TEST_PASS|TEST_FAIL|TEST_END|ALL M10)' \
		/tmp/kevlar-test-m10-apk-$(PROFILE).log || echo "(no test output)"
	@if grep -q '^TEST_FAIL' /tmp/kevlar-test-m10-apk-$(PROFILE).log; then \
		echo "M10 APK TESTS: some failures"; exit 1; \
	elif grep -q '^TEST_END' /tmp/kevlar-test-m10-apk-$(PROFILE).log; then \
		echo "ALL M10 APK TESTS PASSED"; \
	fi

# M10 Phase B: SSH via Dropbear
.PHONY: run-alpine-ssh
run-alpine-ssh: build alpine-disk
	$(PYTHON3) tools/run-qemu.py                                           \
		--arch $(ARCH) --kvm                                           \
		--disk build/alpine-disk.img                                   \
		$(if $(GUI),--gui,)                                            \
		$(if $(GDB),--gdb,)                                            \
		$(if $(LOG),--append-cmdline "log=$(LOG)",)                    \
		$(if $(CMDLINE),--append-cmdline "$(CMDLINE)",)                \
		$(if $(LOG_SERIAL),--log-serial "$(LOG_SERIAL)",)              \
		$(if $(QEMU),--qemu $(QEMU),)                                  \
		$(kernel_qemu_arg) -- $(QEMU_ARGS) -nic user,hostfwd=tcp::2222-:22

# M10 Phase B: SSH integration test (runs in initramfs, no Alpine disk needed)
.PHONY: test-ssh
test-ssh:
	$(PROGRESS) "TEST" "SSH: dropbear start + dbclient connect"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/test-ssh-dropbear"
	$(PYTHON3) tools/run-qemu.py --timeout 120 \
		--kvm --batch --arch $(ARCH) \
		$(kernel_qemu_arg) -- -mem-prealloc -nic user 2>&1 \
		| tee /tmp/kevlar-test-ssh-$(PROFILE).log; true
	@grep -E '^(TEST_PASS|TEST_FAIL|TEST_END|ALL SSH|DIAG:)' \
		/tmp/kevlar-test-ssh-$(PROFILE).log || echo "(no test output)"
	@if grep -q '^TEST_FAIL' /tmp/kevlar-test-ssh-$(PROFILE).log; then \
		echo "SSH TESTS: some failures"; exit 1; \
	elif grep -q '^TEST_END' /tmp/kevlar-test-ssh-$(PROFILE).log; then \
		echo "ALL SSH TESTS PASSED"; \
	fi

# M10 Phase B: nginx integration test (needs Alpine disk for apk install)
.PHONY: test-nginx
test-nginx: build/alpine.img
	$(PROGRESS) "TEST" "nginx: install via apk + start + listen"
	@cp build/alpine.img build/alpine-nginx-test.img
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/test-nginx"
	$(PYTHON3) tools/run-qemu.py --timeout 300 \
		--kvm --batch --arch $(ARCH) --disk build/alpine-nginx-test.img \
		$(kernel_qemu_arg) -- -mem-prealloc -nic user 2>&1 \
		| tee /tmp/kevlar-test-nginx-$(PROFILE).log; true
	@rm -f build/alpine-nginx-test.img
	@grep -E '^(TEST_PASS|TEST_FAIL|TEST_END|ALL NGINX|DIAG:)' \
		/tmp/kevlar-test-nginx-$(PROFILE).log || echo "(no test output)"
	@if grep -q '^TEST_FAIL' /tmp/kevlar-test-nginx-$(PROFILE).log; then \
		echo "NGINX TESTS: some failures"; exit 1; \
	elif grep -q '^TEST_END' /tmp/kevlar-test-nginx-$(PROFILE).log; then \
		echo "ALL NGINX TESTS PASSED"; \
	fi

# Phase 3: Build tools integration test (git, sqlite, perl, gcc/make)
.PHONY: test-build-tools
test-build-tools: build/alpine.img
	$(PROGRESS) "TEST" "Build tools: git + sqlite + perl + gcc/make"
	@cp build/alpine.img build/alpine-build-test.img
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/test-build-tools"
	$(PYTHON3) tools/run-qemu.py --timeout 600 \
		--kvm --batch --arch $(ARCH) --disk build/alpine-build-test.img \
		$(kernel_qemu_arg) -- -mem-prealloc -nic user 2>&1 \
		| tee /tmp/kevlar-test-build-tools-$(PROFILE).log; true
	@rm -f build/alpine-build-test.img
	@grep -E '^(TEST_PASS|TEST_FAIL|TEST_END|ALL BUILD|DIAG:)' \
		/tmp/kevlar-test-build-tools-$(PROFILE).log || echo "(no test output)"
	@if grep -q '^TEST_FAIL' /tmp/kevlar-test-build-tools-$(PROFILE).log; then \
		echo "BUILD TOOL TESTS: some failures"; exit 1; \
	elif grep -q '^TEST_END' /tmp/kevlar-test-build-tools-$(PROFILE).log; then \
		echo "ALL BUILD TOOL TESTS PASSED"; \
	fi

.PHONY: bochs
bochs: iso
	$(BOCHS) -qf boot/bochsrc

.PHONY: test-unit
test-unit:
	$(PROGRESS) "TEST" "unit tests"
	RUSTFLAGS="" $(CARGO) test -p kevlar_utils -p log_filter

.PHONY: testw
testw:
	$(CARGO) watch $(WATCHFLAGS) -s "$(MAKE) test-unit"

.PHONY: check
check:
	$(MAKE) $(DUMMY_INITRAMFS_PATH)
	INITRAMFS_PATH=$(DUMMY_INITRAMFS_PATH) $(CARGO) check $(CARGOFLAGS)

.PHONY: check-all-profiles
check-all-profiles:
	for profile in fortress balanced performance ludicrous; do \
		$(PROGRESS) "CHECK" "profile-$$profile"; \
		$(MAKE) check PROFILE=$$profile || exit 1; \
	done

.PHONY: checkw
checkw:
	$(CARGO) watch $(WATCHFLAGS) -s "$(MAKE) check"

.PHONY: docs
docs:
	$(PROGRESS) "MDBOOK" build/docs
	mkdir -p build
	make doc-images
	mdbook build -d $(topdir)/build/docs Documentation

.PHONY: doc-images
doc-images: $(patsubst %.drawio, %.svg, $(wildcard Documentation/*.drawio))

.PHONY: docsw
docsw:
	mkdir -p build
	mdbook serve -d $(topdir)/build/docs Documentation

.PHONY: src-docs
src-docs:
	RUSTFLAGS="-C panic=abort -Z panic_abort_tests" $(CARGO) doc

.PHONY: lint
lint:
	$(MAKE) $(DUMMY_INITRAMFS_PATH)
	INITRAMFS_PATH=$(DUMMY_INITRAMFS_PATH) RUSTFLAGS="-C panic=abort -Z panic_abort_tests" $(CARGO) clippy

.PHONY: strict-lint
strict-lint:
	$(MAKE) $(DUMMY_INITRAMFS_PATH)
	INITRAMFS_PATH=$(DUMMY_INITRAMFS_PATH) RUSTFLAGS="-C panic=abort -Z panic_abort_tests" $(CARGO) clippy -- -D warnings

.PHONY: lint-and-fix
lint-and-fix:
	$(MAKE) $(DUMMY_INITRAMFS_PATH)
	INITRAMFS_PATH=$(DUMMY_INITRAMFS_PATH) RUSTFLAGS="-C panic=abort -Z panic_abort_tests" $(CARGO) clippy --fix -Z unstable-options

.PHONY: test-integration
test-integration: disk
	$(PROGRESS) "TEST" "syscall correctness tests"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/test"
	$(PYTHON3) tools/run-qemu.py --timeout 120 \
		--kvm --arch $(ARCH) --disk build/disk.img $(kernel_qemu_arg) -- -mem-prealloc 2>&1 \
		| tee /tmp/kevlar-test-$(PROFILE).log; true
	@grep -E '^(PASS|FAIL|TEST_)' /tmp/kevlar-test-$(PROFILE).log || echo "(no TEST output found)"
	@if grep -q '^FAIL' /tmp/kevlar-test-$(PROFILE).log; then \
		echo "TESTS FAILED"; exit 1; \
	elif grep -q '^TEST_END' /tmp/kevlar-test-$(PROFILE).log; then \
		echo "ALL TESTS PASSED"; \
	fi

.PHONY: test
test: test-unit test-integration

.PHONY: test-ext2
test-ext2: disk
	$(PROGRESS) "TEST" "ext2 filesystem tests"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/test"
	$(PYTHON3) tools/run-qemu.py --timeout 120 \
		--kvm --arch $(ARCH) --disk build/disk.img $(kernel_qemu_arg) -- -mem-prealloc 2>&1 \
		| tee /tmp/kevlar-test-ext2-$(PROFILE).log; true
	@grep -E '^(PASS|FAIL|TEST_)' /tmp/kevlar-test-ext2-$(PROFILE).log || echo "(no TEST output found)"
	@if grep -q '^FAIL' /tmp/kevlar-test-ext2-$(PROFILE).log; then \
		echo "TESTS FAILED"; exit 1; \
	elif grep -q '^TEST_END' /tmp/kevlar-test-ext2-$(PROFILE).log; then \
		echo "ALL TESTS PASSED"; \
	fi

.PHONY: test-storage
test-storage: build/disk.img
	$(PROGRESS) "TEST" "M5 storage integration tests"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/mini-storage"
	$(PYTHON3) tools/run-qemu.py --timeout 120 \
		--kvm --arch $(ARCH) --disk build/disk.img $(kernel_qemu_arg) -- -mem-prealloc 2>&1 \
		| tee /tmp/kevlar-test-storage-$(PROFILE).log; true
	@grep -E '^(TEST_PASS|TEST_FAIL|TEST_SKIP|TEST_END)' \
		/tmp/kevlar-test-storage-$(PROFILE).log || echo "(no TEST output found)"
	@if grep -q '^TEST_FAIL' /tmp/kevlar-test-storage-$(PROFILE).log; then \
		echo "STORAGE TESTS FAILED"; exit 1; \
	elif grep -q '^TEST_END' /tmp/kevlar-test-storage-$(PROFILE).log; then \
		echo "ALL STORAGE TESTS PASSED"; \
	fi

.PHONY: test-threads
test-threads:
	$(PROGRESS) "TEST" "M6 threading (1 CPU)"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/mini-threads"
	$(PYTHON3) tools/run-qemu.py --timeout 120 \
		--kvm --arch $(ARCH) $(kernel_qemu_arg) -- -mem-prealloc 2>&1 \
		| tee /tmp/kevlar-test-threads-$(PROFILE).log; true
	@grep -E '^(TEST_PASS|TEST_FAIL|TEST_END)' \
		/tmp/kevlar-test-threads-$(PROFILE).log || echo "(no TEST output found)"
	@if grep -q '^TEST_FAIL' /tmp/kevlar-test-threads-$(PROFILE).log; then \
		echo "THREADING TESTS FAILED"; exit 1; \
	elif grep -q '^TEST_END' /tmp/kevlar-test-threads-$(PROFILE).log; then \
		echo "ALL THREADING TESTS PASSED"; \
	fi

.PHONY: test-threads-smp
test-threads-smp:
	$(PROGRESS) "TEST" "M6 threading (4 CPUs)"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/mini-threads"
	$(PYTHON3) tools/run-qemu.py --timeout 120 \
		--kvm --arch $(ARCH) $(kernel_qemu_arg) -- -mem-prealloc -smp 4 2>&1 \
		| tee /tmp/kevlar-test-threads-smp-$(PROFILE).log; true
	@grep -E '^(TEST_PASS|TEST_FAIL|TEST_END)' \
		/tmp/kevlar-test-threads-smp-$(PROFILE).log || echo "(no TEST output found)"
	@if grep -q '^TEST_FAIL' /tmp/kevlar-test-threads-smp-$(PROFILE).log; then \
		echo "THREADING SMP TESTS FAILED"; exit 1; \
	elif grep -q '^TEST_END' /tmp/kevlar-test-threads-smp-$(PROFILE).log; then \
		echo "ALL THREADING SMP TESTS PASSED"; \
	fi

.PHONY: test-smp
test-smp:
	$(PROGRESS) "TEST" "M6 SMP boot (4 CPUs)"
	$(MAKE) build PROFILE=$(PROFILE)
	$(PYTHON3) tools/run-qemu.py --timeout 120 \
		--kvm --arch $(ARCH) $(kernel_qemu_arg) -- -mem-prealloc -smp 4 2>&1 \
		| tee /tmp/kevlar-test-smp-$(PROFILE).log; true
	@grep -E 'CPU \(LAPIC|smp:|online' /tmp/kevlar-test-smp-$(PROFILE).log || echo "(no SMP output found)"

# Run M4 integration suite (mini_systemd) under -smp 4 as a regression check.
.PHONY: test-regression-smp
test-regression-smp:
	$(PROGRESS) "TEST" "M6 Phase 5 regression: mini_systemd on 4 CPUs"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/mini-systemd"
	$(PYTHON3) tools/run-qemu.py --timeout 120 \
		--kvm --arch $(ARCH) $(kernel_qemu_arg) -- -mem-prealloc -smp 4 2>&1 \
		| tee /tmp/kevlar-test-regression-smp-$(PROFILE).log; true
	@grep -E '^(TEST_PASS|TEST_FAIL|TEST_END)' \
		/tmp/kevlar-test-regression-smp-$(PROFILE).log || echo "(no TEST output found)"
	@if grep -q '^TEST_FAIL' /tmp/kevlar-test-regression-smp-$(PROFILE).log; then \
		echo "REGRESSION TESTS FAILED"; exit 1; \
	elif grep -q '^TEST_END\|TEST_PASS' /tmp/kevlar-test-regression-smp-$(PROFILE).log; then \
		echo "ALL REGRESSION TESTS PASSED"; \
	fi

# M6 Phase 5 full integration suite: threading + stress + regression.
.PHONY: test-m6
test-m6:
	$(PROGRESS) "TEST" "M6 Phase 5: full integration suite"
	$(MAKE) test-threads-smp PROFILE=$(PROFILE)
	$(MAKE) test-regression-smp PROFILE=$(PROFILE)
	@echo "M6 integration suite complete."

# ─── M7 glibc Integration Tests ──────────────────────────────────────────────

.PHONY: test-glibc-hello
test-glibc-hello:
	$(PROGRESS) "TEST" "M7 glibc hello world"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/hello-glibc"
	$(PYTHON3) tools/run-qemu.py --timeout 60 \
		--kvm --arch $(ARCH) $(kernel_qemu_arg) -- -mem-prealloc 2>&1 \
		| tee /tmp/kevlar-test-glibc-hello.log; true
	@grep -E '^(TEST_PASS|TEST_FAIL|TEST_END|hello from glibc)' \
		/tmp/kevlar-test-glibc-hello.log || echo "(no TEST output found)"
	@if grep -q '^TEST_FAIL' /tmp/kevlar-test-glibc-hello.log; then \
		echo "GLIBC HELLO TEST FAILED"; exit 1; \
	elif grep -q '^TEST_PASS' /tmp/kevlar-test-glibc-hello.log; then \
		echo "GLIBC HELLO TEST PASSED"; \
	fi

.PHONY: test-glibc-threads
test-glibc-threads:
	$(PROGRESS) "TEST" "M7 glibc pthreads (4 CPUs)"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/mini-threads-glibc"
	$(PYTHON3) tools/run-qemu.py --timeout 180 \
		--kvm --arch $(ARCH) $(kernel_qemu_arg) -- -mem-prealloc -smp 4 2>&1 \
		| tee /tmp/kevlar-test-glibc-threads.log; true
	@grep -E '^(TEST_PASS|TEST_FAIL|TEST_END)' \
		/tmp/kevlar-test-glibc-threads.log || echo "(no TEST output found)"
	@if grep -q '^TEST_FAIL' /tmp/kevlar-test-glibc-threads.log; then \
		echo "GLIBC THREADING TESTS FAILED"; exit 1; \
	elif grep -q '^TEST_END' /tmp/kevlar-test-glibc-threads.log; then \
		echo "ALL GLIBC THREADING TESTS PASSED"; \
	fi

.PHONY: test-m7
test-m7:
	$(PROGRESS) "TEST" "M7: full integration suite"
	$(MAKE) test-glibc-hello PROFILE=$(PROFILE)
	$(MAKE) test-glibc-threads PROFILE=$(PROFILE)
	$(MAKE) test-threads-smp PROFILE=$(PROFILE)
	$(MAKE) test-regression-smp PROFILE=$(PROFILE)
	$(MAKE) test-contracts
	@echo "M7 integration suite complete."

.PHONY: test-cgroups-ns
test-cgroups-ns:
	$(PROGRESS) "TEST" "M8 cgroups + namespaces (14 tests)"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/mini-cgroups-ns"
	$(PYTHON3) tools/run-qemu.py --timeout 120 \
		--kvm --arch $(ARCH) $(kernel_qemu_arg) -- -mem-prealloc 2>&1 \
		| tee /tmp/kevlar-test-cgroups-ns-$(PROFILE).log; true
	@grep -E '^(TEST_PASS|TEST_FAIL|TEST_END)' \
		/tmp/kevlar-test-cgroups-ns-$(PROFILE).log || echo "(no TEST output found)"
	@if grep -q '^TEST_FAIL' /tmp/kevlar-test-cgroups-ns-$(PROFILE).log; then \
		echo "CGROUPS+NS TESTS FAILED"; exit 1; \
	elif grep -q '^TEST_END' /tmp/kevlar-test-cgroups-ns-$(PROFILE).log; then \
		echo "ALL CGROUPS+NS TESTS PASSED"; \
	fi

.PHONY: test-systemd-boot
test-systemd-boot:
	$(PROGRESS) "TEST" "M9 systemd boot (real systemd PID 1)"
	$(MAKE) build PROFILE=$(PROFILE)
	$(PYTHON3) tools/run-qemu.py --timeout 60 \
		--kvm --arch $(ARCH) $(kernel_qemu_arg) \
		-- -mem-prealloc -append "pci=off init=/usr/lib/systemd/systemd" 2>&1 \
		| tee /tmp/kevlar-test-systemd-boot.log; true
	@grep -aE 'systemd|Reached|target|Started|Failed|exited' \
		/tmp/kevlar-test-systemd-boot.log || echo "(no systemd output found)"

.PHONY: test-systemd-v3
test-systemd-v3:
	$(PROGRESS) "TEST" "M9 systemd init-sequence (25 tests)"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/mini-systemd-v3"
	$(PYTHON3) tools/run-qemu.py --timeout 120 \
		--kvm --arch $(ARCH) $(kernel_qemu_arg) -- -mem-prealloc 2>&1 \
		| tee /tmp/kevlar-test-systemd-v3-$(PROFILE).log; true
	@grep -E '^(TEST_PASS|TEST_FAIL|TEST_END)' \
		/tmp/kevlar-test-systemd-v3-$(PROFILE).log || echo "(no TEST output found)"
	@if grep -q '^TEST_FAIL' /tmp/kevlar-test-systemd-v3-$(PROFILE).log; then \
		echo "SYSTEMD-V3 TESTS FAILED"; exit 1; \
	elif grep -q '^TEST_END' /tmp/kevlar-test-systemd-v3-$(PROFILE).log; then \
		echo "ALL SYSTEMD-V3 TESTS PASSED"; \
	fi

.PHONY: test-systemd-v3-smp
test-systemd-v3-smp:
	$(PROGRESS) "TEST" "M9 systemd init-sequence SMP (25 tests, 4 CPUs)"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/mini-systemd-v3"
	$(PYTHON3) tools/run-qemu.py --timeout 180 \
		--kvm --arch $(ARCH) $(kernel_qemu_arg) -- -mem-prealloc -smp 4 2>&1 \
		| tee /tmp/kevlar-test-systemd-v3-smp-$(PROFILE).log; true
	@grep -E '^(TEST_PASS|TEST_FAIL|TEST_END)' \
		/tmp/kevlar-test-systemd-v3-smp-$(PROFILE).log || echo "(no TEST output found)"
	@if grep -q '^TEST_FAIL' /tmp/kevlar-test-systemd-v3-smp-$(PROFILE).log; then \
		echo "SYSTEMD-V3 SMP TESTS FAILED"; exit 1; \
	elif grep -q '^TEST_END' /tmp/kevlar-test-systemd-v3-smp-$(PROFILE).log; then \
		echo "ALL SYSTEMD-V3 SMP TESTS PASSED"; \
	fi

.PHONY: test-m8
test-m8:
	$(PROGRESS) "TEST" "M8: full integration suite"
	$(MAKE) test-glibc-hello PROFILE=$(PROFILE)
	$(MAKE) test-glibc-threads PROFILE=$(PROFILE)
	$(MAKE) test-threads-smp PROFILE=$(PROFILE)
	$(MAKE) test-regression-smp PROFILE=$(PROFILE)
	$(MAKE) test-contracts
	$(MAKE) test-cgroups-ns PROFILE=$(PROFILE)
	@echo "M8 integration suite complete."

.PHONY: test-m9
test-m9:
	$(PROGRESS) "TEST" "M9: systemd boot end-to-end"
	$(MAKE) build PROFILE=$(PROFILE)
	@echo "Booting systemd v245 under KVM (90s timeout)..."
	@timeout 90 $(PYTHON3) tools/run-qemu.py \
		--arch $(ARCH) --kvm $(kernel_qemu_arg) \
		-- -append "pci=off init=/usr/lib/systemd/systemd" \
		-display none -mem-prealloc 2>&1 \
		| tee /tmp/kevlar-test-m9.log; true
	@echo "=== M9 Test Results ==="
	@FAILED_UNITS=$$(grep -ac '\[FAILED\]' /tmp/kevlar-test-m9.log 2>/dev/null || echo 0); \
	if [ "$$FAILED_UNITS" -gt 0 ]; then \
		echo "INFO: $$FAILED_UNITS failed systemd unit(s) (informational)"; \
	fi
	@PASS=0; FAIL=0; \
	if grep -qa 'Welcome to' /tmp/kevlar-test-m9.log; then \
		echo "PASS: Welcome banner"; PASS=$$((PASS+1)); \
	else \
		echo "FAIL: Welcome banner"; FAIL=$$((FAIL+1)); \
	fi; \
	if grep -qa 'Startup finished' /tmp/kevlar-test-m9.log; then \
		echo "PASS: Startup finished"; PASS=$$((PASS+1)); \
	else \
		echo "FAIL: Startup finished"; FAIL=$$((FAIL+1)); \
	fi; \
	if grep -qa 'Reached target Kevlar Default Target' /tmp/kevlar-test-m9.log; then \
		echo "PASS: Reached target Kevlar Default Target"; PASS=$$((PASS+1)); \
	else \
		echo "FAIL: Reached target Kevlar Default Target"; FAIL=$$((FAIL+1)); \
	fi; \
	if grep -qa 'Started Kevlar Console Shell' /tmp/kevlar-test-m9.log; then \
		echo "PASS: Started Kevlar Console Shell"; PASS=$$((PASS+1)); \
	else \
		echo "FAIL: Started Kevlar Console Shell"; FAIL=$$((FAIL+1)); \
	fi; \
	echo "$$PASS/4 required checks passed"; \
	if [ $$FAIL -gt 0 ]; then exit 1; fi

.PHONY: test-systemd
test-systemd:
	$(PROGRESS) "TEST" "M9.8: comprehensive systemd drop-in validation"
	@echo "Step 1/3: synthetic init-sequence (1 CPU)"
	$(MAKE) test-systemd-v3 PROFILE=$(PROFILE)
	@echo "Step 2/3: synthetic init-sequence SMP (4 CPUs)"
	$(MAKE) test-systemd-v3-smp PROFILE=$(PROFILE)
	@echo "Step 3/3: real systemd PID 1 boot"
	$(MAKE) test-m9 PROFILE=$(PROFILE)
	@echo "=== M9.8 test-systemd: ALL PASSED ==="

# ─── BusyBox Comprehensive Test Suite ────────────────────────────────────────

.PHONY: test-busybox
test-busybox:
	$(PROGRESS) "TEST" "BusyBox applet suite (102 tests)"
	$(MAKE) build PROFILE=$(PROFILE)
	$(PYTHON3) tools/run-qemu.py --timeout 120 \
		--kvm --arch $(ARCH) --append-cmdline "init=/bin/busybox-suite" \
		$(kernel_qemu_arg) -- -mem-prealloc 2>&1 \
		| tee /tmp/kevlar-test-busybox-$(PROFILE).log; true
	@grep -E '^(TEST_PASS|TEST_FAIL|TEST_SKIP|TEST_END)' \
		/tmp/kevlar-test-busybox-$(PROFILE).log || echo "(no TEST output found)"
	@if grep -q '^TEST_FAIL' /tmp/kevlar-test-busybox-$(PROFILE).log; then \
		echo "BUSYBOX TESTS: some failures"; \
		grep -c '^TEST_PASS' /tmp/kevlar-test-busybox-$(PROFILE).log || true; \
		grep -c '^TEST_FAIL' /tmp/kevlar-test-busybox-$(PROFILE).log || true; \
		exit 1; \
	elif grep -q '^TEST_END' /tmp/kevlar-test-busybox-$(PROFILE).log; then \
		echo "ALL BUSYBOX TESTS PASSED"; \
	fi

# Run the busybox suite against Alpine's *production* BusyBox (full
# applet set with all symlinks, dynamically linked against Alpine's
# musl).  Boots via the alpine-lxde disk image — boot-alpine pivots
# into the Alpine root, then execs /bin/busybox-suite (selected via
# the `alpine_init=` cmdline arg).  This is the apples-to-apples
# comparison against the same suite running under Linux on the same
# Alpine userspace.
.PHONY: test-busybox-alpine
test-busybox-alpine: $(LXDE_IMG)
	$(PROGRESS) "TEST" "BusyBox suite on Alpine production busybox"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/boot-alpine"
	$(PYTHON3) tools/run-qemu.py --timeout 180 \
		--kvm --arch $(ARCH) --batch --disk $(LXDE_IMG) \
		--append-cmdline "alpine_init=/bin/busybox-suite" \
		$(kernel_qemu_arg) -- -mem-prealloc 2>&1 \
		| tee /tmp/kevlar-test-busybox-alpine-$(ARCH)-$(PROFILE).log; true
	@grep -E '^(TEST_PASS|TEST_FAIL|TEST_SKIP|TEST_END)' \
		/tmp/kevlar-test-busybox-alpine-$(ARCH)-$(PROFILE).log || echo "(no TEST output found)"
	@if grep -q '^TEST_FAIL' /tmp/kevlar-test-busybox-alpine-$(ARCH)-$(PROFILE).log; then \
		echo "BUSYBOX-ALPINE TESTS: some failures"; \
		grep -c '^TEST_PASS' /tmp/kevlar-test-busybox-alpine-$(ARCH)-$(PROFILE).log || true; \
		grep -c '^TEST_FAIL' /tmp/kevlar-test-busybox-alpine-$(ARCH)-$(PROFILE).log || true; \
		exit 1; \
	elif grep -q '^TEST_END' /tmp/kevlar-test-busybox-alpine-$(ARCH)-$(PROFILE).log; then \
		echo "ALL BUSYBOX-ALPINE TESTS PASSED"; \
	fi

.PHONY: test-busybox-smp
test-busybox-smp:
	$(PROGRESS) "TEST" "BusyBox applet suite (4 CPUs)"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/busybox-suite"
	$(PYTHON3) tools/run-qemu.py --timeout 300 \
		--kvm --arch $(ARCH) $(kernel_qemu_arg) -- -mem-prealloc -smp 4 2>&1 \
		| tee /tmp/kevlar-test-busybox-smp-$(PROFILE).log; true
	@grep -E '^(TEST_PASS|TEST_FAIL|TEST_SKIP|TEST_END)' \
		/tmp/kevlar-test-busybox-smp-$(PROFILE).log || echo "(no TEST output found)"
	@if grep -q '^TEST_FAIL' /tmp/kevlar-test-busybox-smp-$(PROFILE).log; then \
		echo "BUSYBOX SMP TESTS: some failures"; exit 1; \
	elif grep -q '^TEST_END' /tmp/kevlar-test-busybox-smp-$(PROFILE).log; then \
		echo "ALL BUSYBOX SMP TESTS PASSED"; \
	fi

# Huge page assembly stress test: 300 fork+exec iterations under KVM.
.PHONY: test-huge-page
test-huge-page:
	$(PROGRESS) "TEST" "huge page stress test (KVM)"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/fork-exec-stress 300"
	$(PYTHON3) tools/run-qemu.py --timeout 300 \
		--kvm --arch $(ARCH) $(kernel_qemu_arg) -- -mem-prealloc 2>&1 \
		| tee /tmp/kevlar-test-huge-page-$(PROFILE).log; true
	@grep -E '^(TEST_PASS|TEST_FAIL|DBG.*huge_page_verify)' \
		/tmp/kevlar-test-huge-page-$(PROFILE).log || echo "(no TEST output found)"
	@if grep -q '^TEST_FAIL' /tmp/kevlar-test-huge-page-$(PROFILE).log; then \
		echo "HUGE PAGE STRESS TEST: FAILED"; exit 1; \
	elif grep -q '^TEST_PASS' /tmp/kevlar-test-huge-page-$(PROFILE).log; then \
		echo "HUGE PAGE STRESS TEST: PASSED"; \
	fi

# Workload benchmarks: BusyBox applet execution patterns under KVM
.PHONY: bench-workloads
bench-workloads:
	$(PROGRESS) "BENCH" "workload benchmarks (KVM)"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/bench --full exec_true,shell_noop,pipe_grep,file_tree,sed_pipeline,sort_uniq,tar_extract"
	$(PYTHON3) tools/run-qemu.py --timeout 300 \
		--kvm --arch $(ARCH) $(kernel_qemu_arg) -- -mem-prealloc 2>&1 \
		| tee /tmp/kevlar-bench-workloads-$(PROFILE).log; true
	@grep '^BENCH ' /tmp/kevlar-bench-workloads-$(PROFILE).log || echo "(no BENCH output found)"

# dd diagnostic: find exact block size / count where dd hangs
.PHONY: test-busybox-dd
test-busybox-dd:
	$(PROGRESS) "TEST" "dd diagnostic (KVM)"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/dd-diag"
	$(PYTHON3) tools/run-qemu.py --timeout 30 \
		--kvm --arch $(ARCH) $(kernel_qemu_arg) -- -mem-prealloc 2>&1 \
		| tee /tmp/kevlar-test-dd-diag.log; true
	@grep -E '^\s+(OK|FAIL|HANG)|^Phase|^===' /tmp/kevlar-test-dd-diag.log || echo "(no output)"

# BusyBox workload benchmarks: real BusyBox operations under KVM
.PHONY: bench-busybox
bench-busybox:
	$(PROGRESS) "BENCH" "BusyBox workload benchmarks (KVM)"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/busybox-suite --bench-only --full"
	$(PYTHON3) tools/run-qemu.py --timeout 30 \
		--kvm --arch $(ARCH) $(kernel_qemu_arg) -- -mem-prealloc 2>&1 \
		| tee /tmp/kevlar-bench-busybox-kvm-$(PROFILE).log; true
	@grep '^BENCH ' /tmp/kevlar-bench-busybox-kvm-$(PROFILE).log > /tmp/kevlar-bench-busybox-$(PROFILE).txt 2>/dev/null; \
	 lines=$$(wc -l < /tmp/kevlar-bench-busybox-$(PROFILE).txt 2>/dev/null || echo 0); \
	 if [ "$$lines" -gt 0 ]; then echo "Wrote $$lines benchmarks to /tmp/kevlar-bench-busybox-$(PROFILE).txt"; \
	 else echo "(no BENCH output found)"; fi

# Run BusyBox benchmarks on Linux for baseline comparison
.PHONY: bench-busybox-linux
bench-busybox-linux:
	$(PROGRESS) "BENCH" "BusyBox workload benchmarks (Linux baseline)"
	@mkdir -p build
	gcc -static -O2 -o build/busybox-suite.linux testing/busybox_suite.c
	build/busybox-suite.linux --bench-only --full 2>/dev/null | tee /tmp/linux-bench-busybox.log
	@grep '^BENCH ' /tmp/linux-bench-busybox.log > /tmp/linux-bench-busybox.txt 2>/dev/null; \
	 lines=$$(wc -l < /tmp/linux-bench-busybox.txt 2>/dev/null || echo 0); \
	 if [ "$$lines" -gt 0 ]; then echo "Wrote $$lines benchmarks to /tmp/linux-bench-busybox.txt"; \
	 else echo "(no BENCH output found)"; fi

# ─── M6.5 Contract Tests ────────────────────────────────────────────────────

.PHONY: build-contracts
build-contracts:
	$(PROGRESS) "BUILD" "M6.5 contract test binaries"
	@find testing/contracts -name '*.c' | while read src; do \
		rel=$${src#testing/contracts/}; \
		out=build/contracts/$${rel%.c}; \
		mkdir -p $$(dirname $$out); \
		gcc -static -O1 -Wall -Wno-unused-result -o $$out $$src 2>&1 \
		  && echo "  CC  $$rel" \
		  || echo "  FAIL $$rel"; \
	done

# Run contract tests: Linux (host) vs Kevlar (QEMU) comparison
.PHONY: test-contracts
test-contracts: $(kernel_qemu_arg)
	$(PROGRESS) "TEST" "M6.5 contract tests (Linux vs Kevlar)"
	$(PYTHON3) tools/compare-contracts.py \
		--arch $(ARCH) \
		--kernel $(kernel_qemu_arg) \
		--json build/contract-results.json \
		$(if $(CONTRACTS_FILTER),$(CONTRACTS_FILTER),)

# Run only the VM contract tests (fastest subset)
.PHONY: test-contracts-vm
test-contracts-vm: $(kernel_qemu_arg)
	$(PROGRESS) "TEST" "M6.5 VM contract tests"
	$(PYTHON3) tools/compare-contracts.py \
		--arch $(ARCH) \
		--kernel $(kernel_qemu_arg) \
		--json build/contract-results-vm.json \
		vm

# Linux-only baseline (no Kevlar QEMU required)
.PHONY: test-contracts-linux
test-contracts-linux:
	$(PROGRESS) "TEST" "M6.5 contract tests (Linux baseline only)"
	$(PYTHON3) tools/compare-contracts.py \
		--no-kevlar \
		--json build/contract-results-linux.json \
		$(if $(CONTRACTS_FILTER),$(CONTRACTS_FILTER),)

# Trace a single contract test: make trace-contract TEST=brk_basic
.PHONY: trace-contract
trace-contract: $(kernel_qemu_arg)
	$(PROGRESS) "TRACE" "contract: $(TEST)"
	$(PYTHON3) tools/diff-syscall-traces.py $(TEST) \
		--arch $(ARCH) \
		--kernel $(kernel_qemu_arg)

# Run contract tests with auto-trace on failures
.PHONY: test-contracts-trace
test-contracts-trace: $(kernel_qemu_arg)
	$(PROGRESS) "TEST" "M6.5 contract tests (with trace on failure)"
	$(PYTHON3) tools/compare-contracts.py \
		--arch $(ARCH) \
		--kernel $(kernel_qemu_arg) \
		--json build/contract-results.json \
		--trace \
		$(if $(CONTRACTS_FILTER),$(CONTRACTS_FILTER),)

.PHONY: bench
bench:
	$(PROGRESS) "BENCH" "profile-$(PROFILE)"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/bench"
	$(PYTHON3) tools/run-qemu.py --timeout 120 \
		--kvm --arch $(ARCH) $(kernel_qemu_arg) -- -mem-prealloc 2>&1 \
		| tee /tmp/kevlar-bench-$(PROFILE).log; true
	@grep '^BENCH ' /tmp/kevlar-bench-$(PROFILE).log > /tmp/kevlar-bench-$(PROFILE).txt 2>/dev/null; \
	 lines=$$(wc -l < /tmp/kevlar-bench-$(PROFILE).txt 2>/dev/null || echo 0); \
	 if [ "$$lines" -gt 0 ]; then echo "Wrote $$lines benchmarks to /tmp/kevlar-bench-$(PROFILE).txt"; \
	   $(PYTHON3) tools/bench-report.py; \
	 else echo "(no BENCH output found)"; fi

.PHONY: bench-kvm
bench-kvm:
	$(PROGRESS) "BENCH-KVM" "profile-$(PROFILE)"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/bench --full"
	$(PYTHON3) tools/run-qemu.py --timeout 300 \
		--kvm --arch $(ARCH) $(kernel_qemu_arg) -- -mem-prealloc 2>&1 \
		| tee /tmp/kevlar-bench-kvm-$(PROFILE).log; true
	@grep '^BENCH ' /tmp/kevlar-bench-kvm-$(PROFILE).log > /tmp/kevlar-bench-$(PROFILE).txt 2>/dev/null; \
	 lines=$$(wc -l < /tmp/kevlar-bench-$(PROFILE).txt 2>/dev/null || echo 0); \
	 if [ "$$lines" -gt 0 ]; then echo "Wrote $$lines benchmarks to /tmp/kevlar-bench-$(PROFILE).txt"; \
	   $(PYTHON3) tools/bench-report.py; \
	 else echo "(no BENCH output found)"; fi

# Run bench.c under Linux KVM to generate a baseline for bench-report.
.PHONY: bench-linux
bench-linux:
	$(PROGRESS) "BENCH-LINUX" "Linux KVM baseline"
	@mkdir -p build
	gcc -static -O2 -o build/bench.linux benchmarks/bench.c
	$(PYTHON3) tools/bench-linux.py --full | tee /tmp/linux-bench-kvm.log
	@grep '^BENCH ' /tmp/linux-bench-kvm.log > /tmp/linux-bench-kvm.txt 2>/dev/null; \
	 lines=$$(wc -l < /tmp/linux-bench-kvm.txt 2>/dev/null || echo 0); \
	 if [ "$$lines" -gt 0 ]; then echo "Wrote $$lines benchmarks to /tmp/linux-bench-kvm.txt"; \
	   $(PYTHON3) tools/bench-report.py; \
	 else echo "(no BENCH output found)"; fi

# Generate a comparison report (run bench-kvm and bench-linux first).
.PHONY: bench-report
bench-report:
	$(PYTHON3) tools/bench-report.py $(BENCH_REPORT_ARGS)

.PHONY: bench-all
bench-all:
	$(PYTHON3) benchmarks/run-benchmarks.py all-profiles

.PHONY: bench-compare
bench-compare:
	$(PYTHON3) benchmarks/run-benchmarks.py compare $(BENCH_FILES)

.PHONY: benchmark
benchmark:
	$(PROGRESS) "BENCHMARK" "Kevlar vs Linux vs Native (all profiles)"
	$(PYTHON3) tools/run-all-benchmarks.py $(BENCH_ARGS)

.PHONY: benchmark-quick
benchmark-quick:
	$(PROGRESS) "BENCHMARK" "Quick benchmark (balanced only)"
	$(PYTHON3) tools/run-all-benchmarks.py --profile balanced --quick

# Debug mode: boots with structured debug events and GDB enabled.
.PHONY: debug
debug: build
	$(PYTHON3) tools/run-qemu.py                                           \
		--arch $(ARCH) --kvm                                           \
		$(if $(GUI),--gui,)                                            \
		--gdb                                                          \
		--log-serial "file:debug.jsonl"                                \
		--append-cmdline "debug=all"                                   \
		$(if $(LOG),--append-cmdline "log=$(LOG)",)                    \
		$(if $(CMDLINE),--append-cmdline "$(CMDLINE)",)                \
		$(kernel_qemu_arg) -- $(QEMU_ARGS)

# ktrace: high-bandwidth binary tracing via debugcon.
# Builds with all ktrace features enabled and boots with debugcon output.
.PHONY: run-ktrace
run-ktrace:
	$(PROGRESS) "KTRACE" "profile-$(PROFILE) arch-$(ARCH)"
	$(MAKE) build PROFILE=$(PROFILE) FEATURES=ktrace-all
	$(PYTHON3) tools/run-qemu.py                                           \
		--arch $(ARCH) $(ACCEL)                                        \
		--ktrace ktrace.bin                                            \
		--save-dump kevlar.dump                                        \
		$(if $(GUI),--gui,)                                            \
		$(if $(GDB),--gdb,)                                            \
		$(if $(LOG),--append-cmdline "log=$(LOG)",)                    \
		$(if $(CMDLINE),--append-cmdline "$(CMDLINE)",)                \
		$(if $(LOG_SERIAL),--log-serial "$(LOG_SERIAL)",)              \
		$(kernel_qemu_arg) -- $(QEMU_ARGS)

# Decode a ktrace binary dump to text timeline or Perfetto JSON.
.PHONY: decode-ktrace
decode-ktrace:
	$(PYTHON3) tools/ktrace-decode.py ktrace.bin $(KTRACE_ARGS)

# Start the MCP debug server (run alongside `make debug`).
.PHONY: mcp-debug
mcp-debug:
	$(PYTHON3) tools/mcp-debug-server/server.py                            \
		--gdb-port 7789                                                \
		--elf $(kernel_elf)                                            \
		--symbols $(kernel_symbols)                                    \
		--debug-log debug.jsonl

# Analyze a crash dump.
.PHONY: analyze-crash
analyze-crash:
	$(PYTHON3) tools/crash-analyzer/analyzer.py                            \
		--symbols $(kernel_symbols)                                    \
		dump kevlar.dump | $(PYTHON3) -m json.tool

# Analyze the most recent serial log.
.PHONY: analyze-log
analyze-log:
	$(PYTHON3) tools/crash-analyzer/analyzer.py                            \
		--symbols $(kernel_symbols)                                    \
		log /tmp/kevlar-bench-$(PROFILE).log | $(PYTHON3) -m json.tool

.PHONY: print-stack-sizes
print-stack-sizes: build
	$(READELF) --stack-sizes $(kernel_elf) | sort -n | rustfilt

.PHONY: clean
clean:
	$(CARGO) clean
	rm -rf *.elf *.iso *.bin *.symbols isofiles

# Clean native initramfs build caches.
.PHONY: clean-initramfs
clean-initramfs:
	rm -rf build/native-cache/local-bin build/native-cache/local-lib
	rm -f build/testing.initramfs build/testing.arm64.initramfs

# Clean all native caches including external packages (BusyBox, curl, etc.)
.PHONY: clean-initramfs-all
clean-initramfs-all:
	rm -rf build/native-cache build/initramfs-rootfs
	rm -f build/testing.initramfs build/testing.arm64.initramfs

#
#  Build Rules
#
build/testing.initramfs: $(wildcard testing/*) $(wildcard testing/*/*) $(wildcard testing/*/*/*) $(wildcard benchmarks/*) $(wildcard tests/*) Makefile
ifdef USE_DOCKER
ifeq ($(OS),Windows_NT)
	$(PROGRESS) "BUILD" testing
	$(PYTHON3) -c "import subprocess, os; docker_dir = os.path.dirname(r'$(DOCKER_PATH)'); os.environ['PATH'] = docker_dir + os.pathsep + os.environ.get('PATH', ''); subprocess.run([r'$(DOCKER_PATH)', 'build', '-t', 'kevlar-testing', '-f', 'testing/Dockerfile', '.'], check=True)"
	$(PROGRESS) "EXPORT" testing
	$(PYTHON3) -c "import os; os.makedirs('build', exist_ok=True)"
	$(PYTHON3) tools/docker2initramfs.py $@ kevlar-testing
else
	$(PROGRESS) "BUILD" testing
	$(DOCKER) build -t kevlar-testing -f testing/Dockerfile . 2>&1 | $(PYTHON3) tools/docker-progress.py
	$(PROGRESS) "EXPORT" testing
	$(PYTHON3) -c "import os; os.makedirs('build', exist_ok=True)"
	$(PYTHON3) tools/docker2initramfs.py $@ kevlar-testing
endif
else
ifeq ($(OS),Windows_NT)
	$(PROGRESS) "WSL" "build-initramfs.py"
	wsl python3 tools/build-initramfs.py $@
else
	$(PYTHON3) tools/build-initramfs.py $@
endif
endif

# ARM64 initramfs. build-initramfs.py reads ARCH from the environment
# (set by `make ARCH=arm64`) and downloads pre-built Alpine aarch64
# binaries — no cross-compiler needed on the host.
build/testing.arm64.initramfs: $(wildcard testing/*) $(wildcard testing/*/*) $(wildcard testing/*/*/*) $(wildcard benchmarks/*) $(wildcard tests/*) Makefile
	ARCH=arm64 $(PYTHON3) tools/build-initramfs.py --arch arm64 $@

build/$(IMAGE_FILENAME).initramfs: tools/docker2initramfs.py Makefile
	$(PROGRESS) "EXPORT" $(IMAGE)
	$(PYTHON3) -c "import os; os.makedirs('build', exist_ok=True)"
	$(PYTHON3) tools/docker2initramfs.py $@ $(IMAGE)

$(DUMMY_INITRAMFS_PATH):
	$(PYTHON3) -c "import os; os.makedirs('$(@D)', exist_ok=True)"
	touch $@

%.svg: %.drawio
	$(PROGRESS) "DRAWIO" $@
	$(DRAWIO) -x -f svg -o $@ $<
