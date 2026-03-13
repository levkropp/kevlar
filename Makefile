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

# Fortress and Balanced use the unwind target spec for catch_unwind support.
# Performance and Ludicrous use the abort target spec.
ifeq ($(filter $(PROFILE),fortress balanced),$(PROFILE))
ifeq ($(ARCH),x64)
target_json := kernel/arch/$(ARCH)/$(ARCH)-unwind.json
else
target_json := kernel/arch/$(ARCH)/$(ARCH).json
endif
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

export RUSTFLAGS = -Z emit-stack-sizes
CARGOFLAGS += -Z build-std=core,alloc
CARGOFLAGS += -Z json-target-spec
CARGOFLAGS += --target $(target_json)
CARGOFLAGS += $(if $(RELEASE),--release,)
CARGOFLAGS += --no-default-features --features profile-$(PROFILE)
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
	$(CARGO) build $(CARGOFLAGS) --manifest-path kernel/Cargo.toml

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
		--arch $(ARCH)                                                 \
		--save-dump kevlar.dump                                        \
		$(if $(GUI),--gui,)                                            \
		$(if $(KVM),--kvm,)                                            \
		$(if $(GDB),--gdb,)                                            \
		$(if $(LOG),--append-cmdline "log=$(LOG)",)                    \
		$(if $(CMDLINE),--append-cmdline "$(CMDLINE)",)                \
		$(if $(LOG_SERIAL),--log-serial "$(LOG_SERIAL)",)              \
		$(if $(QEMU),--qemu $(QEMU),)                                  \
		$(kernel_qemu_arg) -- $(QEMU_ARGS)

.PHONY: disk
disk: build/disk.img

build/disk.img: testing/Dockerfile testing/disk_hello.c
	$(PROGRESS) "MKDISK" build/disk.img
	$(PYTHON3) -c "import os; os.makedirs('build', exist_ok=True)"
	$(DOCKER) build --target disk_image -t kevlar-disk-image -f testing/Dockerfile .
	$(DOCKER) create --name kevlar-disk-tmp kevlar-disk-image
	$(DOCKER) cp kevlar-disk-tmp:/disk.img build/disk.img
	$(DOCKER) rm kevlar-disk-tmp

.PHONY: run-disk
run-disk: build disk
	$(PYTHON3) tools/run-qemu.py                                           \
		--arch $(ARCH)                                                 \
		--disk build/disk.img                                          \
		$(if $(GUI),--gui,)                                            \
		$(if $(KVM),--kvm,)                                            \
		$(if $(GDB),--gdb,)                                            \
		$(if $(LOG),--append-cmdline "log=$(LOG)",)                    \
		$(if $(CMDLINE),--append-cmdline "$(CMDLINE)",)                \
		$(if $(LOG_SERIAL),--log-serial "$(LOG_SERIAL)",)              \
		$(if $(QEMU),--qemu $(QEMU),)                                  \
		$(kernel_qemu_arg) -- $(QEMU_ARGS)

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
		--arch $(ARCH) --disk build/disk.img $(kernel_qemu_arg) 2>&1 \
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
		--arch $(ARCH) --disk build/disk.img $(kernel_qemu_arg) 2>&1 \
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
		--arch $(ARCH) --disk build/disk.img $(kernel_qemu_arg) 2>&1 \
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
		--arch $(ARCH) $(kernel_qemu_arg) 2>&1 \
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
		--arch $(ARCH) $(kernel_qemu_arg) -- -smp 4 2>&1 \
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
		--arch $(ARCH) $(kernel_qemu_arg) -- -smp 4 2>&1 \
		| tee /tmp/kevlar-test-smp-$(PROFILE).log; true
	@grep -E 'CPU \(LAPIC|smp:|online' /tmp/kevlar-test-smp-$(PROFILE).log || echo "(no SMP output found)"

# Run M4 integration suite (mini_systemd) under -smp 4 as a regression check.
.PHONY: test-regression-smp
test-regression-smp:
	$(PROGRESS) "TEST" "M6 Phase 5 regression: mini_systemd on 4 CPUs"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/mini-systemd"
	timeout 120 $(PYTHON3) tools/run-qemu.py \
		--arch $(ARCH) $(kernel_qemu_arg) -- -smp 4 2>&1 \
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

.PHONY: bench
bench:
	$(PROGRESS) "BENCH" "profile-$(PROFILE)"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/bench"
	timeout 120 $(PYTHON3) tools/run-qemu.py \
		--arch $(ARCH) $(kernel_qemu_arg) 2>&1 \
		| tee /tmp/kevlar-bench-$(PROFILE).log; true
	@grep 'BENCH' /tmp/kevlar-bench-$(PROFILE).log || echo "(no BENCH output found)"

.PHONY: bench-kvm
bench-kvm:
	$(PROGRESS) "BENCH-KVM" "profile-$(PROFILE)"
	$(MAKE) build PROFILE=$(PROFILE) INIT_SCRIPT="/bin/bench --full"
	timeout 120 $(PYTHON3) tools/run-qemu.py \
		--kvm --arch $(ARCH) $(kernel_qemu_arg) -- -mem-prealloc 2>&1 \
		| tee /tmp/kevlar-bench-kvm-$(PROFILE).log; true
	@grep 'BENCH' /tmp/kevlar-bench-kvm-$(PROFILE).log || echo "(no BENCH output found)"

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
		--arch $(ARCH)                                                 \
		$(if $(GUI),--gui,)                                            \
		$(if $(KVM),--kvm,)                                            \
		--gdb                                                          \
		--log-serial "file:debug.jsonl"                                \
		--append-cmdline "debug=all"                                   \
		$(if $(LOG),--append-cmdline "log=$(LOG)",)                    \
		$(if $(CMDLINE),--append-cmdline "$(CMDLINE)",)                \
		$(kernel_qemu_arg) -- $(QEMU_ARGS)

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

#
#  Build Rules
#
build/testing.initramfs: $(wildcard testing/*) $(wildcard testing/*/*) $(wildcard benchmarks/*) $(wildcard tests/*) Makefile
	$(PROGRESS) "BUILD" testing
ifeq ($(OS),Windows_NT)
	$(PYTHON3) -c "import subprocess, os; docker_dir = os.path.dirname(r'$(DOCKER_PATH)'); os.environ['PATH'] = docker_dir + os.pathsep + os.environ.get('PATH', ''); subprocess.run([r'$(DOCKER_PATH)', 'build', '-t', 'kevlar-testing', '-f', 'testing/Dockerfile', '.'], check=True)"
else
	$(DOCKER) build -t kevlar-testing -f testing/Dockerfile .
endif
	$(PROGRESS) "EXPORT" testing
	$(PYTHON3) -c "import os; os.makedirs('build', exist_ok=True)"
	$(PYTHON3) tools/docker2initramfs.py $@ kevlar-testing

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
