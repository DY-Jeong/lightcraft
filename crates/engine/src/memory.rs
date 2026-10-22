//! Memory accounting: what each cache holds (`library.memory`, and `ui.inspect` → `memory` with the
//! frontend's own caches added).
//!
//! The numbers are the caches' own bookkeeping (pixels held), not the process's resident size: the
//! allocator keeps freed pages for reuse and the GPU driver maps device buffers, so `ps`/`time -l`
//! report more. A binary built with a heap profiler installs [`set_heap_stats`] (e.g.
//! `lightcraft-cli` with `--features dhat-heap`) to add live/peak heap bytes.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};

use serde::Serialize;

/// The memory budget: one number from which every cache's share is derived.
///
/// - half for the engine's caches (decoded sources and rendered previews), evicted least recently
///   used *across* them ([`crate::media::MediaCache`]);
/// - a quarter for the working memory of decodes in flight ([`work_gate`]);
/// - an eighth for the GPU renderer's pool of recycled buffers (trimmed when the app is idle).
///
/// The rest is headroom for what isn't cached (renders in progress, textures, the UI). An export
/// batch may, in addition, hold up to twice the budget in photos rendering side by side
/// ([`export_gate`]): transient memory, given back when the batch ends. Default:
/// a quarter of the machine's RAM, at most 1.5 GiB (`LIGHTCRAFT_MEMORY_MB` overrides it).
pub fn default_budget() -> usize {
    if let Some(mb) = std::env::var("LIGHTCRAFT_MEMORY_MB").ok().and_then(|v| v.trim().parse::<usize>().ok()).filter(|m| *m >= 64) {
        return mb << 20;
    }
    let cap = 3usize << 29; // 1.5 GiB
    match total_ram() {
        Some(ram) => (ram / 4).clamp(256 << 20, cap),
        None => cap,
    }
}

/// Physical memory (bytes), when the platform tells us without native calls.
pub(crate) fn total_ram() -> Option<usize> {
    #[cfg(target_os = "linux")]
    {
        let s = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kb: usize = s.lines().find(|l| l.starts_with("MemTotal:"))?.split_whitespace().nth(1)?.parse().ok()?;
        Some(kb * 1024)
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("/usr/sbin/sysctl").args(["-n", "hw.memsize"]).output().ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    }
    #[cfg(target_os = "freebsd")]
    {
        let out = std::process::Command::new("/sbin/sysctl").args(["-n", "hw.physmem"]).output().ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "freebsd")))]
    {
        None
    }
}

static BUDGET: AtomicUsize = AtomicUsize::new(0);

/// The current budget (bytes).
pub fn budget() -> usize {
    match BUDGET.load(Ordering::Relaxed) {
        0 => {
            let b = default_budget();
            BUDGET.store(b, Ordering::Relaxed);
            apply(b);
            b
        }
        b => b,
    }
}

/// Change the budget (bytes, at least 64 MiB) for the process: the decode gate and the GPU pool
/// follow at once; sessions apply their cache share with [`crate::Session::set_memory_budget`].
pub fn set_budget(bytes: usize) -> usize {
    let b = bytes.max(64 << 20);
    BUDGET.store(b, Ordering::Relaxed);
    apply(b);
    b
}

fn apply(b: usize) {
    if let Some(g) = GATE.get() {
        g.set_limit(b / 4);
    }
    if let Some(g) = EXPORT_GATE.get() {
        g.set_limit(export_share(b));
    }
    if let Some(g) = IMPORT_GATE.get() {
        g.set_limit(b / 8);
    }
    lightcraft_gpu::set_pool_limit((b / 8) as u64);
}

/// Share of the budget for the engine's caches.
pub fn cache_share(budget: usize) -> usize {
    budget / 2
}

/// A weighted semaphore over bytes of working memory: [`WorkGate::acquire`] waits while other
/// holders already use the limit (one holder may always proceed, whatever its size).
pub struct WorkGate {
    state: Mutex<GateState>,
    cv: Condvar,
}

struct GateState {
    bytes: usize,
    limit: usize,
    next: u64,
    queue: std::collections::VecDeque<u64>,
}

impl GateState {
    fn enqueue(&mut self) -> u64 {
        let ticket = self.next;
        self.next = self.next.wrapping_add(1);
        self.queue.push_back(ticket);
        ticket
    }

    fn ready(&self, ticket: u64, bytes: usize) -> bool {
        self.queue.front() == Some(&ticket) && (self.bytes == 0 || self.bytes.saturating_add(bytes) <= self.limit)
    }
}

/// Bytes held from a [`WorkGate`] until dropped.
pub struct Permit<'a> {
    gate: &'a WorkGate,
    bytes: usize,
}

impl WorkGate {
    pub fn new(limit: usize) -> WorkGate {
        WorkGate { state: Mutex::new(GateState { bytes: 0, limit, next: 0, queue: Default::default() }), cv: Condvar::new() }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, GateState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set_limit(&self, limit: usize) {
        self.lock().limit = limit;
        self.cv.notify_all();
    }

    pub fn usage(&self) -> (usize, usize) {
        let g = self.lock();
        (g.bytes, g.limit)
    }

    pub fn waiting(&self) -> usize {
        self.lock().queue.len()
    }

    /// FIFO admission: repeated small headers cannot starve a larger preview or transfer.
    /// A single holder may exceed the budget, as before, but must wait for existing work.
    pub fn acquire(&self, bytes: usize) -> Permit<'_> {
        let mut g = self.lock();
        let ticket = g.enqueue();
        while !g.ready(ticket, bytes) {
            g = self.cv.wait(g).unwrap_or_else(|e| e.into_inner());
        }
        g.queue.pop_front();
        g.bytes = g.bytes.saturating_add(bytes);
        self.cv.notify_all();
        Permit { gate: self, bytes }
    }

    /// Timed waits notice cancellation even if its owner cannot notify this gate.
    pub fn acquire_cancel(&self, bytes: usize, cancel: &std::sync::atomic::AtomicBool) -> Option<Permit<'_>> {
        let started = web_time::Instant::now();
        let mut g = self.lock();
        let ticket = g.enqueue();
        loop {
            if cancel.load(Ordering::Relaxed) {
                g.queue.retain(|&waiting| waiting != ticket);
                self.cv.notify_all();
                return None;
            }
            if g.ready(ticket, bytes) {
                break;
            }
            let (next, _) = self.cv.wait_timeout(g, std::time::Duration::from_millis(50)).unwrap_or_else(|e| e.into_inner());
            g = next;
        }
        g.queue.pop_front();
        if started.elapsed().as_secs() >= 1 {
            log::debug!("import memory: waited {:?} for {bytes} bytes", started.elapsed());
        }
        g.bytes = g.bytes.saturating_add(bytes);
        self.cv.notify_all();
        Some(Permit { gate: self, bytes })
    }

    /// Interactive decodes keep their nonblocking path, including nested Rayon face work.
    pub fn acquire_urgent(&self, bytes: usize) -> Permit<'_> {
        let mut g = self.lock();
        g.bytes = g.bytes.saturating_add(bytes);
        Permit { gate: self, bytes }
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        let mut g = self.gate.lock();
        g.bytes = g.bytes.saturating_sub(self.bytes);
        drop(g);
        self.gate.cv.notify_all();
    }
}

thread_local! {
    static BACKGROUND: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `f` as background work (grid thumbnails, neighbour prefetch): its decodes wait at the
/// [`work_gate`] while interactive work holds the memory.
pub fn in_background<R>(f: impl FnOnce() -> R) -> R {
    let was = BACKGROUND.with(|b| b.replace(true));
    let r = f();
    BACKGROUND.with(|b| b.set(was));
    r
}

/// Is this thread running background work ([`in_background`])?
pub fn is_background() -> bool {
    BACKGROUND.with(|b| b.get())
}

/// The process-wide gate for decodes and import probes (limit: a quarter of the budget).
pub fn work_gate() -> &'static WorkGate {
    GATE.get_or_init(|| WorkGate::new(budget() / 4))
}

static GATE: OnceLock<WorkGate> = OnceLock::new();

/// Import I/O has reserved working space, so face decodes cannot starve it.
/// Its workers reserve before calling a probe; filesystem hooks must not acquire twice.
pub fn import_gate() -> &'static WorkGate {
    IMPORT_GATE.get_or_init(|| WorkGate::new(budget() / 8))
}
static IMPORT_GATE: OnceLock<WorkGate> = OnceLock::new();

thread_local! { static IMPORT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

pub(crate) fn is_import() -> bool {
    IMPORT.with(|v| v.get())
}

pub fn in_import<T>(f: impl FnOnce() -> T) -> T {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            IMPORT.with(|v| v.set(self.0));
        }
    }
    let _reset = Reset(IMPORT.with(|v| v.replace(true)));
    f()
}

/// The process-wide gate for the photos of an export batch rendering side by side
/// ([`crate::export::run_batch`]; limit: twice the budget). Separate from [`work_gate`], which the
/// decodes inside each export take: one gate held across the other's acquire could deadlock.
pub fn export_gate() -> &'static WorkGate {
    EXPORT_GATE.get_or_init(|| WorkGate::new(export_share(budget())))
}

fn export_share(budget: usize) -> usize {
    budget.saturating_mul(2)
}

static EXPORT_GATE: OnceLock<WorkGate> = OnceLock::new();

/// Entries and bytes held by one cache.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Usage {
    pub count: usize,
    pub bytes: usize,
}

impl Usage {
    pub fn new(count: usize, bytes: usize) -> Usage {
        Usage { count, bytes }
    }
}

/// Heap bytes as counted by an instrumented allocator.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeapUsage {
    /// Live heap bytes now.
    pub current: u64,
    /// Highest live heap bytes so far.
    pub peak: u64,
}

/// Device buffers of the GPU renderer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuUsage {
    /// Every device buffer that exists.
    pub allocated: u64,
    /// Of which recycled buffers waiting in the free pool.
    pub pooled: u64,
    /// Of which buffers released since their thread's last submit.
    pub retired: u64,
}

/// What the engine's caches hold.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryReport {
    /// Decoded thumbnail-level sources (≤ 512 px, linear float).
    pub thumb_sources: Usage,
    /// Decoded preview-level sources (≤ 2560 px, linear float) of the current and prefetched photos.
    pub preview_sources: Usage,
    /// The last full-resolution original (exports, 1:1).
    pub full_source: Usage,
    /// Rendered thumbnails and view renders (8-bit) in memory.
    pub rendered: Usage,
    /// Sum of the above.
    pub engine_bytes: usize,
    pub gpu: GpuUsage,
    /// The memory budget (see [`budget`]) and the engine caches' share of it.
    pub budget: usize,
    pub cache_budget: usize,
    /// Working memory of decodes / probes in flight and its limit ([`work_gate`]).
    pub work_bytes: usize,
    pub work_limit: usize,
    pub import_bytes: usize,
    pub import_limit: usize,
    pub import_waiting: usize,
    /// Live/peak heap when the binary counts allocations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heap: Option<HeapUsage>,
}

static HEAP: OnceLock<fn() -> HeapUsage> = OnceLock::new();
static RELEASE: OnceLock<fn()> = OnceLock::new();

/// Install a hook that hands memory the allocator keeps after frees back to the system (first
/// call wins). The macOS allocator keeps freed large blocks resident (reusable, but counted in
/// the resident size) until the system runs short; the apps install
/// `malloc_zone_pressure_relief` so the resident size follows what is actually held.
pub fn set_release_hook(f: fn()) {
    let _ = RELEASE.set(f);
}

/// Return freed memory to the system now (after large temporaries were dropped: a decode, an
/// import). No-op without a hook.
pub fn release() {
    if let Some(f) = RELEASE.get() {
        f();
    }
}

/// Install the heap counter of an instrumented binary (first call wins).
pub fn set_heap_stats(f: fn() -> HeapUsage) {
    let _ = HEAP.set(f);
}

/// Live/peak heap bytes, when the binary installed a counter.
pub fn heap_stats() -> Option<HeapUsage> {
    HEAP.get().map(|f| f())
}

/// The GPU renderer's device buffers.
pub fn gpu_usage() -> GpuUsage {
    let g = lightcraft_gpu::memory();
    GpuUsage { allocated: g.allocated, pooled: g.pooled, retired: g.retired }
}

impl crate::Session {
    /// What the engine's caches hold now.
    pub fn memory_report(&self) -> MemoryReport {
        let (thumb_sources, preview_sources, full_source) = self.media.usage();
        let (n, b) = self.media.rendered.mem_usage();
        let rendered = Usage::new(n, b);
        MemoryReport {
            thumb_sources,
            preview_sources,
            full_source,
            rendered,
            engine_bytes: thumb_sources.bytes + preview_sources.bytes + full_source.bytes + rendered.bytes,
            gpu: gpu_usage(),
            budget: budget(),
            cache_budget: self.media.budget(),
            work_bytes: work_gate().usage().0,
            work_limit: work_gate().usage().1,
            import_bytes: import_gate().usage().0,
            import_limit: import_gate().usage().1,
            import_waiting: import_gate().waiting(),
            heap: heap_stats(),
        }
    }

    /// Set the memory budget (bytes; process-wide) and apply this session's cache share.
    pub fn set_memory_budget(&mut self, bytes: usize) -> usize {
        let b = set_budget(bytes);
        self.media.set_budget(cache_share(b));
        b
    }
}

#[cfg(test)]
mod import_gate_tests {
    use super::*;
    use std::sync::{Arc, atomic::AtomicBool, mpsc::channel};
    use std::time::Duration;

    #[test]
    fn import_wait_is_cancellable_without_releasing_the_busy_holder() {
        let gate = Arc::new(WorkGate::new(10));
        let held = gate.acquire(10);
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let (g, c) = (gate.clone(), cancel.clone());
        let worker = std::thread::spawn(move || {
            tx.send(g.acquire_cancel(1, &c).is_none()).unwrap();
        });
        assert!(rx.recv_timeout(Duration::from_millis(60)).is_err());
        cancel.store(true, Ordering::Relaxed);
        let result = rx.recv_timeout(Duration::from_secs(2));
        drop(held);
        worker.join().unwrap();
        assert!(result.unwrap());
        assert_eq!(gate.usage().0, 0);
    }

    #[test]
    fn a_large_preview_is_not_overtaken_by_new_header_reads() {
        let gate = Arc::new(WorkGate::new(10));
        let held = gate.acquire(4);
        let (events, seen) = channel();
        let (release, released) = channel();
        let (g, e) = (gate.clone(), events.clone());
        let large = std::thread::spawn(move || {
            let _held = g.acquire(20);
            e.send("large").unwrap();
            released.recv_timeout(Duration::from_secs(5)).unwrap();
        });
        let start = std::time::Instant::now();
        while gate.waiting() == 0 && start.elapsed() < Duration::from_secs(2) {
            std::thread::yield_now();
        }
        let g = gate.clone();
        let small = std::thread::spawn(move || {
            let _held = g.acquire(1);
            events.send("small").unwrap();
        });
        drop(held);
        let first = seen.recv_timeout(Duration::from_secs(2));
        release.send(()).unwrap();
        large.join().unwrap();
        small.join().unwrap();
        assert_eq!(first.unwrap(), "large");
        assert_eq!(seen.recv_timeout(Duration::from_secs(2)).unwrap(), "small");
        assert_eq!(gate.waiting(), 0);
    }

    #[test]
    fn import_reservation_does_not_wait_on_face_decodes() {
        let faces = WorkGate::new(10);
        let imports = WorkGate::new(10);
        let _face = faces.acquire_urgent(1000);
        let cancel = AtomicBool::new(false);
        let _import = imports.acquire_cancel(5, &cancel).unwrap();
        assert_eq!(imports.usage().0, 5);
        assert_eq!(faces.usage().0, 1000);
    }
}
