//! Tauri-Schicht: Befehle für Rechnen, Ziffern holen, Suchen, Formensuche, Statistik.
//!
//! Der Rechenzustand liegt global (`ENGINE`), nicht im Fenster: Rechnet die App im Hintergrund
//! weiter und die Oberfläche wird neu aufgebaut (Handy war aus, App wieder geöffnet), fragt sie
//! einfach `compute_status` ab und hängt sich wieder an.

use pi_core::{chudnovsky, estimate, ooc, search, shape, split, stats};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::{Duration, Instant};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
use tauri::ipc::Response;
use tauri::{AppHandle, Manager};

const MAX_DIGITS_FAST: usize = 1_000_000_000;
/// Mit Platte theoretisch beliebig; die Grenze ist der freie Speicherplatz.
const MAX_DIGITS_SPLIT: usize = 1_000_000_000_000;
const HIT_PAGE_MAX: usize = 5_000;
/// Name der Ordner, in denen die App temporäre Dateien anlegt (nur diese werden aufgeräumt)
const TEMP_DIR_NAME: &str = "PiExplorerTemp";

#[derive(Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct ComputeInfo {
    digits: usize,
    mode: String,
    budget: u64,
    seconds: f64,
    /// (Phase, Sekunden)
    phases: Vec<(String, f64)>,
    peak_rss: u64,
    /// > 0, wenn mit Hilfsdateien gerechnet wurde
    disk_peak: u64,
    temp_dir: String,
}

/// Wo die fertigen Ziffern liegen: im RAM oder (Plattenmodus) in einer Datei.
#[derive(Clone)]
enum DigitStore {
    Ram(Arc<Vec<u8>>),
    File { path: PathBuf, n: u64 },
}

impl Default for DigitStore {
    fn default() -> Self {
        DigitStore::Ram(Arc::new(Vec::new()))
    }
}

impl DigitStore {
    fn len(&self) -> u64 {
        match self {
            DigitStore::Ram(d) => d.len() as u64,
            DigitStore::File { n, .. } => *n,
        }
    }
    fn read(&self, off: u64, len: usize) -> std::io::Result<Vec<u8>> {
        match self {
            DigitStore::Ram(d) => {
                let a = (off as usize).min(d.len());
                let b = (a + len).min(d.len());
                Ok(d[a..b].to_vec())
            }
            DigitStore::File { path, .. } => ooc::read_digit_file(path, off, len),
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    /// "idle" | "running" | "done" | "cancelled" | "error"
    state: &'static str,
    run_id: u64,
    phase: String,
    fraction: f64,
    elapsed: f64,
    digits: usize,
    mode: String,
    budget: u64,
    message: String,
    rss: u64,
    /// aktuell belegte Hilfsdateien (Plattenmodus)
    disk: u64,
    info: Option<ComputeInfo>,
}

impl Default for Status {
    fn default() -> Self {
        Status {
            state: "idle",
            run_id: 0,
            phase: String::new(),
            fraction: 0.0,
            elapsed: 0.0,
            digits: 0,
            mode: String::new(),
            budget: 0,
            message: String::new(),
            rss: 0,
            disk: 0,
            info: None,
        }
    }
}

#[derive(Default)]
struct Engine {
    digits: RwLock<DigitStore>,
    cancel: AtomicBool,
    running: AtomicBool,
    status: Mutex<Status>,
    stats: Mutex<Option<stats::Stats>>,
    calib: Mutex<Option<f64>>,
    hits: Mutex<Vec<(u64, u8)>>,
    shape: Mutex<Vec<shape::ShapeMatch>>,
    peak_rss: AtomicU64,
    run_counter: AtomicU64,
    status_file: Mutex<Option<PathBuf>>,
    /// Ordner der aktuellen Plattenrechnung (wird bei neuer Rechnung / Beenden gelöscht)
    run_dir: Mutex<Option<PathBuf>>,
    /// Standard-Ort für Hilfsdateien (App-Cache)
    cache_base: Mutex<Option<PathBuf>>,
}

fn dir_size(p: &Path) -> u64 {
    std::fs::read_dir(p)
        .map(|it| {
            it.flatten()
                .map(|e| {
                    let m = e.metadata();
                    match m {
                        Ok(m) if m.is_dir() => dir_size(&e.path()),
                        Ok(m) => m.len(),
                        Err(_) => 0,
                    }
                })
                .sum()
        })
        .unwrap_or(0)
}

/// Ziffern-Datei und Hilfsdateien der letzten Plattenrechnung löschen.
fn cleanup_run() {
    *ENGINE.digits.write().unwrap() = DigitStore::default();
    if let Some(d) = ENGINE.run_dir.lock().unwrap().take() {
        let _ = std::fs::remove_dir_all(d);
    }
}

/// Übrig gebliebene Läufe (Absturz, hartes Beenden) in einem Basisordner löschen.
fn cleanup_stale(base: &Path) {
    if let Ok(it) = std::fs::read_dir(base) {
        for e in it.flatten() {
            if e.file_name().to_string_lossy().starts_with("run-") {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
}

static ENGINE: LazyLock<Engine> = LazyLock::new(Engine::default);

fn mem() -> (u64, u64, u64) {
    let Ok(mut sys) = System::new() else { return (0, 0, 0) };
    sys.refresh_memory();
    let mut rss = 0;
    if let Ok(pid) = sysinfo::get_current_pid() {
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            false,
            ProcessRefreshKind::nothing().with_memory(),
        );
        rss = sys.process(pid).map(|p| p.memory()).unwrap_or(0);
    }
    (sys.total_memory(), sys.available_memory(), rss)
}

fn calib() -> f64 {
    let mut c = ENGINE.calib.lock().unwrap();
    *c.get_or_insert_with(estimate::calibrate)
}

/// Kurzer Status als Textdatei – liest der Android-Hintergrunddienst für seine Benachrichtigung.
fn write_status_file() {
    let Some(path) = ENGINE.status_file.lock().unwrap().clone() else { return };
    let s = ENGINE.status.lock().unwrap().clone();
    let text = format!(
        "state={}\nphase={}\nfraction={:.4}\nelapsed={:.1}\ndigits={}\nmode={}\n",
        s.state, s.phase, s.fraction, s.elapsed, s.digits, s.mode
    );
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

// ------------------------------------------------------------------ Gerät und Schätzung

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceInfo {
    cores: usize,
    total_ram: u64,
    avail_ram: u64,
    os: &'static str,
    arch: &'static str,
    max_digits_fast: usize,
    max_digits_split: usize,
    status_file: String,
}

#[tauri::command]
async fn device_info() -> Result<DeviceInfo, String> {
    calib();
    let (total, avail, _) = mem();
    Ok(DeviceInfo {
        cores: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
        total_ram: total,
        avail_ram: avail,
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        max_digits_fast: MAX_DIGITS_FAST,
        max_digits_split: MAX_DIGITS_SPLIT,
        status_file: ENGINE
            .status_file
            .lock()
            .unwrap()
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Estimate {
    seconds: f64,
    peak_ram: u64,
    avail_ram: u64,
    total_ram: u64,
    /// höchste Stellenzahl nur im RAM für dieses Budget (SPLIT)
    max_digits: u64,
    /// SPLIT: mehr Stellen, als ins Budget passen → Hilfsdateien auf der Platte
    needs_disk: bool,
    disk_needed: u64,
    disk_free: u64,
    /// "ok" | "warn" | "danger"
    level: &'static str,
}

fn split_plan(budget_mb: u64) -> split::Plan {
    split::Plan::for_budget(budget_mb.max(64) << 20)
}

fn ooc_plan(budget_mb: u64) -> ooc::OocPlan {
    ooc::OocPlan::for_budget(budget_mb.max(64) << 20)
}

/// Freier Platz am Pfad (oder am nächsten existierenden Elternordner).
fn free_space(p: &Path) -> u64 {
    let mut cur = Some(p);
    while let Some(c) = cur {
        if c.exists() {
            return fs4::available_space(c).unwrap_or(0);
        }
        cur = c.parent();
    }
    0
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StorageOption {
    path: String,
    label: String,
    free: u64,
}

/// Orte für Hilfsdateien: App-Speicher und (am PC) jedes Laufwerk mit Platz.
#[tauri::command]
async fn storage_options() -> Vec<StorageOption> {
    let mut v = Vec::new();
    if let Some(c) = ENGINE.cache_base.lock().unwrap().clone() {
        let free = free_space(&c);
        v.push(StorageOption { path: c.to_string_lossy().into_owned(), label: "App storage".into(), free });
    }
    #[cfg(windows)]
    {
        let mut drives = Vec::new();
        for l in b'C'..=b'Z' {
            let root = format!("{}:\\", l as char);
            let rp = Path::new(&root);
            if rp.exists() {
                let free = free_space(rp);
                if free >= 1 << 30 {
                    let p = rp.join(TEMP_DIR_NAME);
                    drives.push(StorageOption {
                        path: p.to_string_lossy().into_owned(),
                        label: format!("Drive {}:", l as char),
                        free,
                    });
                }
            }
        }
        drives.sort_by(|a, b| b.free.cmp(&a.free));
        v.extend(drives);
    }
    v
}

#[tauri::command]
async fn estimate(digits: usize, mode: String, budget_mb: u64, temp_dir: Option<String>) -> Result<Estimate, String> {
    let k = calib();
    let (total, avail, _) = mem();
    let fast_s = estimate::estimate_seconds(digits, k);
    let mut needs_disk = false;
    let mut disk_needed = 0;
    let (seconds, peak, max_digits) = if mode == "split" {
        let plan = split_plan(budget_mb);
        let limbs = digits as f64 * 3.3219 / 64.0;
        if (digits as u64) <= plan.max_digits() {
            // gemessen: 100 Mio. Stellen mit 512 MB ≈ 4× langsamer als im RAM
            let factor = 2.0 + 0.2 * (limbs / plan.chunk as f64);
            (fast_s * factor, split::estimate_peak(digits as u64, &plan), plan.max_digits())
        } else {
            needs_disk = true;
            disk_needed = ooc::estimate_disk(digits as u64);
            let op = ooc_plan(budget_mb);
            let factor = ooc::time_factor(digits as u64, &op);
            (fast_s * factor, (budget_mb << 20) * 9 / 10, plan.max_digits())
        }
    } else {
        (fast_s, estimate::estimate_peak_ram(digits), MAX_DIGITS_FAST as u64)
    };
    let disk_free = if needs_disk {
        let base = temp_dir.map(PathBuf::from).or_else(|| ENGINE.cache_base.lock().unwrap().clone());
        base.map(|b| free_space(&b)).unwrap_or(0)
    } else {
        0
    };
    let level = if avail > 0 && peak as f64 > avail as f64 * 0.85 || (needs_disk && disk_needed > disk_free) {
        "danger"
    } else if avail > 0 && peak as f64 > avail as f64 * 0.5 || (needs_disk && disk_needed > disk_free / 10 * 8) {
        "warn"
    } else {
        "ok"
    };
    Ok(Estimate {
        seconds,
        peak_ram: peak,
        avail_ram: avail,
        total_ram: total,
        max_digits,
        needs_disk,
        disk_needed,
        disk_free,
        level,
    })
}

// ------------------------------------------------------------------ Rechnen

#[tauri::command]
fn start_compute(
    digits: usize,
    mode: String,
    budget_mb: u64,
    temp_dir: Option<String>,
    app: AppHandle,
) -> Result<u64, String> {
    let max = if mode == "split" { MAX_DIGITS_SPLIT } else { MAX_DIGITS_FAST };
    if digits == 0 || digits > max {
        return Err(format!("Choose between 1 and {max} digits"));
    }
    let e = &*ENGINE;
    if e.running.swap(true, Ordering::SeqCst) {
        return Err("A computation is already running".into());
    }
    e.cancel.store(false, Ordering::SeqCst);
    // alte Daten (auch Ziffern-Dateien) freigeben, bevor neu gerechnet wird
    cleanup_run();
    *e.stats.lock().unwrap() = None;
    let use_disk = mode == "split" && (digits as u64) > split_plan(budget_mb).max_digits();
    let run_dir = if use_disk {
        let base = temp_dir.map(PathBuf::from).or_else(|| e.cache_base.lock().unwrap().clone());
        let Some(base) = base else {
            e.running.store(false, Ordering::SeqCst);
            return Err("No folder for temporary files".into());
        };
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let d = base.join(format!("run-{stamp}"));
        if let Err(err) = std::fs::create_dir_all(&d) {
            e.running.store(false, Ordering::SeqCst);
            return Err(format!("Can't create {}: {err}", d.display()));
        }
        *e.run_dir.lock().unwrap() = Some(d.clone());
        Some(d)
    } else {
        None
    };
    e.hits.lock().unwrap().clear();
    e.shape.lock().unwrap().clear();
    e.peak_rss.store(0, Ordering::Relaxed);
    let run_id = e.run_counter.fetch_add(1, Ordering::SeqCst) + 1;
    let budget = budget_mb << 20;
    *e.status.lock().unwrap() = Status {
        state: "running",
        run_id,
        phase: "SERIES".into(),
        digits,
        mode: mode.clone(),
        budget,
        ..Default::default()
    };
    write_status_file();

    let start = Instant::now();
    let watch_dir = run_dir.clone();
    // Nebenher: RAM-Spitze messen, Platte zählen, Status-Datei schreiben
    std::thread::spawn(move || {
        let mut tick = 0u32;
        while ENGINE.running.load(Ordering::Relaxed) {
            let (_, _, rss) = mem();
            ENGINE.peak_rss.fetch_max(rss, Ordering::Relaxed);
            let disk = match (&watch_dir, tick % 5) {
                (Some(d), 0) => Some(dir_size(d)),
                _ => None,
            };
            tick += 1;
            {
                let mut s = ENGINE.status.lock().unwrap();
                s.rss = rss;
                s.elapsed = start.elapsed().as_secs_f64();
                if let Some(d) = disk {
                    s.disk = d;
                }
            }
            write_status_file();
            #[cfg(desktop)]
            desktop::update_tray_tooltip(&app);
            std::thread::sleep(Duration::from_millis(400));
        }
        #[cfg(desktop)]
        desktop::update_tray_tooltip(&app);
        let _ = &app;
    });

    std::thread::Builder::new()
        .name("pi-compute".into())
        .spawn(move || {
            #[cfg(windows)]
            let _awake = keep_awake::Guard::new();
            let phases: Mutex<Vec<(String, f64)>> = Mutex::new(Vec::new());
            let last = Mutex::new(Instant::now() - Duration::from_secs(1));
            let progress = |phase: &str, f: f64| {
                {
                    let mut ph = phases.lock().unwrap();
                    if ph.last().map(|p| p.0.as_str()) != Some(phase) {
                        ph.push((phase.to_string(), start.elapsed().as_secs_f64()));
                    }
                }
                let mut l = last.lock().unwrap();
                if l.elapsed() >= Duration::from_millis(60) || f >= 1.0 {
                    *l = Instant::now();
                    let mut s = ENGINE.status.lock().unwrap();
                    s.phase = phase.to_string();
                    s.fraction = f;
                    s.elapsed = start.elapsed().as_secs_f64();
                }
            };
            let mut disk_peak = 0u64;
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<DigitStore, String> {
                if let Some(dir) = &run_dir {
                    let out = dir.join("digits.bin");
                    let r = ooc::compute_pi_ooc(
                        digits,
                        ooc_plan(budget_mb),
                        &dir.join("work"),
                        &out,
                        &ENGINE.cancel,
                        &progress,
                    );
                    match r {
                        Ok(r) => {
                            disk_peak = r.disk_peak + digits as u64;
                            Ok(DigitStore::File { path: out, n: digits as u64 })
                        }
                        Err(ooc::OocError::Cancelled) => Err(String::new()),
                        Err(ooc::OocError::Io(m)) => Err(format!("Disk error: {m}")),
                    }
                } else if mode == "split" {
                    split::compute_pi_split(digits, split_plan(budget_mb), &ENGINE.cancel, &progress)
                        .map(|d| DigitStore::Ram(Arc::new(d)))
                        .map_err(|_| String::new())
                } else {
                    chudnovsky::compute_pi_digits(digits, &ENGINE.cancel, &|ph, f| progress(ph.label(), f))
                        .map(|(d, _)| DigitStore::Ram(Arc::new(d)))
                        .map_err(|_| String::new())
                }
            }));
            let total_s = start.elapsed().as_secs_f64();
            let (_, _, rss) = mem();
            ENGINE.peak_rss.fetch_max(rss, Ordering::Relaxed);
            // Phasen-Startzeiten → Dauern
            let marks = phases.into_inner().unwrap();
            let durations: Vec<(String, f64)> = marks
                .iter()
                .enumerate()
                .map(|(i, (n, t0))| {
                    let t1 = marks.get(i + 1).map(|m| m.1).unwrap_or(total_s);
                    (n.clone(), (t1 - t0).max(0.0))
                })
                .collect();
            let mut st = ENGINE.status.lock().unwrap().clone();
            st.elapsed = total_s;
            match res {
                Ok(Ok(store)) => {
                    if let DigitStore::Ram(d) = &store {
                        *ENGINE.stats.lock().unwrap() = Some(stats::digit_stats(d));
                    }
                    *ENGINE.digits.write().unwrap() = store;
                    st.state = "done";
                    st.fraction = 1.0;
                    st.info = Some(ComputeInfo {
                        digits,
                        mode: mode.clone(),
                        budget,
                        seconds: total_s,
                        phases: durations,
                        peak_rss: ENGINE.peak_rss.load(Ordering::Relaxed),
                        disk_peak,
                        temp_dir: run_dir.as_ref().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default(),
                    });
                }
                Ok(Err(msg)) => {
                    if msg.is_empty() {
                        st.state = "cancelled";
                    } else {
                        st.state = "error";
                        st.message = msg;
                    }
                }
                Err(_) => {
                    st.state = "error";
                    st.message = "The computation crashed (out of memory?)".into();
                }
            }
            if st.state != "done" {
                cleanup_run();
            }
            *ENGINE.status.lock().unwrap() = st;
            ENGINE.running.store(false, Ordering::SeqCst);
            write_status_file();
        })
        .map_err(|e| e.to_string())?;
    Ok(run_id)
}

#[tauri::command]
fn compute_status() -> Status {
    let mut s = ENGINE.status.lock().unwrap().clone();
    if s.state != "running" {
        s.rss = mem().2;
    }
    s
}

#[tauri::command]
fn cancel_compute() {
    ENGINE.cancel.store(true, Ordering::SeqCst);
}

/// Rohbytes (Werte 0..=9) ab 0-basiertem Index.
#[tauri::command]
fn get_digits(offset: u64, len: usize) -> Response {
    let d = ENGINE.digits.read().unwrap().clone();
    Response::new(d.read(offset, len).unwrap_or_default())
}

// ------------------------------------------------------------------ Suche

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PatternOut {
    label: String,
    digits: String,
    count: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchOut {
    patterns: Vec<PatternOut>,
    letters: Option<String>,
    /// alle Treffer
    total: u64,
    /// davon gespeichert (zum Durchblättern)
    stored: usize,
    ms: f64,
    hits: Vec<(u64, u8)>,
    searched: u64,
}

/// kind: "number" | "birthday" | "birthday_all" | "word"
#[tauri::command]
async fn search(kind: String, query: String) -> Result<SearchOut, String> {
    let d = ENGINE.digits.read().unwrap().clone();
    let mut letters = None;
    let pats: Vec<(String, Vec<u8>)> = match kind.as_str() {
        "number" => {
            let p = search::parse_digits(&query).map_err(|c| format!("\"{c}\" is not a digit"))?;
            vec![("DIGITS".into(), p)]
        }
        "word" => {
            let (l, p) =
                search::word_to_digits(&query).map_err(|c| format!("\"{c}\" can't be converted (letters A–Z only)"))?;
            letters = Some(l);
            vec![("WORD".into(), p)]
        }
        "birthday" | "birthday_all" => {
            let mut v = search::birthday_patterns(&query)?;
            if kind == "birthday" {
                v.truncate(1);
            }
            v.into_iter().map(|p| (p.label.to_string(), p.digits)).collect()
        }
        _ => return Err("Unknown search type".into()),
    };
    if pats.iter().all(|p| p.1.is_empty()) {
        return Err("Type something first".into());
    }
    let t = Instant::now();
    let raw: Vec<Vec<u8>> = pats.iter().map(|p| p.1.clone()).collect();
    let searched = d.len();
    let (hits, counts) = tauri::async_runtime::spawn_blocking(move || match &d {
        DigitStore::Ram(v) => Ok(search::find_multi(v, &raw, search::MAX_STORED_HITS)),
        DigitStore::File { n, .. } => {
            search::find_multi_stream(*n, &|o, l| d.read(o, l), &raw, search::MAX_STORED_HITS).map_err(|e| e.to_string())
        }
    })
    .await
    .map_err(|e| e.to_string())??;
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    let out = SearchOut {
        patterns: pats
            .iter()
            .zip(&counts)
            .map(|((l, p), &c)| PatternOut { label: l.clone(), digits: chudnovsky::digits_to_string(p), count: c })
            .collect(),
        letters,
        total: counts.iter().sum(),
        stored: hits.len(),
        ms,
        hits: hits[..hits.len().min(HIT_PAGE_MAX)].to_vec(),
        searched,
    };
    *ENGINE.hits.lock().unwrap() = hits;
    Ok(out)
}

#[tauri::command]
fn get_hits(from: usize, count: usize) -> Vec<(u64, u8)> {
    let h = ENGINE.hits.lock().unwrap();
    let a = from.min(h.len());
    let b = (from + count.min(HIT_PAGE_MAX)).min(h.len());
    h[a..b].to_vec()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ShapeOut {
    box_w: usize,
    box_h: usize,
    box_x: usize,
    box_y: usize,
    checked_cells: u32,
    best_pos: Option<u64>,
    best_mismatches: Option<u32>,
    best_percent: Option<f64>,
    match_count: u64,
    stored: usize,
    positions: u64,
    ms: f64,
    /// (1-basierte Position der linken oberen Ecke, Abweichungen)
    matches: Vec<(u64, u32)>,
}

#[tauri::command]
async fn shape_search(
    cells: Vec<bool>,
    size: usize,
    width: usize,
    tolerance: u32,
    only_painted: bool,
) -> Result<ShapeOut, String> {
    let d = ENGINE.digits.read().unwrap().clone();
    let q = shape::ShapeQuery { cells, size, width, tolerance, only_painted };
    let t = Instant::now();
    let r = tauri::async_runtime::spawn_blocking(move || match &d {
        DigitStore::Ram(v) => shape::search_shape(v, &q),
        DigitStore::File { n, .. } => shape::search_shape_stream(*n, &|o, l| d.read(o, l), &q),
    })
    .await
    .map_err(|e| e.to_string())??;
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    let out = ShapeOut {
        box_w: r.box_w,
        box_h: r.box_h,
        box_x: r.box_x,
        box_y: r.box_y,
        checked_cells: r.checked_cells,
        best_pos: r.best.map(|b| b.index + 1),
        best_mismatches: r.best.map(|b| b.mismatches),
        best_percent: r.best.map(|b| r.percent(&b)),
        match_count: r.match_count,
        stored: r.matches.len(),
        positions: r.positions,
        ms,
        matches: r.matches.iter().take(HIT_PAGE_MAX).map(|m| (m.index + 1, m.mismatches)).collect(),
    };
    *ENGINE.shape.lock().unwrap() = r.matches;
    Ok(out)
}

#[tauri::command]
fn get_shape_matches(from: usize, count: usize) -> Vec<(u64, u32)> {
    let m = ENGINE.shape.lock().unwrap();
    let a = from.min(m.len());
    let b = (from + count.min(HIT_PAGE_MAX)).min(m.len());
    m[a..b].iter().map(|x| (x.index + 1, x.mismatches)).collect()
}

// ------------------------------------------------------------------ Statistik

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatsOut {
    digits: u64,
    counts: [u64; 10],
    run_digit: u8,
    run_len: u64,
    run_pos: u64,
    info: Option<ComputeInfo>,
    rss: u64,
    avail_ram: u64,
}

#[tauri::command]
async fn get_stats() -> Result<StatsOut, String> {
    let n = ENGINE.digits.read().unwrap().len();
    let cached = ENGINE.stats.lock().unwrap().clone();
    let s = match cached {
        Some(s) => s,
        None => {
            let d = ENGINE.digits.read().unwrap().clone();
            let s = tauri::async_runtime::spawn_blocking(move || match &d {
                DigitStore::Ram(v) => Ok(stats::digit_stats(v)),
                DigitStore::File { n, .. } => stats::digit_stats_stream(*n, &|o, l| d.read(o, l)).map_err(|e| e.to_string()),
            })
            .await
            .map_err(|e| e.to_string())??;
            *ENGINE.stats.lock().unwrap() = Some(s.clone());
            s
        }
    };
    let (_, avail, rss) = mem();
    Ok(StatsOut {
        digits: n,
        counts: s.counts,
        run_digit: s.run_digit,
        run_len: s.run_len,
        run_pos: s.run_pos,
        info: ENGINE.status.lock().unwrap().info.clone(),
        rss,
        avail_ram: avail,
    })
}

// ------------------------------------------------------------------ PC: Standby-Sperre und Tray

#[cfg(windows)]
mod keep_awake {
    use windows_sys::Win32::System::Power::{
        SetThreadExecutionState, ES_AWAYMODE_REQUIRED, ES_CONTINUOUS, ES_SYSTEM_REQUIRED,
    };

    /// Solange er lebt, geht der PC nicht in den Standby (der Bildschirm darf aus).
    pub struct Guard;
    impl Guard {
        pub fn new() -> Guard {
            unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_AWAYMODE_REQUIRED) };
            Guard
        }
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
        }
    }
}

#[cfg(desktop)]
mod desktop {
    use super::ENGINE;
    use std::sync::atomic::Ordering;
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
    use tauri::{AppHandle, Manager};

    const TRAY_ID: &str = "pi-tray";

    fn show_main(app: &AppHandle) {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.show();
            let _ = w.unminimize();
            let _ = w.set_focus();
        }
    }

    pub fn ensure_tray(app: &AppHandle) -> Option<TrayIcon> {
        if let Some(t) = app.tray_by_id(TRAY_ID) {
            return Some(t);
        }
        let show = MenuItem::with_id(app, "show", "Show π EXPLORER", true, None::<&str>).ok()?;
        let quit = MenuItem::with_id(app, "quit", "Stop computing and quit", true, None::<&str>).ok()?;
        let menu = Menu::with_items(app, &[&show, &quit]).ok()?;
        TrayIconBuilder::with_id(TRAY_ID)
            .icon(app.default_window_icon()?.clone())
            .tooltip("π EXPLORER")
            .menu(&menu)
            .show_menu_on_left_click(false)
            .on_menu_event(|app, ev| match ev.id().as_ref() {
                "show" => show_main(app),
                "quit" => {
                    ENGINE.cancel.store(true, Ordering::SeqCst);
                    app.exit(0);
                }
                _ => {}
            })
            .on_tray_icon_event(|tray, ev| {
                if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                    show_main(tray.app_handle());
                }
            })
            .build(app)
            .ok()
    }

    pub fn update_tray_tooltip(app: &AppHandle) {
        let Some(t) = app.tray_by_id(TRAY_ID) else { return };
        let s = ENGINE.status.lock().unwrap().clone();
        let text = match s.state {
            "running" => format!("π EXPLORER – {} {:.0} %", s.phase, s.fraction * 100.0),
            "done" => "π EXPLORER – done, click to open".to_string(),
            _ => "π EXPLORER".to_string(),
        };
        let _ = t.set_tooltip(Some(text));
    }

    /// Fenster schließen, während gerechnet wird: in den Infobereich statt beenden.
    pub fn on_close(window: &tauri::Window, api: &tauri::CloseRequestApi) {
        if ENGINE.running.load(Ordering::SeqCst) {
            api.prevent_close();
            let _ = window.hide();
            ensure_tray(window.app_handle());
            update_tray_tooltip(window.app_handle());
        }
    }
}

static APP: std::sync::OnceLock<AppHandle> = std::sync::OnceLock::new();

/// Neues Hauptfenster, falls keins mehr da ist.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn recreate_main_window() {
    let Some(app) = APP.get() else { return };
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let n = handle.webview_windows().len();
        eprintln!("pi-explorer: recreate requested, windows={n}");
        if n == 0 {
            if let Some(cfg) = handle.config().app.windows.first() {
                let r = tauri::WebviewWindowBuilder::from_config(&handle, cfg).and_then(|b| b.build());
                eprintln!("pi-explorer: window recreated: {:?}", r.as_ref().map(|_| ()).map_err(|e| e.to_string()));
            }
        }
    });
}

/// Android: Die App wurde im noch laufenden Prozess neu geöffnet (vorher aus den
/// „Zuletzt verwendet“ gewischt, während gerechnet wurde). Tauri bemerkt die neue Activity
/// nicht von selbst – `MainActivity` ruft das hier auf.
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_app_piexplorer_MainActivity_nativeRecreateWindow(
    _env: *mut std::ffi::c_void,
    _class: *mut std::ffi::c_void,
) {
    recreate_main_window();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .setup(|app| {
            let _ = APP.set(app.handle().clone());
            let dir = app.path().app_cache_dir().or_else(|_| app.path().app_data_dir());
            if let Ok(dir) = dir {
                let _ = std::fs::create_dir_all(&dir);
                *ENGINE.status_file.lock().unwrap() = Some(dir.join("compute-status.txt"));
                let base = dir.join(TEMP_DIR_NAME);
                *ENGINE.cache_base.lock().unwrap() = Some(base.clone());
                // Reste früherer Läufe (Absturz, hartes Beenden) entfernen – nur eigene „run-*“-Ordner
                std::thread::spawn(move || {
                    cleanup_stale(&base);
                    #[cfg(windows)]
                    for l in b'C'..=b'Z' {
                        let p = PathBuf::from(format!("{}:\\", l as char)).join(TEMP_DIR_NAME);
                        if p.exists() {
                            cleanup_stale(&p);
                        }
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            device_info,
            storage_options,
            estimate,
            start_compute,
            compute_status,
            cancel_compute,
            get_digits,
            search,
            get_hits,
            shape_search,
            get_shape_matches,
            get_stats
        ]);
    #[cfg(desktop)]
    let builder = builder.on_window_event(|window, event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            desktop::on_close(window, api);
        }
    });
    let app = builder.build(tauri::generate_context!()).expect("Tauri konnte nicht starten");
    app.run(|app, event| match &event {
        // Solange gerechnet wird, nicht beenden – auch wenn das letzte Fenster weg ist
        // (Android: App aus den „Zuletzt verwendet“ gewischt; der Hintergrunddienst hält den Prozess).
        tauri::RunEvent::ExitRequested { api, code, .. } => {
            let keep = code.is_none() && ENGINE.running.load(Ordering::SeqCst);
            eprintln!("pi-explorer: ExitRequested code={code:?} running={} keep={keep}", ENGINE.running.load(Ordering::SeqCst));
            if keep {
                api.prevent_exit();
            }
        }
        // Neue Activity im weiterlaufenden Prozess (App nach dem Wegwischen wieder geöffnet):
        // das alte Fenster ist weg, also ein neues anlegen. Die Oberfläche hängt sich dann an.
        tauri::RunEvent::Resumed => {
            let n = app.webview_windows().len();
            eprintln!("pi-explorer: Resumed, windows={n}");
            if n == 0 {
                if let Some(cfg) = app.config().app.windows.first() {
                    let r = tauri::WebviewWindowBuilder::from_config(app, cfg).and_then(|b| b.build());
                    eprintln!("pi-explorer: window recreated: {:?}", r.as_ref().map(|_| ()).map_err(|e| e.to_string()));
                }
            }
        }
        tauri::RunEvent::WindowEvent { label, event: tauri::WindowEvent::Destroyed, .. } => {
            eprintln!("pi-explorer: window {label} destroyed");
        }
        // Beim Beenden: Ziffern-Datei und Hilfsdateien löschen (werden nur so lange gebraucht)
        tauri::RunEvent::Exit => {
            ENGINE.cancel.store(true, Ordering::SeqCst);
            cleanup_run();
        }
        _ => {}
    });
}
