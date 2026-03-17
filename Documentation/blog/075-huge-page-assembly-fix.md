# M9.8.1: Fixing the Huge Page Assembly Corruption

The huge page assembly path was disabled (`ASSEMBLE_THRESHOLD=600`) due
to SIGSEGV crashes after ~100 fork+exec iterations. This session diagnosed
the root cause, fixed it, re-enabled assembly, and added verification
tooling.

## Results

Assembly re-enabled at threshold=128. All tests pass:
- BusyBox suite: **134/134** (was 64/100 with assembly, 101/101 without)
- fork_exec_stress: **300/300** with kwab-verify content checking
- Both default and Fortress profiles compile clean
- Zero verification failures across all execs

## The investigation

### Why the crash appeared at ~PID 130, not immediately

The crash was never about iteration count. The assembly threshold
(`ASSEMBLE_THRESHOLD=128`) requires 128+ cached 4KB pages before
assembling a huge page. Each BusyBox shell invocation touches ~20-30
unique pages. Around test 65 (~PID 130), the 4KB PAGE_CACHE
accumulates enough entries. The next exec triggers assembly for the
first time, the assembled page has corrupt content, and every
subsequent exec reuses the corrupted cached huge page.

This explains why `fork_exec_stress` (300x `/bin/true`) always passed:
`/bin/true` exits immediately, touching only ~20 pages per exec, never
crossing the 128-page threshold.

Setting `ASSEMBLE_THRESHOLD=0` to force immediate assembly confirmed
this: PID 2 (the very first BusyBox exec) crashed.

### Bug 1: Full-page cache copy on boundary pages

The assembly loop has two sub-page population paths:
1. **Cache HIT**: copy from the 4KB PAGE_CACHE
2. **Cache MISS**: read from the backing file

For boundary pages (where a VMA starts mid-page, e.g. `.data` at
`0x51cbf0` within sub-page `0x51c000`), `offset_in_page = 0xbf0`.
The gap portion `[0..0xbf0)` must stay zero (anonymous gap VMA).

The cache-hit path did `dst.copy_from_slice(src)` — a full 4KB copy
that overwrote the zero gap with file content. The first diagnostic
caught this:

```
huge_page_verify_fail: sub_page=284, first_diff=0,
  expected=0x00, actual=0x65
```

Byte 0 should be zero (anonymous gap) but had file content (0x65).

### Bug 2: PAGE_CACHE index collision between VMAs

After fixing bug 1, the verifier caught a subtler issue:

```
huge_page_verify_fail: sub_page=284, first_diff=3056,
  expected=0xc0, actual=0x00
```

Byte 0xBF0 should have `.data` content (0xC0) but was zero. The
boundary page's `page_index = file_offset / PAGE_SIZE` (0x11b)
collided with `.rodata`'s last page at the same index. That `.rodata`
page was only partially filled (0x1f0 bytes of content, rest zeros)
and cached by the demand fault handler. The assembly got a cache hit
on this partial `.rodata` page, reading zeros where `.data` content
should be.

## The fix

Restrict cache usage to full, page-aligned sub-pages only:

```rust
let use_cache = offset_in_page == 0 && copy_len == PAGE_SIZE;
if use_cache {
    if let Some(&src) = cache_map.get(&(file_ptr, page_index)) {
        dst.copy_from_slice(src);  // Safe: full page, no boundary
        break;
    }
}
// Cache miss or partial/boundary page: always read from file
file.read(file_offset, &mut dst[offset_in_page..offset_in_page+copy_len]);
```

This eliminates both bugs:
- Boundary pages always take the file-read path (correct partial writes)
- No index collision risk (partial pages are never served from cache)

The performance impact is minimal: boundary pages are rare (~2 per
binary), and file reads from initramfs are fast (in-memory).

## Verification tooling added

**`verify_huge_page_assembly()`** — runs after each assembly when
`debug=kwab-verify` is enabled. For each populated sub-page, reads
expected content from the file (ground truth) and compares byte-by-byte.
Emits `HugePageVerifyFail` events with sub-page index, first differing
byte, expected/actual values, and covering VMA info.

**`HugePageVerifyFail` debug event** — new JSONL event type for
structured diagnostics of assembly content mismatches.

**`fork_exec_stress` test binary** — 300 fork+exec+wait iterations
with exit status checking. Integrated into `make test-huge-page`.

## Files changed

| File | Change |
|------|--------|
| `kernel/process/process.rs` | Fixed cache-hit path (partial copy + cache restriction), re-enabled threshold=128, added verify function |
| `kernel/debug/event.rs` | Added `HugePageVerifyFail` event variant |
| `testing/fork_exec_stress.c` | New stress test binary |
| `tools/build-initramfs.py` | Added fork_exec_stress to build |
| `Makefile` | Added `test-huge-page` target |
