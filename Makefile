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
    LLVM_BIN_DIR := $(shell rustc --print sysroot 2>/dev/null || echo "")/lib/rustlib/x86_64-unknown-linux-gnu/bin
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
export RUSTFLAGS = -Z emit-stack-sizes
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

# `make run-alpine` — Boot with Alpine disk, auto-mount, interactive shell
.PHONY: run-alpine
run-alpine: build alpine-disk
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

.PHONY: test-alpine
test-alpine: alpine-disk
	$(PROGRESS) "TEST" "Alpine apk integration (mount + update)"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/test_apk_update.sh"
	timeout 120 $(PYTHON3) tools/run-qemu.py \
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

# M10 Phase A: apk add integration test
.PHONY: test-m10-apk
test-m10-apk: alpine-disk
	$(PROGRESS) "TEST" "M10 apk add integration (mount + update + install)"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/test_m10_apk.sh"
	timeout 180 $(PYTHON3) tools/run-qemu.py \
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
	timeout 120 $(PYTHON3) tools/run-qemu.py \
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
	timeout 120 $(PYTHON3) tools/run-qemu.py \
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
	timeout 120 $(PYTHON3) tools/run-qemu.py \
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
	timeout 120 $(PYTHON3) tools/run-qemu.py \
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
	timeout 120 $(PYTHON3) tools/run-qemu.py \
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
	timeout 120 $(PYTHON3) tools/run-qemu.py \
		--kvm --arch $(ARCH) $(kernel_qemu_arg) -- -mem-prealloc -smp 4 2>&1 \
		| tee /tmp/kevlar-test-smp-$(PROFILE).log; true
	@grep -E 'CPU \(LAPIC|smp:|online' /tmp/kevlar-test-smp-$(PROFILE).log || echo "(no SMP output found)"

# Run M4 integration suite (mini_systemd) under -smp 4 as a regression check.
.PHONY: test-regression-smp
test-regression-smp:
	$(PROGRESS) "TEST" "M6 Phase 5 regression: mini_systemd on 4 CPUs"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/mini-systemd"
	timeout 120 $(PYTHON3) tools/run-qemu.py \
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
	timeout 60 $(PYTHON3) tools/run-qemu.py \
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
	timeout 180 $(PYTHON3) tools/run-qemu.py \
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
	timeout 120 $(PYTHON3) tools/run-qemu.py \
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
	timeout 60 $(PYTHON3) tools/run-qemu.py \
		--kvm --arch $(ARCH) $(kernel_qemu_arg) \
		-- -mem-prealloc -append "pci=off init=/usr/lib/systemd/systemd" 2>&1 \
		| tee /tmp/kevlar-test-systemd-boot.log; true
	@grep -aE 'systemd|Reached|target|Started|Failed|exited' \
		/tmp/kevlar-test-systemd-boot.log || echo "(no systemd output found)"

.PHONY: test-systemd-v3
test-systemd-v3:
	$(PROGRESS) "TEST" "M9 systemd init-sequence (25 tests)"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/mini-systemd-v3"
	timeout 120 $(PYTHON3) tools/run-qemu.py \
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
	timeout 180 $(PYTHON3) tools/run-qemu.py \
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
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/busybox-suite"
	timeout 120 $(PYTHON3) tools/run-qemu.py \
		--kvm --arch $(ARCH) $(kernel_qemu_arg) -- -mem-prealloc 2>&1 \
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

.PHONY: test-busybox-smp
test-busybox-smp:
	$(PROGRESS) "TEST" "BusyBox applet suite (4 CPUs)"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/busybox-suite"
	timeout 300 $(PYTHON3) tools/run-qemu.py \
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
	timeout 300 $(PYTHON3) tools/run-qemu.py \
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
	timeout 300 $(PYTHON3) tools/run-qemu.py \
		--kvm --arch $(ARCH) $(kernel_qemu_arg) -- -mem-prealloc 2>&1 \
		| tee /tmp/kevlar-bench-workloads-$(PROFILE).log; true
	@grep '^BENCH ' /tmp/kevlar-bench-workloads-$(PROFILE).log || echo "(no BENCH output found)"

# dd diagnostic: find exact block size / count where dd hangs
.PHONY: test-busybox-dd
test-busybox-dd:
	$(PROGRESS) "TEST" "dd diagnostic (KVM)"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/dd-diag"
	timeout 30 $(PYTHON3) tools/run-qemu.py \
		--kvm --arch $(ARCH) $(kernel_qemu_arg) -- -mem-prealloc 2>&1 \
		| tee /tmp/kevlar-test-dd-diag.log; true
	@grep -E '^\s+(OK|FAIL|HANG)|^Phase|^===' /tmp/kevlar-test-dd-diag.log || echo "(no output)"

# BusyBox workload benchmarks: real BusyBox operations under KVM
.PHONY: bench-busybox
bench-busybox:
	$(PROGRESS) "BENCH" "BusyBox workload benchmarks (KVM)"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/busybox-suite --bench-only --full"
	timeout 30 $(PYTHON3) tools/run-qemu.py \
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
	timeout 120 $(PYTHON3) tools/run-qemu.py \
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
	timeout 120 $(PYTHON3) tools/run-qemu.py \
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
