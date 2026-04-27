# Linux-on-HVF harness for the ASID / HVF investigation.
#
# Boots a minimal Linux kernel under QEMU with HVF acceleration and runs
# the bench binary as /init.  Used to ground-truth "what does Linux
# actually do on this hypervisor" questions.
#
# Targets:
#   fork_exit    — 5 runs, print each.  Expect ~15 us/iter on Apple Silicon.
#   bench        — full suite, save to baseline.txt.
#   linux-src    — clone Linux v6.12 into build/linux-src (git-ignored).
#   image        — build Image from linux-src.
#   initramfs    — (re)pack bench as /init.
#   clean        — remove build outputs (keep downloaded source).
#
# Defaults use the pre-built Alpine arm64 kernel under build/linux-arm64/
# and the existing bench-on-cpio initramfs in /tmp, so fork_exit and
# bench work without running `image` first.  Override KERNEL / INITRAMFS
# on the command line to test a custom-built kernel:
#
#   make fork_exit KERNEL=build/linux-src/arch/arm64/boot/Image
#
# Linux v6.12 was picked because arch/arm64/mm/context.c::cpu_do_switch_mm
# has been stable since 6.9.  Any 6.9+ works for the investigation.

LINUX_TAG      ?= v6.12
LINUX_SRC_DIR  ?= ../../build/linux-src
LINUX_BUILD    ?= $(LINUX_SRC_DIR)/arch/arm64/boot/Image

# Defaults: use the pre-built Alpine kernel + the bench-as-init initramfs.
KERNEL         ?= ../../build/linux-arm64/vmlinuz-virt
# Use Kevlar's full initramfs (includes busybox + applet symlinks) so
# Linux runs the SAME test environment — otherwise benches like
# tar_extract, sort_uniq, sed_pipeline just exec-fail-fast on Linux
# because /bin/sh doesn't exist there, making the comparison
# meaningless.
INITRAMFS      ?= /tmp/kevlar-rootfs.cpio.gz
BENCH_SRC      ?= ../../benchmarks/bench.c
BENCH_BIN      ?= /tmp/linux-initramfs/bin/bench
KEVLAR_CPIO    ?= ../../build/testing.arm64.initramfs

CROSS_CC       ?= aarch64-linux-musl-gcc
QEMU           ?= qemu-system-aarch64
QEMU_ARGS      = -machine virt,accel=hvf -cpu host -m 1024 -mem-prealloc \
                 -kernel $(KERNEL) -initrd $(INITRAMFS) \
                 -append "console=ttyAMA0 panic=1 rdinit=/bin/bench quiet -- --full" \
                 -nographic -no-reboot

# Poll the output for completion; HVF reliably finishes in a couple of
# seconds but we give 30 s of headroom.
RUN_SCRIPT     = \
  ($(QEMU) $(QEMU_ARGS) > $(1) 2>&1 & PID=$$!; \
   for j in $$(seq 1 30); do \
     grep -qE '^BENCH_END|panic' $(1) 2>/dev/null && break; \
     sleep 1; \
   done; \
   kill $$PID 2>/dev/null; sleep 1; kill -9 $$PID 2>/dev/null; wait 2>/dev/null || true)

.PHONY: initramfs
initramfs: $(KEVLAR_CPIO)
	@echo "  GZIP    $(KEVLAR_CPIO) -> /tmp/kevlar-rootfs.cpio.gz"
	@gzip -c $(KEVLAR_CPIO) > /tmp/kevlar-rootfs.cpio.gz
	@ls -la /tmp/kevlar-rootfs.cpio.gz

.PHONY: fork_exit
fork_exit: initramfs
	@for i in 1 2 3 4 5; do \
	  $(call RUN_SCRIPT,/tmp/linux-hvf-fork_exit-$$i.log); \
	  grep '^BENCH fork_exit' /tmp/linux-hvf-fork_exit-$$i.log | head -1; \
	done

BASELINE_OUT   ?= ../../Documentation/optimization/linux-on-hvf-baseline.txt

.PHONY: bench
bench: initramfs
	@$(call RUN_SCRIPT,/tmp/linux-hvf-bench.log)
	@mkdir -p $(dir $(BASELINE_OUT))
	@echo "# Linux-on-HVF baseline ($(shell date -u +%Y-%m-%dT%H:%M:%SZ))" \
	   > $(BASELINE_OUT)
	@echo "# kernel: $(KERNEL)" >> $(BASELINE_OUT)
	@echo "# initramfs: $(INITRAMFS)" >> $(BASELINE_OUT)
	@grep -E '^(BENCH|BENCH_EXTRA|BENCH_START|BENCH_MODE|BENCH_END)' \
	   /tmp/linux-hvf-bench.log >> $(BASELINE_OUT) || true
	@echo "# saved to $(BASELINE_OUT)"
	@tail -20 $(BASELINE_OUT)

# Clone Linux at a known-good tag.  Shallow clone (~250 MB, vs ~3 GB full
# history) — enough for the investigation since we won't bisect into
# kernel history.
.PHONY: linux-src
linux-src:
	@if [ ! -d $(LINUX_SRC_DIR) ]; then \
	  echo "  CLONE   linux $(LINUX_TAG) -> $(LINUX_SRC_DIR)"; \
	  mkdir -p $(dir $(LINUX_SRC_DIR)); \
	  git clone --depth=1 --branch $(LINUX_TAG) \
	    https://github.com/torvalds/linux.git $(LINUX_SRC_DIR); \
	else \
	  echo "  linux source already at $(LINUX_SRC_DIR)"; \
	fi

# Build arm64 Image from linux-src.  Uses defconfig stripped to the
# minimum plus what HVF+virt boot needs.  CROSS_COMPILE points at the
# Homebrew musl toolchain (aarch64-linux-musl-gcc).  On macOS this
# requires a case-sensitive volume — if you see "attempt to invoke
# directive when it is disabled" or "file exists" errors, rerun the
# build on a case-sensitive APFS volume.  (See linux-src/README.)
.PHONY: image
image: linux-src
	@echo "  CONFIG  defconfig + minimal tweaks"
	$(MAKE) -C $(LINUX_SRC_DIR) ARCH=arm64 \
	    CROSS_COMPILE=aarch64-linux-musl- defconfig
	@echo "  BUILD   arm64 Image"
	$(MAKE) -C $(LINUX_SRC_DIR) ARCH=arm64 \
	    CROSS_COMPILE=aarch64-linux-musl- -j$$(sysctl -n hw.ncpu) Image

# Cross-compile bench.c for aarch64-linux-musl (statically linked so
# nothing else is needed in the initramfs).  Kept as a separate target
# so the kernel build + bench build can be done independently.
.PHONY: bench-bin
bench-bin: $(BENCH_BIN)

$(BENCH_BIN): $(BENCH_SRC)
	@mkdir -p $(dir $(BENCH_BIN))
	@echo "  CC      bench (aarch64-linux-musl)"
	$(CROSS_CC) -O2 -static -o $(BENCH_BIN) $(BENCH_SRC)

# Rebuild the cpio initramfs from the current bench binary.  /init is a
# symlink to /bin/bench so the kernel executes it on boot.
.PHONY: initramfs
initramfs: $(BENCH_BIN)
	@echo "  CPIO    linux-initramfs.gz"
	@cd /tmp/linux-initramfs && \
	  find . | cpio -H newc -o 2>/dev/null | gzip > /tmp/linux-initramfs.gz
	@ls -la /tmp/linux-initramfs.gz

# ─── Per-program LXDE baseline (Phase 3) ────────────────────────────
#
# `make lxde-program PROG=<name>` boots Alpine arm64's prebuilt
# `linux-virt` kernel against the SAME alpine-lxde rootfs Kevlar
# uses (extracted into a cpio.gz by tools/build-alpine-cpio.py),
# then runs `/bin/test-lxde-program` as init with
# `kevlar-prog=<name> kevlar-no-mount=1` on the cmdline.  The
# program harness behaves identically to the Kevlar path — the
# only difference is the kernel.  Diff against
# /tmp/kevlar-iterate-program-<name>-arm64.log to find Kevlar bugs.

LXDE_DISK      ?= ../../build/alpine-lxde.arm64.img
LXDE_CPIO      ?= ../../build/alpine-lxde.arm64.cpio.gz
LXDE_LOG       ?= /tmp/linux-iterate-program-$(PROG).log

# Build the cpio on-demand if older than the disk.
$(LXDE_CPIO): $(LXDE_DISK)
	@$(PYTHON3) ../../tools/build-alpine-cpio.py \
	    $(LXDE_DISK) $(LXDE_CPIO)
PYTHON3 ?= python3

LXDE_QEMU_ARGS = -machine virt,accel=hvf -cpu host -m 2048 -mem-prealloc \
                 -kernel $(KERNEL) -initrd $(LXDE_CPIO) \
                 -device virtio-gpu-pci,xres=1024,yres=768 \
                 -device virtio-keyboard-device,event_idx=off,indirect_desc=off \
                 -device virtio-mouse-device,event_idx=off,indirect_desc=off \
                 -append "console=ttyAMA0 panic=1 \
                          rdinit=/bin/test-lxde-program \
                          kevlar-prog=$(PROG) \
                          kevlar-no-mount=1 \
                          $(if $(PROG_ARGS),kevlar-prog-args=$(PROG_ARGS),)" \
                 -nographic -no-reboot

# Poll for TEST_END (the harness's terminator) or a kernel panic.
# 240s budget — Linux kernel + 350MB initramfs decompress + full
# LXDE bring-up + per-program phase can take a minute under HVF.
#
# Note: anchor patterns to the start of line.  Earlier the regex was
# `panic|kernel panic` which matched `panic=1` on the kernel cmdline
# echo line and broke the loop immediately.
LXDE_RUN_SCRIPT = \
  ($(QEMU) $(LXDE_QEMU_ARGS) > $(LXDE_LOG) 2>&1 & PID=$$!; \
   for j in $$(seq 1 240); do \
     grep -qE '^TEST_END|^Kernel panic|^\[.*Kernel panic' $(LXDE_LOG) 2>/dev/null && break; \
     sleep 1; \
   done; \
   kill $$PID 2>/dev/null; sleep 1; kill -9 $$PID 2>/dev/null; \
   wait 2>/dev/null || true)

.PHONY: lxde-program
lxde-program: $(LXDE_CPIO)
	@if [ -z "$(PROG)" ]; then \
	  echo "Usage: make lxde-program PROG=<name>"; exit 1; fi
	@echo "  RUN     linux-on-hvf lxde-program PROG=$(PROG)"
	@$(LXDE_RUN_SCRIPT)
	@echo ""
	@grep -E '^(TEST_PASS|TEST_FAIL|TEST_SKIP|TEST_END)' $(LXDE_LOG) \
	    || echo "(no test output — see $(LXDE_LOG))"

.PHONY: clean
clean:
	rm -f /tmp/linux-hvf-*.log
	rm -f /tmp/linux-iterate-program-*.log
	rm -f Documentation/optimization/linux-on-hvf-baseline.txt
