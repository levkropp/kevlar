# M6.6 Phase 4: Buddy Allocator

**Duration:** ~2 days
**Goal:** Replace the bitmap allocator with a buddy allocator for O(1) single-page allocation.

## Why

The current bitmap allocator (`libs/kevlar_utils/bitmap_allocator.rs`)
finds free pages by scanning bytes: `(!byte).trailing_zeros()`.  On a
cache miss this costs ~50ns.  Linux's buddy allocator maintains per-order
free lists — allocating a single page is a list pop (O(1), ~5ns).

The page cache hides this for the first 64 allocs, but every 64th
allocation triggers a refill that scans the bitmap.  During mmap_fault
with 17 pages per fault and 256 faults, that's ~68 refills = ~3,400ns
of bitmap scanning overhead.

## Design

### Data structure

```
struct BuddyAllocator {
    // Free list heads for each order (0 = 4KB, 1 = 8KB, ..., MAX_ORDER = 4MB)
    free_lists: [FreeList; MAX_ORDER + 1],
    // Base physical address of the managed region
    base: usize,
    // Total pages
    num_pages: usize,
    // Bitmap for buddy status (1 bit per page pair at each order)
    buddy_bitmap: &'static mut [u8],
}
```

Each free page has its first 8 bytes used as a next-pointer for the
free list (page content doesn't matter since it will be zeroed on use).
This means **zero extra memory** for free list nodes — they're embedded
in the free pages themselves.

### API (must match existing BitMapAllocator)

```rust
pub fn new(base: *mut u8, base_paddr: usize, len: usize) -> BuddyAllocator
pub fn alloc_one(&mut self) -> Option<usize>       // O(1): pop from order-0 list
pub fn free_one(&mut self, ptr: usize)              // O(1): push + merge
pub fn alloc_pages(&mut self, order: usize) -> Option<usize>
pub fn free_pages(&mut self, ptr: usize, order: usize)
pub fn includes(&mut self, ptr: usize) -> bool
pub fn num_total_pages(&self) -> usize
```

### alloc_one fast path

```rust
fn alloc_one(&mut self) -> Option<usize> {
    // Try order-0 free list first
    if let Some(page) = self.free_lists[0].pop() {
        return Some(page);
    }
    // Split a higher-order block
    for order in 1..=MAX_ORDER {
        if let Some(block) = self.free_lists[order].pop() {
            // Split: put the second half on the lower-order free list
            let buddy = block + (PAGE_SIZE << (order - 1));
            self.free_lists[order - 1].push(buddy);
            // Recursively split if needed (or just return the first half)
            return Some(block);
        }
    }
    None
}
```

### free_one with coalescing

```rust
fn free_one(&mut self, ptr: usize) {
    let mut block = ptr;
    let mut order = 0;
    loop {
        let buddy = block ^ (PAGE_SIZE << order);
        if order >= MAX_ORDER || !self.is_buddy_free(buddy, order) {
            break;
        }
        // Buddy is free — remove it from its free list, merge
        self.free_lists[order].remove(buddy);
        block = block.min(buddy);
        order += 1;
    }
    self.free_lists[order].push(block);
}
```

## Implementation plan

1. Create `libs/kevlar_utils/buddy_allocator.rs` (new file, ~200 lines)
2. Implement FreeList (intrusive linked list using page content)
3. Implement BuddyAllocator with the existing API
4. In `platform/page_allocator.rs`, change the import:
   `use kevlar_utils::buddy_allocator::BuddyAllocator as Allocator;`
5. Build and test — the rest of the code uses the Allocator trait

## Testing

- `make test-contracts-vm` — 8/8 VM tests (demand paging, fork CoW)
- `make bench-kvm` — mmap_fault should improve by ~90ns/fault
- `make test-threads-smp` — 14/14 threading tests (alloc under contention)

## Risk

Buddy allocator bugs cause silent memory corruption (double-alloc,
use-after-free).  Test extensively with page poisoning enabled.
