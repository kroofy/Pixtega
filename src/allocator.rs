//! glibc allocator policy for this workload. No-ops on other targets.
//!
//! glibc gives threads their own malloc arenas — by default up to eight per
//! core — and each arena keeps what was freed into it. Derivations run
//! across short-lived blocking threads and libvips workers, so left alone
//! the process holds one retained heap per thread that ever decoded an
//! image: 324 MB resident while idle after a burst, with 3 MB in use.
//!
//! [`tune`] bounds what the arenas keep while the service is under load.
//! [`release_free_memory`] gives the rest back once it is idle.

/// Tune the allocator. Call once at startup, before any other thread
/// exists: arenas that already exist are not taken back by the cap.
///
/// - One arena per derivation slot. Fewer made tiny derivations contend for
///   the allocator (-19% throughput with a single arena at eight
///   concurrent); more only added retained heaps.
/// - A fixed 32 MB trim threshold, so an arena hands its freed top back
///   above that and keeps typical decode scratch for the next request.
pub fn tune(max_concurrent_derivations: usize) {
    glibc::tune_malloc(max_concurrent_derivations);
}

/// Return every free heap page to the operating system.
///
/// This takes every arena lock in turn, and the next derivation faults its
/// scratch back in, so the HTTP layer calls it only once derivations have
/// gone quiet.
pub fn release_free_memory() {
    glibc::trim_heap();
}

/// One of the two places in the crate allowed to use `unsafe` (the other is
/// `processor::vips_ffi`): the glibc allocator calls, which have no safe
/// wrapper.
#[allow(unsafe_code)]
mod glibc {
    /// Cap glibc at `arenas` malloc arenas and fix its heap trim threshold
    /// at 32 MB.
    ///
    /// Left dynamic, the trim threshold ratchets upward in step with the
    /// mmap threshold every time a large mmap-backed block is freed, after
    /// which multi-MB source and decode buffers are carved from arenas that
    /// effectively never shrink. Setting any mallopt parameter disables
    /// that adjustment, which is what makes the fixed value stick
    /// (measured: peak RSS -12..-77% across the benchmark corpus at
    /// unchanged latency; see scripts/bench/).
    pub fn tune_malloc(arenas: usize) {
        // SAFETY: mallopt only sets process-global allocator tuning
        // parameters; it dereferences no pointers. Both parameters are
        // glibc-specific, which the linux-gnu cfg guarantees.
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        unsafe {
            libc::mallopt(
                libc::M_ARENA_MAX,
                arenas.try_into().unwrap_or(libc::c_int::MAX),
            );
            libc::mallopt(libc::M_TRIM_THRESHOLD, 32 * 1024 * 1024);
        }
        #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
        let _ = arenas;
    }

    /// Release every free page in every glibc arena back to the OS.
    pub fn trim_heap() {
        // SAFETY: malloc_trim only returns already-free allocator pages to
        // the kernel under the allocator's own locks; it dereferences no
        // caller pointers and invalidates no live allocation. It is
        // glibc-specific, which the linux-gnu cfg guarantees.
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        unsafe {
            libc::malloc_trim(0);
        }
    }
}

#[cfg(all(test, target_os = "linux", target_env = "gnu"))]
mod tests {
    use super::*;

    /// Resident set size of this process, in bytes.
    fn resident_bytes() -> usize {
        let status = std::fs::read_to_string("/proc/self/status").expect("read /proc/self/status");
        let kilobytes: usize = status
            .lines()
            .find_map(|line| line.strip_prefix("VmRSS:"))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|value| value.parse().ok())
            .expect("VmRSS line");
        kilobytes * 1024
    }

    /// Freed heap stays resident in the arena until it is asked back. The
    /// blocks are small enough to come from the arena rather than their own
    /// mappings, and the one kept alive sits above all the others, so
    /// freeing alone cannot shrink the heap from its top.
    #[test]
    fn release_free_memory_returns_freed_heap_to_the_os() {
        const BLOCK: usize = 32 * 1024;
        const BLOCKS: usize = 4096; // 128 MiB in total

        let mut blocks: Vec<Vec<u8>> = (0..BLOCKS).map(|_| vec![1u8; BLOCK]).collect();
        let top = blocks.pop().expect("at least one block");
        drop(blocks);
        let retained = resident_bytes();

        release_free_memory();
        let released = retained.saturating_sub(resident_bytes());
        drop(top);

        assert!(
            released >= BLOCK * BLOCKS / 2,
            "expected at least half of the freed heap back, got {released} bytes"
        );
    }
}
