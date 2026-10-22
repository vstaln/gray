use std::collections::{BTreeSet, HashMap};
use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const MASK: u32 = libc::IN_MODIFY
    | libc::IN_CLOSE_WRITE
    | libc::IN_CREATE
    | libc::IN_DELETE
    | libc::IN_MOVED_FROM
    | libc::IN_MOVED_TO
    | libc::IN_ATTRIB
    | libc::IN_DELETE_SELF
    | libc::IN_MOVE_SELF
    | libc::IN_ONLYDIR
    | libc::IN_DONT_FOLLOW;

struct Inner {
    fd: i32,

    dirs: HashMap<i32, PathBuf>,

    touched: BTreeSet<Vec<u8>>,

    broken: bool,
}

pub struct Watch {
    inner: Mutex<Inner>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        unsafe { libc::close(self.fd) };
    }
}

impl Watch {
    pub fn new(root: &Path) -> io::Result<Watch> {
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut inner = Inner {
            fd,
            dirs: HashMap::new(),
            touched: BTreeSet::new(),
            broken: false,
        };
        add_tree(&mut inner, root)?;
        Ok(Watch {
            inner: Mutex::new(inner),
        })
    }

    pub fn drain_forever(&self) {
        loop {
            self.drain();
            let mut pfd = libc::pollfd {
                fd: self.inner.lock().unwrap().fd,
                events: libc::POLLIN,
                revents: 0,
            };
            unsafe { libc::poll(&mut pfd, 1, 200) };
        }
    }

    pub fn changed(&self, scope: &[u8]) -> Option<Vec<Vec<u8>>> {
        let mut inner = self.inner.lock().unwrap();
        read_events(&mut inner);
        if inner.broken {
            return None;
        }
        Some(
            inner
                .touched
                .range(scope.to_vec()..)
                .take_while(|p| p.starts_with(scope))
                .filter(|p| p.len() == scope.len() || p.get(scope.len()) == Some(&b'/'))
                .cloned()
                .collect(),
        )
    }

    fn drain(&self) {
        let mut inner = self.inner.lock().unwrap();
        read_events(&mut inner);
    }
}

fn add_tree(inner: &mut Inner, dir: &Path) -> io::Result<()> {
    let c =
        CString::new(dir.as_os_str().as_bytes()).map_err(|_| io::Error::other("nul in path"))?;
    let wd = unsafe { libc::inotify_add_watch(inner.fd, c.as_ptr(), MASK) };
    if wd < 0 {
        return Err(io::Error::last_os_error());
    }
    inner.dirs.insert(wd, dir.to_path_buf());
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_name().to_str().is_none() {
            inner.broken = true;
        }
        if entry.file_type()?.is_dir() {
            add_tree(inner, &entry.path())?;
        }
    }
    Ok(())
}

fn read_events(inner: &mut Inner) {
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = unsafe { libc::read(inner.fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n <= 0 {
            if n < 0 && io::Error::last_os_error().kind() != io::ErrorKind::WouldBlock {
                inner.broken = true;
            }
            return;
        }
        let mut at = 0usize;
        let n = n as usize;
        while at + std::mem::size_of::<libc::inotify_event>() <= n {
            let ev: libc::inotify_event =
                unsafe { std::ptr::read_unaligned(buf[at..].as_ptr().cast()) };
            let name_at = at + std::mem::size_of::<libc::inotify_event>();
            let name_len = ev.len as usize;
            let name = &buf[name_at..name_at + name_len];
            let name = name.split(|&b| b == 0).next().unwrap_or(&[]);
            at = name_at + name_len;
            apply(inner, ev.wd, ev.mask, name);
        }
    }
}

fn apply(inner: &mut Inner, wd: i32, mask: u32, name: &[u8]) {
    if mask & libc::IN_Q_OVERFLOW != 0 || std::str::from_utf8(name).is_err() {
        inner.broken = true;
        return;
    }
    if mask & libc::IN_IGNORED != 0 {
        inner.dirs.remove(&wd);
        return;
    }
    let Some(dir) = inner.dirs.get(&wd).cloned() else {
        return;
    };
    let path = if name.is_empty() {
        dir
    } else {
        dir.join(std::ffi::OsStr::from_bytes(name))
    };

    if mask & libc::IN_ISDIR != 0 && mask & (libc::IN_CREATE | libc::IN_MOVED_TO) != 0 {
        if add_tree(inner, &path).is_err() {
            inner.broken = true;
            return;
        }
        let mut stack = vec![path.clone()];
        while let Some(d) = stack.pop() {
            inner.touched.insert(d.as_os_str().as_bytes().to_vec());
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                if e.file_name().to_str().is_none() {
                    inner.broken = true;
                }
                let p = e.path();
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    stack.push(p.clone());
                }
                inner.touched.insert(p.as_os_str().as_bytes().to_vec());
            }
        }
        return;
    }

    inner.touched.insert(path.as_os_str().as_bytes().to_vec());
}
