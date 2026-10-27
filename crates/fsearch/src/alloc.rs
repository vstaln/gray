use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};

pub struct Alloc;

const BIG: usize = 1 << 20;

const LISTS: usize = 13;

const PER_LIST: u16 = 64;
const THREAD_BYTES: usize = 1 << 20;

const MAX_THREADS: usize = 64;

static THREADS: AtomicUsize = AtomicUsize::new(0);

static KEPT: AtomicBool = AtomicBool::new(false);

struct Lists {
    on: bool,
    bytes: usize,
    n: [u16; LISTS],
    head: [*mut u8; LISTS],
}

impl Lists {
    fn take(&mut self, c: usize) -> Option<*mut u8> {
        let p = *self.head.get(c)?;
        if p.is_null() {
            return None;
        }

        self.head[c] = unsafe { *(p as *mut *mut u8) };
        self.n[c] -= 1;
        self.bytes -= 16 << c;
        Some(p)
    }

    fn keep(&mut self, c: usize, p: *mut u8) -> bool {
        if !self.on || c >= LISTS || self.n[c] == PER_LIST || self.bytes + (16 << c) > THREAD_BYTES {
            return false;
        }
        if self.bytes == 0 && !KEPT.load(Relaxed) {
            KEPT.store(true, Relaxed);
        }

        unsafe { *(p as *mut *mut u8) = self.head[c] };
        self.head[c] = p;
        self.n[c] += 1;
        self.bytes += 16 << c;
        true
    }

    fn drain(&mut self) {
        for c in 0..LISTS {
            while let Some(p) = self.take(c) {
                unsafe { System.dealloc(p, Layout::from_size_align_unchecked(16 << c, 16)) };
            }
        }
    }
}

thread_local! {

    static FREE: UnsafeCell<Lists> = const { UnsafeCell::new(Lists { on: false, bytes: 0, n: [0; LISTS], head: [std::ptr::null_mut(); LISTS] }) };
    static EXIT: Exit = const { Exit };
}

struct Exit;

impl Drop for Exit {
    fn drop(&mut self) {
        FREE.with(|f| {
            let f = unsafe { &mut *f.get() };
            if f.on {
                f.on = false;
                THREADS.fetch_sub(1, Relaxed);
            }
            f.drain();
        });
    }
}

pub fn keep_on_this_thread() {
    if FREE.with(|f| unsafe { (*f.get()).on }) {
        return;
    }
    if THREADS.fetch_add(1, Relaxed) >= MAX_THREADS {
        THREADS.fetch_sub(1, Relaxed);
        return;
    }

    EXIT.with(|_| {});
    FREE.with(|f| unsafe { (*f.get()).on = true });
}

pub fn drain() {
    FREE.with(|f| unsafe { (*f.get()).drain() });
}

pub fn kept() -> bool {
    KEPT.swap(false, Relaxed)
}

fn fit(size: usize) -> usize {
    (size.max(16).next_power_of_two().trailing_zeros() - 4) as usize
}

fn list(size: usize) -> usize {
    (usize::BITS - 5 - size.max(16).leading_zeros()) as usize
}

fn small(l: Layout) -> Layout {
    if l.size() < 16 { unsafe { Layout::from_size_align_unchecked(16, l.align()) } } else { l }
}

fn take(l: Layout) -> Option<*mut u8> {
    if l.align() > 16 {
        return None;
    }
    FREE.with(|f| unsafe { (*f.get()).take(fit(l.size())) })
}

unsafe impl GlobalAlloc for Alloc {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if l.size() >= BIG && l.align() <= 16384 {
            let p =
                unsafe { libc::mmap(std::ptr::null_mut(), l.size(), libc::PROT_READ | libc::PROT_WRITE, libc::MAP_PRIVATE | libc::MAP_ANON, -1, 0) };
            return if p == libc::MAP_FAILED { std::ptr::null_mut() } else { p as *mut u8 };
        }
        take(l).unwrap_or_else(|| unsafe { System.alloc(small(l)) })
    }

    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        if l.size() >= BIG && l.align() <= 16384 {
            return unsafe { self.alloc(l) };
        }
        match take(l) {
            Some(p) => {
                unsafe { p.write_bytes(0, l.size()) };
                p
            }
            None => unsafe { System.alloc_zeroed(small(l)) },
        }
    }

    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        if l.size() >= BIG && l.align() <= 16384 {
            unsafe { libc::munmap(p as *mut libc::c_void, l.size()) };
            return;
        }
        let kept = l.align() <= 16 && FREE.with(|f| unsafe { (*f.get()).keep(list(l.size()), p) });
        if !kept {
            unsafe { System.dealloc(p, small(l)) }
        }
    }

    unsafe fn realloc(&self, p: *mut u8, l: Layout, new_size: usize) -> *mut u8 {
        if l.size() < BIG && new_size < BIG && l.align() <= 16 && FREE.with(|f| unsafe { (*f.get()).on }) {
            if new_size <= l.size() {
                return p;
            }
            let q = unsafe { self.alloc(Layout::from_size_align_unchecked(new_size, l.align())) };
            if !q.is_null() {
                unsafe {
                    std::ptr::copy_nonoverlapping(p, q, l.size());
                    self.dealloc(p, l);
                }
            }
            return q;
        }
        if (l.size() < BIG && new_size < BIG) || l.align() > 16384 {
            return unsafe { System.realloc(p, small(l), new_size.max(16)) };
        }
        let q = unsafe { self.alloc(Layout::from_size_align_unchecked(new_size, l.align())) };
        if !q.is_null() {
            unsafe {
                std::ptr::copy_nonoverlapping(p, q, l.size().min(new_size));
                self.dealloc(p, l);
            }
        }
        q
    }
}
