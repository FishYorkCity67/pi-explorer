//! Plattenmodus („out of core“): π mit festem RAM-Budget für beliebig viele Stellen.
//!
//! Große Zwischenzahlen liegen als Dateien (Limbs, little endian) in einem Arbeitsordner und
//! werden gelöscht, sobald sie nicht mehr gebraucht werden. Gerechnet wird darauf
//! - blockweise (Addition, Subtraktion, Verschieben: immer nur ein Block im RAM),
//! - Multiplikation per Karatsuba auf Platte, bis die Teile in einen RAM-Block passen.
//! Der Ablauf ist derselbe wie im SPLIT-Modus (gekürzte Reihe, Newton, Umwandlung), nur dass
//! die Umwandlung hier rekursiv halbiert (f·10^(D/2)), damit nicht jeder Block alles neu liest.
//! Die fertigen Ziffern landen ebenfalls in einer Datei (1 Byte pro Ziffer).

use crate::chudnovsky::{natural_to_digits, series_range, terms_for, Cancelled};
use malachite_base::num::arithmetic::traits::{FloorSqrt, ModPowerOf2, Pow, UnsignedAbs};
use malachite_base::num::basic::traits::{One, Zero};
use malachite_nz::integer::Integer;
use malachite_nz::natural::Natural;
use std::cmp::Ordering;
use std::fs::{self, File};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AO};
use std::sync::Arc;

#[cfg(not(target_endian = "little"))]
compile_error!("Plattenmodus setzt little endian voraus");

const GUARD_DIGITS: usize = 40;
const LOG2_10: f64 = 3.321_928_094_887_362;
/// Limbs pro Lese-/Schreibblock bei Streaming-Operationen (8 MB)
const IO_BLOCK: usize = 1 << 20;

#[derive(Debug)]
pub enum OocError {
    Cancelled,
    Io(String),
}

impl From<std::io::Error> for OocError {
    fn from(e: std::io::Error) -> Self {
        OocError::Io(e.to_string())
    }
}

impl From<Cancelled> for OocError {
    fn from(_: Cancelled) -> Self {
        OocError::Cancelled
    }
}

type R<T> = Result<T, OocError>;

/// Speicherplan für ein RAM-Budget.
#[derive(Debug, Clone, Copy)]
pub struct OocPlan {
    /// Limbs pro RAM-Multiplikationsblock
    pub chunk: usize,
    /// Zahlen bis zu dieser Länge bleiben im RAM
    pub ram_limbs: usize,
    /// Reihenglieder pro im RAM gerechnetem Blatt
    pub leaf_terms: u64,
    /// Ziffern pro Umwandlungsblatt
    pub block_digits: usize,
    /// so viele RAM-Blockprodukte dürfen gleichzeitig laufen (Karatsuba parallel)
    pub par: usize,
}

impl OocPlan {
    pub fn for_budget(budget: u64) -> OocPlan {
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        // 1, 3 oder 9 parallele Teilprodukte (eine bzw. zwei Karatsuba-Stufen gleichzeitig) –
        // aber nur, solange die RAM-Blöcke groß bleiben; sonst überwiegt der Dateiaufwand
        // (gemessen: 256 MB mit 9 parallel ist langsamer als seriell).
        const MIN_CHUNK: f64 = 2_000_000.0;
        let seq_chunk = (budget.max(64 << 20) as f64 * CHUNK_SHARE) / 190.0;
        let par = [9usize, 3, 1]
            .into_iter()
            .find(|&p| p <= cores && seq_chunk / p as f64 >= MIN_CHUNK)
            .unwrap_or(1);
        Self::with_par(budget, par)
    }

    pub fn with_par(budget: u64, par: usize) -> OocPlan {
        let b = budget.max(64 << 20) as f64;
        // Multiplikation zweier k-Limb-Blöcke ≈ 190 Byte pro k (gemessen, eher weniger) →
        // CHUNK_SHARE des Budgets, aufgeteilt auf die parallel laufenden Produkte
        let chunk = ((b * CHUNK_SHARE) / (190.0 * par as f64)).max(4096.0) as usize;
        let ram_limbs = (chunk / 2).max(1024);
        let leaf_terms = ((b * 0.15) / 600.0).max(256.0) as u64;
        let block_digits = ((b * 0.06) / 16.0).min(chunk as f64 * 19.0 * 0.4).max(10_000.0) as usize;
        OocPlan { chunk, ram_limbs, leaf_terms, block_digits, par }
    }
}

/// Anteil des Budgets für die RAM-Blockprodukte. Gemessen: mit 0,40 nutzte 1 Mrd. Stellen bei
/// 2 GB nur 785 MB – daher mehr.
const CHUNK_SHARE: f64 = 0.85;

/// Plattenbedarf in der Spitze (Zwischenzahlen + Ziffern-Datei).
/// Gemessen: 1 Mrd. Stellen → 5,9 GB Hilfsdateien + 1 GB Ziffern; 200 Mio. → 0,9 + 0,2 GB.
pub fn estimate_disk(digits: u64) -> u64 {
    (digits as f64 * 7.0) as u64 + (64 << 20)
}

/// Faktor gegenüber dem reinen RAM-Modus (gemessen: 200 Mio./256 MB seriell 17,5×,
/// 1 Mrd./2 GB mit 3 parallel 16×): ≈ 3,3 · (Blöcke)^0,585 / √parallel
pub fn time_factor(digits: u64, plan: &OocPlan) -> f64 {
    let limbs = digits as f64 * 3.3219 / 64.0;
    let blocks = (limbs / plan.chunk as f64).max(1.0);
    3.3 * blocks.powf(0.585) / (plan.par as f64).sqrt()
}

// ------------------------------------------------------------------ Zahlen auf Platte

struct FileInner {
    path: PathBuf,
    limbs: usize,
    disk: Arc<AtomicU64>,
}

impl Drop for FileInner {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        self.disk.fetch_sub(self.limbs as u64 * 8, AO::Relaxed);
    }
}

/// Ansicht auf einen Ausschnitt einer Datei (mehrere Ansichten teilen sich eine Datei).
#[derive(Clone)]
pub struct DiskNat {
    file: Arc<FileInner>,
    off: usize,
    len: usize,
}

#[derive(Clone)]
pub enum Num {
    Ram(Natural),
    Disk(DiskNat),
}

impl Num {
    pub fn len(&self) -> usize {
        match self {
            Num::Ram(n) => n.limb_count() as usize,
            Num::Disk(d) => d.len,
        }
    }
    fn zero() -> Num {
        Num::Ram(Natural::ZERO)
    }
}

/// Vorzeichenbehaftet: (negativ?, Betrag)
#[derive(Clone)]
struct SNum {
    neg: bool,
    mag: Num,
}

pub struct Store<'a> {
    dir: PathBuf,
    next: AtomicU64,
    pub plan: OocPlan,
    cancel: &'a AtomicBool,
    /// aktuell belegte Bytes auf der Platte
    pub disk_now: Arc<AtomicU64>,
    pub disk_peak: AtomicU64,
}

struct Writer<'s, 'a> {
    st: &'s Store<'a>,
    path: PathBuf,
    w: BufWriter<File>,
    n: usize,
    nz: usize,
}

fn as_bytes(xs: &[u64]) -> &[u8] {
    // SAFETY: u64 hat keine Füllbytes; little endian ist oben erzwungen
    unsafe { std::slice::from_raw_parts(xs.as_ptr() as *const u8, xs.len() * 8) }
}

fn as_bytes_mut(xs: &mut [u64]) -> &mut [u8] {
    // SAFETY: wie oben, jedes Bitmuster ist ein gültiges u64
    unsafe { std::slice::from_raw_parts_mut(xs.as_mut_ptr() as *mut u8, xs.len() * 8) }
}

impl Writer<'_, '_> {
    fn push(&mut self, xs: &[u64]) -> R<()> {
        if let Some(p) = xs.iter().rposition(|&x| x != 0) {
            self.nz = self.n + p + 1;
        }
        self.w.write_all(as_bytes(xs))?;
        self.n += xs.len();
        Ok(())
    }

    fn zeros(&mut self, mut k: usize) -> R<()> {
        let z = vec![0u64; IO_BLOCK.min(k.max(1))];
        while k > 0 {
            let c = k.min(z.len());
            self.w.write_all(as_bytes(&z[..c]))?;
            self.n += c;
            k -= c;
        }
        Ok(())
    }

    fn finish(self) -> R<Num> {
        let Writer { st, path, w, nz, .. } = self;
        let f = w.into_inner().map_err(|e| OocError::Io(e.to_string()))?;
        f.set_len(nz as u64 * 8)?;
        drop(f);
        if nz <= st.plan.ram_limbs {
            let mut v = vec![0u64; nz];
            if nz > 0 {
                File::open(&path)?.read_exact(as_bytes_mut(&mut v))?;
            }
            fs::remove_file(&path)?;
            return Ok(Num::Ram(Natural::from_owned_limbs_asc(v)));
        }
        let now = st.disk_now.fetch_add(nz as u64 * 8, AO::Relaxed) + nz as u64 * 8;
        st.disk_peak.fetch_max(now, AO::Relaxed);
        Ok(Num::Disk(DiskNat {
            file: Arc::new(FileInner { path, limbs: nz, disk: st.disk_now.clone() }),
            off: 0,
            len: nz,
        }))
    }
}

impl<'a> Store<'a> {
    pub fn new(dir: &Path, plan: OocPlan, cancel: &'a AtomicBool) -> R<Store<'a>> {
        fs::create_dir_all(dir)?;
        Ok(Store {
            dir: dir.to_path_buf(),
            next: AtomicU64::new(0),
            plan,
            cancel,
            disk_now: Arc::new(AtomicU64::new(0)),
            disk_peak: AtomicU64::new(0),
        })
    }

    fn check(&self) -> R<()> {
        if self.cancel.load(AO::Relaxed) {
            Err(OocError::Cancelled)
        } else {
            Ok(())
        }
    }

    fn writer(&self) -> R<Writer<'_, 'a>> {
        let path = self.dir.join(format!("n{}.bin", self.next.fetch_add(1, AO::Relaxed)));
        let f = File::create(&path)?;
        Ok(Writer { st: self, path, w: BufWriter::with_capacity(8 << 20, f), n: 0, nz: 0 })
    }

    /// Limbs [start, start+count) (über das Ende hinaus Nullen)
    fn read(&self, x: &Num, start: usize, count: usize) -> R<Vec<u64>> {
        let mut v = vec![0u64; count];
        match x {
            Num::Ram(n) => {
                let it = n.limbs();
                let l = it.len();
                for (i, o) in v.iter_mut().enumerate() {
                    let j = start + i;
                    if j >= l {
                        break;
                    }
                    *o = it[j];
                }
            }
            Num::Disk(d) => {
                let s = start.min(d.len);
                let e = (start + count).min(d.len);
                if e > s {
                    let mut f = File::open(&d.file.path)?;
                    f.seek(SeekFrom::Start(((d.off + s) * 8) as u64))?;
                    f.read_exact(as_bytes_mut(&mut v[s - start..e - start]))?;
                }
            }
        }
        Ok(v)
    }

    /// Ganze Zahl in den RAM holen (nur für kleine Zahlen gedacht)
    fn nat(&self, x: &Num) -> R<Natural> {
        match x {
            Num::Ram(n) => Ok(n.clone()),
            Num::Disk(d) => Ok(Natural::from_owned_limbs_asc(self.read(x, 0, d.len)?)),
        }
    }

    fn put(&self, n: Natural) -> R<Num> {
        if (n.limb_count() as usize) <= self.plan.ram_limbs {
            return Ok(Num::Ram(n));
        }
        let mut w = self.writer()?;
        let v = n.into_limbs_asc();
        for c in v.chunks(IO_BLOCK) {
            w.push(c)?;
        }
        w.finish()
    }

    /// Limbs [start, start+count) als eigene Zahl (bei Dateien nur eine Ansicht)
    fn slice(&self, x: &Num, start: usize, count: usize) -> R<Num> {
        match x {
            Num::Ram(n) => {
                let s = (n >> (64 * start as u64)).mod_power_of_2(64 * count as u64);
                Ok(Num::Ram(s))
            }
            Num::Disk(d) => {
                let s = start.min(d.len);
                let mut e = (start + count).min(d.len);
                // führende Null-Limbs abschneiden
                while e > s {
                    let b = (e - s).min(4096);
                    let top = self.read(x, e - b, b)?;
                    match top.iter().rposition(|&v| v != 0) {
                        Some(p) => {
                            e = e - b + p + 1;
                            break;
                        }
                        None => e -= b,
                    }
                }
                let len = e - s;
                let view = Num::Disk(DiskNat { file: d.file.clone(), off: d.off + s, len });
                if len <= self.plan.ram_limbs {
                    return Ok(Num::Ram(self.nat(&view)?));
                }
                Ok(view)
            }
        }
    }

    fn shr(&self, x: &Num, s: usize) -> R<Num> {
        let l = x.len();
        if s == 0 {
            return Ok(x.clone());
        }
        if s >= l {
            return Ok(Num::zero());
        }
        self.slice(x, s, l - s)
    }

    fn low(&self, x: &Num, n: usize) -> R<Num> {
        if n >= x.len() {
            return Ok(x.clone());
        }
        self.slice(x, 0, n)
    }

    /// Eine Ansicht, die nur einen kleinen Teil ihrer Datei nutzt, in eine eigene Datei kopieren,
    /// damit die große Datei gelöscht werden kann.
    fn compact(&self, x: Num) -> R<Num> {
        match &x {
            Num::Disk(d) if d.file.limbs > d.len + d.len / 4 => {
                let mut w = self.writer()?;
                let mut i = 0;
                while i < d.len {
                    let c = IO_BLOCK.min(d.len - i);
                    w.push(&self.read(&x, i, c)?)?;
                    i += c;
                }
                w.finish()
            }
            _ => Ok(x),
        }
    }

    fn shl(&self, x: &Num, s: usize) -> R<Num> {
        if s == 0 || x.len() == 0 {
            return Ok(x.clone());
        }
        if let Num::Ram(n) = x {
            if x.len() + s <= self.plan.ram_limbs {
                return Ok(Num::Ram(n << (64 * s as u64)));
            }
        }
        let mut w = self.writer()?;
        w.zeros(s)?;
        let l = x.len();
        let mut i = 0;
        while i < l {
            let c = IO_BLOCK.min(l - i);
            w.push(&self.read(x, i, c)?)?;
            i += c;
        }
        w.finish()
    }

    /// a + b·2^(64·s)
    fn add_shifted(&self, a: &Num, b: &Num, s: usize) -> R<Num> {
        self.check()?;
        let (la, lb) = (a.len(), b.len());
        if lb == 0 {
            return Ok(a.clone());
        }
        if let (Num::Ram(x), Num::Ram(y)) = (a, b) {
            if la.max(lb + s) < self.plan.ram_limbs {
                return Ok(Num::Ram(x + (y << (64 * s as u64))));
            }
        }
        let total = la.max(lb + s);
        let mut w = self.writer()?;
        let mut carry = false;
        let mut i = 0;
        while i < total {
            let c = IO_BLOCK.min(total - i);
            let mut av = self.read(a, i, c)?;
            // b-Limbs an Position j gehören zu b[j − s]
            let bv = if i + c <= s {
                None
            } else {
                let from = i.max(s);
                Some((from - i, self.read(b, from - s, i + c - from)?))
            };
            if let Some((at, bv)) = bv {
                for (k, &bw) in bv.iter().enumerate() {
                    let (v1, c1) = av[at + k].overflowing_add(bw);
                    let (v2, c2) = v1.overflowing_add(carry as u64);
                    av[at + k] = v2;
                    carry = c1 || c2;
                }
                // Übertrag durch die Limbs vor `at` gibt es nicht (dort wird nur a kopiert),
                // aber er läuft über das Blockende hinaus weiter.
            } else if carry {
                for v in av.iter_mut() {
                    let (r, c1) = v.overflowing_add(1);
                    *v = r;
                    carry = c1;
                    if !carry {
                        break;
                    }
                }
            }
            w.push(&av)?;
            i += c;
        }
        if carry {
            w.push(&[1])?;
        }
        w.finish()
    }

    fn add(&self, a: &Num, b: &Num) -> R<Num> {
        self.add_shifted(a, b, 0)
    }

    /// a − b, a ≥ b
    fn sub(&self, a: &Num, b: &Num) -> R<Num> {
        self.check()?;
        if let (Num::Ram(x), Num::Ram(y)) = (a, b) {
            return Ok(Num::Ram(x - y));
        }
        let la = a.len();
        let mut w = self.writer()?;
        let mut borrow = false;
        let mut i = 0;
        while i < la {
            let c = IO_BLOCK.min(la - i);
            let mut av = self.read(a, i, c)?;
            let bv = self.read(b, i, c)?;
            for k in 0..c {
                let (v1, b1) = av[k].overflowing_sub(bv[k]);
                let (v2, b2) = v1.overflowing_sub(borrow as u64);
                av[k] = v2;
                borrow = b1 || b2;
            }
            w.push(&av)?;
            i += c;
        }
        debug_assert!(!borrow, "sub: a < b");
        w.finish()
    }

    fn cmp(&self, a: &Num, b: &Num) -> R<Ordering> {
        let (la, lb) = (a.len(), b.len());
        if la != lb {
            return Ok(la.cmp(&lb));
        }
        if let (Num::Ram(x), Num::Ram(y)) = (a, b) {
            return Ok(x.cmp(y));
        }
        let mut e = la;
        while e > 0 {
            let c = IO_BLOCK.min(e);
            let av = self.read(a, e - c, c)?;
            let bv = self.read(b, e - c, c)?;
            for k in (0..c).rev() {
                match av[k].cmp(&bv[k]) {
                    Ordering::Equal => {}
                    o => return Ok(o),
                }
            }
            e -= c;
        }
        Ok(Ordering::Equal)
    }

    fn mul_small(&self, a: &Num, m: u64) -> R<Num> {
        if let Num::Ram(x) = a {
            return self.put(x * Natural::from(m));
        }
        let la = a.len();
        let mut w = self.writer()?;
        let mut carry: u64 = 0;
        let mut i = 0;
        while i < la {
            let c = IO_BLOCK.min(la - i);
            let mut av = self.read(a, i, c)?;
            for v in av.iter_mut() {
                let p = *v as u128 * m as u128 + carry as u128;
                *v = p as u64;
                carry = (p >> 64) as u64;
            }
            w.push(&av)?;
            i += c;
        }
        if carry != 0 {
            w.push(&[carry])?;
        }
        w.finish()
    }

    /// Halbieren (Rechtsverschiebung um 1 Bit)
    fn half(&self, a: &Num) -> R<Num> {
        if let Num::Ram(x) = a {
            return Ok(Num::Ram(x >> 1u64));
        }
        let la = a.len();
        let mut w = self.writer()?;
        let mut i = 0;
        while i < la {
            let c = IO_BLOCK.min(la - i);
            let mut av = self.read(a, i, c + 1)?; // ein Limb mehr für das Übertragsbit
            for k in 0..c {
                av[k] = (av[k] >> 1) | (av[k + 1] << 63);
            }
            w.push(&av[..c])?;
            i += c;
        }
        w.finish()
    }

    /// 2^(64n) − x (vorzeichenbehaftet)
    fn pow2_minus(&self, n: usize, x: Num) -> R<SNum> {
        if let Num::Ram(v) = &x {
            if n <= self.plan.ram_limbs {
                let p = Integer::from(Natural::ONE << (64 * n as u64));
                let r = p - Integer::from(v.clone());
                return Ok(SNum { neg: r < 0, mag: Num::Ram(r.unsigned_abs()) });
            }
        }
        let lx = x.len();
        let mut w = self.writer()?;
        if lx > n {
            // x ≥ 2^(64n): −(x − 2^(64n))
            let mut borrow = false;
            let mut i = 0;
            while i < lx {
                let c = IO_BLOCK.min(lx - i);
                let mut v = self.read(&x, i, c)?;
                for k in 0..c {
                    let j = i + k;
                    if j == n {
                        borrow = true;
                    }
                    if j >= n && borrow {
                        let (r, b) = v[k].overflowing_sub(1);
                        v[k] = r;
                        borrow = b;
                    }
                }
                w.push(&v)?;
                i += c;
            }
            Ok(SNum { neg: true, mag: w.finish()? })
        } else {
            // 2^(64n) − x = (~x + 1) auf n Limbs
            let mut carry = true;
            let mut i = 0;
            while i < n {
                let c = IO_BLOCK.min(n - i);
                let mut v = self.read(&x, i, c)?;
                for e in v.iter_mut() {
                    let (r, cc) = (!*e).overflowing_add(carry as u64);
                    *e = r;
                    carry = cc;
                }
                w.push(&v)?;
                i += c;
            }
            if carry {
                w.push(&[1])?;
            }
            Ok(SNum { neg: false, mag: w.finish()? })
        }
    }

    /// a·b exakt. RAM-Blöcke, darüber Karatsuba auf Platte (oberste Stufen parallel).
    pub fn mul(&self, a: &Num, b: &Num) -> R<Num> {
        self.mul_slots(a, b, self.plan.par)
    }

    /// `slots` = wie viele RAM-Blockprodukte in diesem Teilbaum gleichzeitig laufen dürfen
    fn mul_slots(&self, a: &Num, b: &Num, slots: usize) -> R<Num> {
        self.check()?;
        let (mut a, mut b) = (a, b);
        let (mut la, mut lb) = (a.len(), b.len());
        if la == 0 || lb == 0 {
            return Ok(Num::zero());
        }
        if la < lb {
            std::mem::swap(&mut a, &mut b);
            std::mem::swap(&mut la, &mut lb);
        }
        let k = self.plan.chunk;
        if la <= k {
            // beide passen in einen Block
            let x = self.nat(a)?;
            let y = self.nat(b)?;
            return self.put(&x * &y);
        }
        if lb <= k {
            // b klein: a blockweise durchlaufen
            let y = self.nat(b)?;
            let mut w = self.writer()?;
            let mut carry = Natural::ZERO;
            let mut i = 0;
            while i < la {
                self.check()?;
                let c = k.min(la - i);
                let ai = Natural::from_owned_limbs_asc(self.read(a, i, c)?);
                let p = &ai * &y + std::mem::take(&mut carry);
                let mut pl = p.into_limbs_asc();
                if i + c < la {
                    if pl.len() > c {
                        carry = Natural::from_owned_limbs_asc(pl.split_off(c));
                    }
                    pl.resize(c, 0);
                }
                w.push(&pl)?;
                i += c;
            }
            return w.finish();
        }
        if la >= 2 * lb {
            // unausgewogen: a in Stücke der Länge lb teilen
            let mut res = Num::zero();
            let mut j = 0;
            while j < la {
                let piece = self.slice(a, j, lb.min(la - j))?;
                let prod = self.mul_slots(&piece, b, slots)?;
                res = self.add_shifted(&res, &prod, j)?;
                j += lb;
            }
            return Ok(res);
        }
        // Karatsuba
        let m = la.div_ceil(2);
        let a0 = self.slice(a, 0, m)?;
        let a1 = self.slice(a, m, la - m)?;
        let b0 = self.slice(b, 0, m)?;
        let b1 = self.slice(b, m, lb.saturating_sub(m))?;
        let sa = self.add(&a0, &a1)?;
        let sb = self.add(&b0, &b1)?;
        let (z0, z2, z1) = if slots >= 3 {
            // die drei Teilprodukte gleichzeitig (jedes mit einem Drittel der Plätze)
            let s = slots / 3;
            let ((z0, z2), z1) = rayon::join(
                || rayon::join(|| self.mul_slots(&a0, &b0, s), || self.mul_slots(&a1, &b1, s)),
                || self.mul_slots(&sa, &sb, s),
            );
            (z0?, z2?, z1?)
        } else {
            let z0 = self.mul_slots(&a0, &b0, 1)?;
            let z2 = self.mul_slots(&a1, &b1, 1)?;
            let z1 = self.mul_slots(&sa, &sb, 1)?;
            (z0, z2, z1)
        };
        drop((a0, a1, b0, b1, sa, sb));
        let z1 = self.sub(&z1, &z0)?;
        let z1 = self.sub(&z1, &z2)?;
        let r = self.add_shifted(&z0, &z1, m)?;
        drop((z0, z1));
        self.add_shifted(&r, &z2, 2 * m)
    }

    fn sadd(&self, x: SNum, y: SNum) -> R<SNum> {
        if x.neg == y.neg {
            return Ok(SNum { neg: x.neg, mag: self.add(&x.mag, &y.mag)? });
        }
        match self.cmp(&x.mag, &y.mag)? {
            Ordering::Less => Ok(SNum { neg: y.neg, mag: self.sub(&y.mag, &x.mag)? }),
            _ => Ok(SNum { neg: x.neg, mag: self.sub(&x.mag, &y.mag)? }),
        }
    }

    /// (oberste n Limbs, Anzahl abgeschnittener Limbs)
    fn top(&self, x: &Num, n: usize) -> R<(Num, usize)> {
        let l = x.len();
        if l <= n {
            Ok((x.clone(), 0))
        } else {
            Ok((self.shr(x, l - n)?, l - n))
        }
    }
}

// ------------------------------------------------------------------ Gleitkomma-Binary-Splitting

struct F {
    m: SNum,
    e: i64,
}

struct Node {
    p: Option<F>,
    q: F,
    t: F,
}

fn int_to_snum(st: &Store, x: Integer) -> R<SNum> {
    let neg = x < 0;
    Ok(SNum { neg, mag: st.put(x.unsigned_abs())? })
}

fn trunc(st: &Store, f: F, cap: usize) -> R<F> {
    let l = f.m.mag.len();
    if l <= cap {
        return Ok(f);
    }
    let s = l - cap;
    let mag = st.compact(st.shr(&f.m.mag, s)?)?;
    Ok(F { m: SNum { neg: f.m.neg, mag }, e: f.e + s as i64 })
}

fn fmul(st: &Store, a: &F, b: &F, cap: usize) -> R<F> {
    let mag = st.mul(&a.m.mag, &b.m.mag)?;
    trunc(st, F { m: SNum { neg: a.m.neg != b.m.neg, mag }, e: a.e + b.e }, cap)
}

fn fadd(st: &Store, a: F, b: F, cap: usize) -> R<F> {
    let (a, b) = if a.e >= b.e { (a, b) } else { (b, a) };
    // b hat den kleineren Exponenten: seine untersten Limbs fallen weg
    let d = (a.e - b.e) as usize;
    let bm = SNum { neg: b.m.neg, mag: st.shr(&b.m.mag, d)? };
    let m = st.sadd(a.m, bm)?;
    trunc(st, F { m, e: a.e }, cap)
}

struct Ctx<'s, 'a> {
    st: &'s Store<'a>,
    cap: usize,
    total_terms: u64,
    done_terms: AtomicU64,
    progress: &'s (dyn Fn(&'static str, f64) + Sync),
}

const W_SERIES: f64 = 0.45;
const W_ROOTS: f64 = 0.20;
const W_MUL: f64 = 0.05;

fn bs(a: u64, b: u64, need_p: bool, cx: &Ctx) -> R<Node> {
    let st = cx.st;
    st.check()?;
    if b - a <= st.plan.leaf_terms {
        let base = cx.done_terms.load(AO::Relaxed) as f64;
        let span = (b - a) as f64;
        let total = cx.total_terms as f64;
        let report = |f: f64| (cx.progress)("SERIES", W_SERIES * 0.7 * (base + span * f) / total);
        let (p, q, t) = series_range(a, b, need_p, st.cancel, &report).ok_or(OocError::Cancelled)?;
        cx.done_terms.fetch_add(b - a, AO::Relaxed);
        let mk = |x: Integer| -> R<F> { trunc(st, F { m: int_to_snum(st, x)?, e: 0 }, cx.cap) };
        return Ok(Node { p: if need_p { Some(mk(p)?) } else { None }, q: mk(q)?, t: mk(t)? });
    }
    let m = (a + b) / 2;
    let l = bs(a, m, true, cx)?;
    let r = bs(m, b, need_p, cx)?;
    let lp = l.p.expect("linkes P");
    let x = fmul(st, &l.t, &r.q, cx.cap)?;
    drop(l.t);
    let y = fmul(st, &lp, &r.t, cx.cap)?;
    drop(r.t);
    let t = fadd(st, x, y, cx.cap)?;
    let q = fmul(st, &l.q, &r.q, cx.cap)?;
    drop(l.q);
    drop(r.q);
    let p = match (need_p, r.p) {
        (true, Some(rp)) => Some(fmul(st, &lp, &rp, cx.cap)?),
        _ => None,
    };
    let done = cx.done_terms.load(AO::Relaxed) as f64 / cx.total_terms as f64;
    let combined = (b as f64 / cx.total_terms as f64).min(1.0);
    (cx.progress)("SERIES", W_SERIES * (0.7 * done + 0.3 * combined));
    Ok(Node { p, q, t })
}

// ------------------------------------------------------------------ Newton-Verfahren

fn ram_threshold(st: &Store) -> usize {
    (st.plan.chunk / 4).max(64).min(st.plan.ram_limbs)
}

/// ≈ 2^(64p) / √a
fn invsqrt(st: &Store, a: u64, p: usize) -> R<Num> {
    st.check()?;
    if p <= ram_threshold(st) {
        let num = Natural::ONE << (128 * p as u64);
        return st.put((num / Natural::from(a)).floor_sqrt());
    }
    let ph = p / 2 + 2;
    let y = invsqrt(st, a, ph)?;
    let sq = st.mul(&y, &y)?;
    let asq = st.mul_small(&sq, a)?;
    drop(sq);
    let e = st.pow2_minus(2 * ph, asq)?;
    let c = st.mul(&y, &e.mag)?;
    let c = st.half(&st.shr(&c, 3 * ph - p)?)?;
    let y0 = st.shl(&y, p - ph)?;
    drop(y);
    if e.neg {
        st.sub(&y0, &c)
    } else {
        st.add(&y0, &c)
    }
}

/// ≈ 2^(64(p+q)) / x, q = Limbs von x
fn recip(st: &Store, x: &Num, p: usize) -> R<Num> {
    st.check()?;
    if p <= ram_threshold(st) {
        let (xr, _) = st.top(x, p + 2)?;
        let xr = st.nat(&xr)?;
        let num = Natural::ONE << (64 * (p + xr.limb_count() as usize) as u64);
        return st.put(num / xr);
    }
    let ph = p / 2 + 2;
    let r = recip(st, x, ph)?;
    let (xp, _) = st.top(x, p + 2)?;
    let qp = xp.len();
    let prod = st.mul(&xp, &r)?;
    drop(xp);
    let e = st.pow2_minus(ph + qp, prod)?;
    let c = st.mul(&r, &e.mag)?;
    let c = st.shr(&c, 2 * ph + qp - p)?;
    let r0 = st.shl(&r, p - ph)?;
    drop(r);
    if e.neg {
        st.sub(&r0, &c)
    } else {
        st.add(&r0, &c)
    }
}

// ------------------------------------------------------------------ Dezimalumwandlung

fn limbs_for_digits(d: usize) -> usize {
    ((d as f64 * LOG2_10) / 64.0).ceil() as usize + 2
}

struct Conv<'s, 'a, 'w> {
    st: &'s Store<'a>,
    out: &'w mut BufWriter<File>,
    written: usize,
    total: usize,
    /// powers[j] = 10^(B·2^j)
    powers: Vec<Num>,
    p_block: Natural,
    progress: &'s (dyn Fn(&'static str, f64) + Sync),
}

impl Conv<'_, '_, '_> {
    fn power(&mut self, j: usize) -> R<Num> {
        while self.powers.len() <= j {
            let next = match self.powers.last() {
                None => self.st.put(self.p_block.clone())?,
                Some(last) => {
                    let last = last.clone();
                    self.st.mul(&last, &last)?
                }
            };
            self.powers.push(next);
        }
        Ok(self.powers[j].clone())
    }

    /// Schreibt die ersten `d` Dezimalstellen des Bruchs f / 2^(64·l).
    fn run(&mut self, f: Num, l: usize, d: usize) -> R<()> {
        self.st.check()?;
        let bd = self.st.plan.block_digits;
        if d <= bd {
            let lb = limbs_for_digits(d + GUARD_DIGITS).min(l);
            let ft = self.st.nat(&self.st.shr(&f, l - lb)?)?;
            let pw = if d == bd { self.p_block.clone() } else { Natural::from(10u32).pow(d as u64) };
            let hi = (&ft * &pw) >> (64 * lb as u64);
            let digits = natural_to_digits(hi, d, self.st.cancel).ok_or(OocError::Cancelled)?;
            self.out.write_all(&digits)?;
            self.written += d;
            let w0 = W_SERIES + W_ROOTS + W_MUL;
            (self.progress)("CONVERT", w0 + (1.0 - w0) * self.written as f64 / self.total as f64);
            return Ok(());
        }
        // d1 = größtes B·2^j < d
        let mut j = 0;
        while bd << (j + 1) < d {
            j += 1;
        }
        let d1 = bd << j;
        let d2 = d - d1;
        // erste Hälfte: f mit Genauigkeit für d1 Stellen
        let l1 = limbs_for_digits(d1 + GUARD_DIGITS).min(l);
        let f1 = self.st.compact(self.st.shr(&f, l - l1)?)?;
        self.run(f1, l1, d1)?;
        // zweite Hälfte: Nachkommateil von f·10^d1
        let p = self.power(j)?;
        let prod = self.st.mul(&f, &p)?;
        drop(f);
        let f2 = self.st.low(&prod, l)?;
        drop(prod);
        let l2 = limbs_for_digits(d2 + GUARD_DIGITS).min(l);
        let f2 = self.st.compact(self.st.shr(&f2, l - l2)?)?;
        self.run(f2, l2, d2)
    }
}

// ------------------------------------------------------------------ Gesamtablauf

pub struct OocResult {
    pub disk_peak: u64,
}

/// Berechnet `digits` Nachkommastellen; Ziffern (je 1 Byte) nach `out_path`,
/// Zwischenzahlen in `work_dir` (wird danach gelöscht).
pub fn compute_pi_ooc(
    digits: usize,
    plan: OocPlan,
    work_dir: &Path,
    out_path: &Path,
    cancel: &AtomicBool,
    progress: &(dyn Fn(&'static str, f64) + Sync),
) -> Result<OocResult, OocError> {
    let best = std::sync::Mutex::new(0.0f64);
    let outer = progress;
    let mono = |ph: &'static str, f: f64| {
        let mut b = best.lock().unwrap();
        if f > *b {
            *b = f;
        }
        outer(ph, *b);
    };
    let progress: &(dyn Fn(&'static str, f64) + Sync) = &mono;

    let res = (|| -> R<OocResult> {
        let st = Store::new(work_dir, plan, cancel)?;
        let d = digits + GUARD_DIGITS;
        let l = limbs_for_digits(d);
        let cap = l + 4;
        let n_terms = terms_for(digits) + 1;
        progress("SERIES", 0.0);

        // 1. Reihe
        let cx = Ctx { st: &st, cap, total_terms: n_terms, done_terms: AtomicU64::new(0), progress };
        let root = bs(0, n_terms, false, &cx)?;
        let (q, t) = (root.q, root.t);
        drop(root.p);

        // 2. 1/√10005 und 1/T
        progress("ROOTS", W_SERIES);
        let y = invsqrt(&st, 10005, l)?;
        progress("ROOTS", W_SERIES + W_ROOTS * 0.4);
        let tlen = t.m.mag.len() as i64;
        let r = recip(&st, &t.m.mag, l + 1)?;
        let te = t.e;
        drop(t);

        // 3. π·2^(64L) = U·Y·R / 2^(64(L+1+len(T) − eQ + eT)),  U = Q·426880·10005
        progress("MULTIPLY", W_SERIES + W_ROOTS);
        let (qm, su) = st.top(&q.m.mag, l + 4)?;
        let qe = q.e;
        drop(q);
        let u = st.mul_small(&qm, 426_880u64 * 10_005)?;
        drop(qm);
        let uy = st.mul(&u, &y)?;
        drop((u, y));
        let s1 = uy.len().saturating_sub(l + 5);
        let uy = st.compact(st.shr(&uy, s1)?)?;
        let shift = (l as i64 + 1 + tlen) - qe + te - su as i64 - s1 as i64;
        let full = st.mul(&uy, &r)?;
        drop((uy, r));
        let fixed = if shift >= 0 { st.shr(&full, shift as usize)? } else { st.shl(&full, (-shift) as usize)? };
        drop(full);
        let int_part = st.nat(&st.shr(&fixed, l)?)?;
        if int_part != 3u32 {
            return Err(OocError::Io(format!("internal check failed (integer part {int_part})")));
        }
        let f = st.compact(st.low(&fixed, l)?)?;
        drop(fixed);

        // 4. Dezimalumwandlung
        progress("CONVERT", W_SERIES + W_ROOTS + W_MUL);
        let mut out = BufWriter::with_capacity(8 << 20, File::create(out_path)?);
        let bd = plan.block_digits;
        let mut conv = Conv {
            st: &st,
            out: &mut out,
            written: 0,
            total: digits,
            powers: Vec::new(),
            p_block: Natural::from(10u32).pow(bd as u64),
            progress,
        };
        conv.run(f, l, digits)?;
        drop(conv);
        out.flush()?;
        let peak = st.disk_peak.load(AO::Relaxed);
        progress("CONVERT", 1.0);
        Ok(OocResult { disk_peak: peak })
    })();
    let _ = fs::remove_dir_all(work_dir);
    if res.is_err() {
        let _ = fs::remove_file(out_path);
    }
    res
}

/// Bequemer Leser für die Ziffern-Datei.
pub fn read_digit_file(path: &Path, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
    let mut f = File::open(path)?;
    let size = f.metadata()?.len();
    if offset >= size {
        return Ok(Vec::new());
    }
    let len = len.min((size - offset) as usize);
    let mut v = vec![0u8; len];
    f.seek(SeekFrom::Start(offset))?;
    f.read_exact(&mut v)?;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chudnovsky::pi_digits;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pi-ooc-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn tiny() -> OocPlan {
        OocPlan { chunk: 97, ram_limbs: 40, leaf_terms: 25, block_digits: 700, par: 9 }
    }

    #[test]
    fn disk_mul_matches_ram() {
        let cancel = AtomicBool::new(false);
        let dir = tmp("mul");
        let st = Store::new(&dir, tiny(), &cancel).unwrap();
        for (x, y) in [(30_000u64, 17_000u64), (9_000, 9_100), (40_000, 2_000), (5_000, 300)] {
            let a = Natural::from(7u32).pow(x);
            let b = Natural::from(13u32).pow(y);
            let want = &a * &b;
            let na = st.put(a.clone()).unwrap();
            let nb = st.put(b.clone()).unwrap();
            assert!(matches!(na, Num::Disk(_)));
            let got = st.nat(&st.mul(&na, &nb).unwrap()).unwrap();
            assert_eq!(got, want, "x={x} y={y}");
            // Addition, Subtraktion, Verschiebung
            let s = st.nat(&st.add_shifted(&na, &nb, 37).unwrap()).unwrap();
            assert_eq!(s, &a + (&b << (64 * 37u64)));
            let big = st.put(&a * Natural::from(5u32)).unwrap();
            assert_eq!(st.nat(&st.sub(&big, &na).unwrap()).unwrap(), &a * Natural::from(4u32));
            assert_eq!(st.nat(&st.half(&na).unwrap()).unwrap(), &a >> 1u64);
            assert_eq!(st.nat(&st.mul_small(&na, 12345).unwrap()).unwrap(), &a * Natural::from(12345u32));
            assert_eq!(st.nat(&st.shl(&nb, 300).unwrap()).unwrap(), &b << (64 * 300u64));
        }
        drop(st);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn disk_pow2_minus() {
        let cancel = AtomicBool::new(false);
        let dir = tmp("p2m");
        let st = Store::new(&dir, tiny(), &cancel).unwrap();
        let n = 300;
        let p = Natural::ONE << (64 * n as u64);
        let small = Natural::from(3u32).pow(10_000);
        let r = st.pow2_minus(n, st.put(small.clone()).unwrap()).unwrap();
        assert!(!r.neg);
        assert_eq!(st.nat(&r.mag).unwrap(), &p - &small);
        let over = &p + Natural::from(99u32).pow(2000);
        let r = st.pow2_minus(n, st.put(over.clone()).unwrap()).unwrap();
        assert!(r.neg);
        assert_eq!(st.nat(&r.mag).unwrap(), &over - &p);
        drop(st);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ooc_matches_in_memory() {
        let cancel = AtomicBool::new(false);
        for n in [1usize, 99, 2_500, 40_000, 77_777] {
            let dir = tmp(&format!("pi{n}"));
            let out = dir.with_extension("digits");
            compute_pi_ooc(n, tiny(), &dir, &out, &cancel, &|_, _| {}).unwrap();
            let got = fs::read(&out).unwrap();
            let _ = fs::remove_file(&out);
            assert_eq!(got, pi_digits(n), "n={n}");
            assert!(!dir.exists(), "Arbeitsordner muss weg sein");
        }
    }

    #[test]
    fn cancel_cleans_up() {
        let cancel = AtomicBool::new(true);
        let dir = tmp("cancel");
        let out = dir.with_extension("digits");
        assert!(compute_pi_ooc(50_000, tiny(), &dir, &out, &cancel, &|_, _| {}).is_err());
        assert!(!dir.exists());
        assert!(!out.exists());
    }
}
