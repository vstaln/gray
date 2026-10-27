use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub const HALF_LIFE: f32 = 14.0 * 86_400.0;

const FLOOR: f32 = 0.06;
const MAX_ENTRIES: usize = 4096;

const DEBOUNCE: u32 = 30;

pub fn decay(age: u32) -> f32 {
    (-(age as f32) / HALF_LIFE).exp2()
}

pub fn boost(s: f32) -> i32 {
    if s > 0.0 { (12.0 * s.ln_1p()).min(40.0) as i32 } else { 0 }
}

pub struct Frecency {
    file: PathBuf,

    map: Mutex<HashMap<Vec<u8>, (f32, u32)>>,
    saving: AtomicBool,
}

fn decayed((score, last): (f32, u32), now: u32) -> f32 {
    score * decay(now.saturating_sub(last))
}

impl Frecency {
    pub fn load(file: PathBuf) -> Arc<Frecency> {
        let mut map = HashMap::new();
        let text = std::fs::read(&file).unwrap_or_default();
        for line in text.split(|&b| b == b'\n') {
            let mut f = line.splitn(3, |&b| b == b'\t');
            let (Some(s), Some(t), Some(p)) = (f.next(), f.next(), f.next()) else { continue };
            let num = |b: &[u8]| -> Option<f64> { std::str::from_utf8(b).ok()?.parse().ok() };
            let p = if p.first() == Some(&b'"') { serde_json::from_slice::<String>(p).ok().map(String::into_bytes) } else { Some(p.to_vec()) };
            if let (Some(s), Some(t), Some(p)) = (num(s), num(t), p) {
                map.insert(p, (s as f32, t as u32));
            }
        }
        Arc::new(Frecency { file, map: Mutex::new(map), saving: AtomicBool::new(false) })
    }

    pub fn open(self: &Arc<Self>, path: &[u8], now: u32) -> f32 {
        let mut map = self.map.lock().unwrap();
        let e = map.entry(path.to_vec()).or_insert((0.0, now));
        if e.0 > 0.0 && now.saturating_sub(e.1) < DEBOUNCE {
            return e.0;
        }
        *e = (decayed(*e, now) + 1.0, now);
        let score = e.0;
        if map.len() > MAX_ENTRIES {
            let mut v: Vec<(Vec<u8>, (f32, u32))> = map.drain().collect();
            v.sort_by(|a, b| decayed(b.1, now).total_cmp(&decayed(a.1, now)));
            v.truncate(MAX_ENTRIES * 3 / 4);
            map.extend(v);
        }
        drop(map);
        self.save_soon();
        score
    }

    pub fn score(&self, path: &[u8], now: u32) -> f32 {
        self.map.lock().unwrap().get(path).map_or(0.0, |&e| decayed(e, now))
    }

    pub fn under(&self, dir: &[u8], now: u32) -> Vec<(Vec<u8>, f32)> {
        let map = self.map.lock().unwrap();
        let inside = |p: &[u8]| dir == b"/" || (p.starts_with(dir) && p.get(dir.len()) == Some(&b'/'));
        map.iter().filter(|(p, _)| inside(p)).map(|(p, &e)| (p.clone(), decayed(e, now))).collect()
    }

    fn save_soon(self: &Arc<Self>) {
        if self.saving.swap(true, Ordering::Relaxed) {
            return;
        }
        let me = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(2));
            me.saving.store(false, Ordering::Relaxed);
            let now = fsearch::query::now_secs();
            let mut out = Vec::new();
            {
                let mut map = me.map.lock().unwrap();
                map.retain(|_, e| decayed(*e, now) >= FLOOR);
                for (p, &(s, t)) in map.iter() {
                    let quoted = (p.contains(&b'\n') || p.first() == Some(&b'"'))
                        .then(|| std::str::from_utf8(p).ok().map(|p| serde_json::json!(p).to_string()));
                    let p = match &quoted {
                        None => &p[..],
                        Some(Some(q)) => q.as_bytes(),
                        Some(None) => continue,
                    };
                    out.extend_from_slice(format!("{s:.3}\t{t}\t").as_bytes());
                    out.extend_from_slice(p);
                    out.push(b'\n');
                }
            }
            let tmp = me.file.with_extension("new");
            if let Err(e) = std::fs::write(&tmp, out).and_then(|_| std::fs::rename(&tmp, &me.file)) {
                fsearch::diag_line(format!("frecency: save failed: {e}"));
            }
        });
    }
}
