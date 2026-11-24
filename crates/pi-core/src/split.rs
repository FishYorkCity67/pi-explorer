//! „SPLIT“-Modus: π mit fest begrenztem RAM, nur im Speicher (keine Dateien).
//!
//! Drei Tricks halten den Speicher klein:
//! 1. Blockmultiplikation (`cmul`): große Zahlen werden in Blöcke à `chunk` Limbs zerlegt und
//!    blockweise multipliziert. Der Zusatzspeicher der Multiplikation hängt nur von `chunk` ab.
//! 2. Gleitkomma-Binary-Splitting: P, Q, T werden auf die nötige Genauigkeit gekürzt
//!    (Mantisse + Exponent in Limbs). Exakte Werte bräuchten etwa 2,5× mehr Platz.
//! 3. Division und Wurzel per Newton-Verfahren, Dezimalumwandlung blockweise per
//!    Multiplikation mit 10^B – alles aus `cmul` gebaut.
//!
//! Langsamer als der normale Modus, aber etwa 10× weniger RAM pro Stelle.

use crate::chudnovsky::{natural_to_digits, series_range, terms_for, Cancelled};
use malachite_base::num::arithmetic::traits::{FloorSqrt, ModPowerOf2, Pow};
use malachite_base::num::basic::traits::{One, Zero};
use malachite_nz::integer::Integer;
use malachite_nz::natural::Natural;
use std::sync::atomic::{AtomicBool, Ordering};

const GUARD_DIGITS: usize = 40;
const LOG2_10: f64 = 3.321_928_094_887_362;

/// Speicheraufteilung für ein RAM-Budget.
#[derive(Debug, Clone, Copy)]
pub struct Plan {
    pub budget: u64,
    /// Limbs pro Multiplikationsblock
    pub chunk: usize,
    /// Reihenglieder pro im RAM gerechnetem Blatt
    pub leaf_terms: u64,
    /// Ziffern pro Umwandlungsblock
    pub block_digits: usize,
}

impl Plan {
    pub fn for_budget(budget: u64) -> Plan {
        let b = budget.max(64 << 20) as f64;
        // Multiplikation zweier k-Limb-Blöcke: ~185 Byte pro k (gemessen) -> 18 % des Budgets
        let chunk = ((b * 0.18) / 190.0).max(8192.0) as usize;
        // Blatt im RAM: ~600 Byte pro Glied (inkl. paralleler Teilbäume) -> 15 %
        let leaf_terms = ((b * 0.15) / 600.0).max(512.0) as u64;
        // Umwandlungsblock: ~16 Byte pro Ziffer -> 8 %, höchstens ein Multiplikationsblock groß
        let block_digits = ((b * 0.08) / 16.0).min(chunk as f64 * 19.0 * 0.9).max(10_000.0) as usize;
        Plan { budget, chunk, leaf_terms, block_digits }
    }

    /// Wie viele Stellen in dieses Budget passen (konservativ).
    pub fn max_digits(&self) -> u64 {
        ((self.budget as f64 * 0.62) / BYTES_PER_DIGIT) as u64
    }
}

/// Dauerhafter Speicher pro Stelle in der Spitze (Ziffern 1 B + Zahlen), gemessen und gerundet.
pub const BYTES_PER_DIGIT: f64 = 2.6;

pub fn estimate_peak(digits: u64, plan: &Plan) -> u64 {
    let fixed = (plan.budget as f64 * 0.38) as u64;
    (digits as f64 * BYTES_PER_DIGIT) as u64 + fixed.min(digits * 40 + (48 << 20))
}

// ------------------------------------------------------------------ Blockmultiplikation

fn limbs(x: &Natural) -> usize {
    x.limb_count() as usize
}

/// Block i (à k Limbs) einer Zahl, ohne die ganze Zahl zu kopieren.
fn chunk_of(x: &Natural, i: usize, k: usize) -> Natural {
    let it = x.limbs();
    let n = it.len();
    let s = (i * k).min(n);
    let e = (s + k).min(n);
    Natural::from_owned_limbs_asc((s..e).map(|j| it[j]).collect())
}

/// a·b mit begrenztem Zusatzspeicher (exakt).
pub fn cmul(a: &Natural, b: &Natural, chunk: usize) -> Natural {
    cmul_shr(a, b, 0, chunk, true)
}

/// ≈ (a·b) >> (64·drop). Die untersten Blöcke werden gar nicht erst berechnet;
/// der Fehler liegt bei wenigen Einheiten im letzten Limb.
pub fn cmul_high(a: &Natural, b: &Natural, drop: usize, chunk: usize) -> Natural {
    cmul_shr(a, b, drop, chunk, false)
}

fn cmul_shr(a: &Natural, b: &Natural, drop: usize, chunk: usize, exact: bool) -> Natural {
    let (la, lb) = (limbs(a), limbs(b));
    if la == 0 || lb == 0 || la + lb <= drop {
        return Natural::ZERO;
    }
    if la <= chunk && lb <= chunk {
        return shr_limbs(a * b, drop);
    }
    // Ab `base` wird gesammelt; darunter (mit Sicherheitsabstand) wird nichts berechnet
    let base = if exact { 0 } else { drop.saturating_sub(chunk + 2) };
    let mut out = vec![0u64; la + lb + 1 - base];
    let na = la.div_ceil(chunk);
    let nb = lb.div_ceil(chunk);
    let b_single = (nb == 1).then(|| chunk_of(b, 0, chunk));
    for i in 0..na {
        let mut ai: Option<Natural> = None;
        for j in 0..nb {
            let off = (i + j) * chunk;
            if off + 2 * chunk <= base {
                continue; // liegt komplett unterhalb dessen, was wir behalten
            }
            let a_i = ai.get_or_insert_with(|| chunk_of(a, i, chunk));
            let prod = match &b_single {
                Some(b0) => &*a_i * b0,
                None => &*a_i * chunk_of(b, j, chunk),
            };
            let pl = prod.into_limbs_asc();
            if off >= base {
                add_limbs_at(&mut out, &pl, off - base);
            } else {
                let skip = base - off;
                if skip < pl.len() {
                    add_limbs_at(&mut out, &pl[skip..], 0);
                }
            }
        }
    }
    shr_limbs(Natural::from_owned_limbs_asc(out), drop - base)
}

fn add_limbs_at(out: &mut [u64], src: &[u64], at: usize) {
    let mut carry = false;
    let mut k = at;
    for &s in src {
        let (v1, c1) = out[k].overflowing_add(s);
        let (v2, c2) = v1.overflowing_add(carry as u64);
        out[k] = v2;
        carry = c1 || c2;
        k += 1;
    }
    while carry {
        let (v, c) = out[k].overflowing_add(1);
        out[k] = v;
        carry = c;
        k += 1;
    }
}

fn shr_limbs(x: Natural, s: usize) -> Natural {
    if s == 0 { x } else { x >> (64 * s as u64) }
}

fn top_limbs(x: &Natural, n: usize) -> (Natural, usize) {
    let l = limbs(x);
    if l <= n {
        (x.clone(), 0)
    } else {
        (x >> (64 * (l - n) as u64), l - n)
    }
}

// ------------------------------------------------------------------ Gleitkomma-Binary-Splitting

/// Wert = m · 2^(64·e)
struct F {
    m: Integer,
    e: i64,
}

impl F {
    fn exact(m: Integer) -> F {
        F { m, e: 0 }
    }
    fn trunc(mut self, cap: usize) -> F {
        let l = limbs(self.m.unsigned_abs_ref());
        if l > cap {
            let s = l - cap;
            self.m >>= 64 * s as u64;
            self.e += s as i64;
        }
        self
    }
}

/// a·b, gleich auf `cap` Limbs gekürzt (die untersten Blöcke werden gar nicht berechnet).
fn fmul(a: &F, b: &F, cap: usize, chunk: usize) -> F {
    let (am, bm) = (a.m.unsigned_abs_ref(), b.m.unsigned_abs_ref());
    let drop = (limbs(am) + limbs(bm)).saturating_sub(cap + 1);
    let neg = (a.m < 0) != (b.m < 0);
    let m = cmul_high(am, bm, drop, chunk);
    F { m: Integer::from_sign_and_abs(!neg, m), e: a.e + b.e + drop as i64 }.trunc(cap)
}

/// 2^(64n) − x als vorzeichenbehaftete Zahl, ohne eine riesige Zweierpotenz anzulegen.
fn pow2_minus(n: usize, x: Natural) -> Integer {
    let mut v = x.into_limbs_asc();
    if v.len() > n {
        // x ≥ 2^(64n): Ergebnis −(x − 2^(64n)); von Limb n eins abziehen
        let mut k = n;
        loop {
            let (r, borrow) = v[k].overflowing_sub(1);
            v[k] = r;
            if !borrow {
                break;
            }
            k += 1;
        }
        Integer::from_sign_and_abs(false, Natural::from_owned_limbs_asc(v))
    } else {
        // 2^(64n) − x = (~x + 1) auf n Limbs, plus Überlauf bei x = 0
        v.resize(n, 0);
        let mut carry = true;
        for w in v.iter_mut() {
            let (r, c) = (!*w).overflowing_add(carry as u64);
            *w = r;
            carry = c;
        }
        if carry {
            v.push(1);
        }
        Integer::from_sign_and_abs(true, Natural::from_owned_limbs_asc(v))
    }
}

fn fadd(a: F, b: F) -> F {
    use std::cmp::Ordering::*;
    match a.e.cmp(&b.e) {
        Equal => F { m: a.m + b.m, e: a.e },
        Greater => F { m: a.m + (b.m >> (64 * (a.e - b.e) as u64)), e: a.e },
        Less => F { m: (a.m >> (64 * (b.e - a.e) as u64)) + b.m, e: b.e },
    }
}

struct Node {
    p: Option<F>,
    q: F,
    t: F,
}

struct SeriesCtx<'a> {
    plan: Plan,
    cap: usize,
    cancel: &'a AtomicBool,
    total_terms: u64,
    done_terms: std::sync::atomic::AtomicU64,
    progress: &'a (dyn Fn(&'static str, f64) + Sync),
}

fn bs_split(a: u64, b: u64, need_p: bool, cx: &SeriesCtx) -> Result<Node, Cancelled> {
    if cx.cancel.load(Ordering::Relaxed) {
        return Err(Cancelled);
    }
    let chunk = cx.plan.chunk;
    if b - a <= cx.plan.leaf_terms {
        let base = cx.done_terms.load(Ordering::Relaxed);
        let span = (b - a) as f64;
        let total = cx.total_terms as f64;
        let report = |f: f64| (cx.progress)("SERIES", W_SERIES * 0.7 * (base as f64 + span * f) / total);
        let (p, q, t) = series_range(a, b, need_p, cx.cancel, &report).ok_or(Cancelled)?;
        cx.done_terms.fetch_add(b - a, Ordering::Relaxed);
        return Ok(Node {
            p: need_p.then(|| F::exact(p).trunc(cx.cap)),
            q: F::exact(q).trunc(cx.cap),
            t: F::exact(t).trunc(cx.cap),
        });
    }
    let m = (a + b) / 2;
    let l = bs_split(a, m, true, cx)?;
    let r = bs_split(m, b, need_p, cx)?;
    let lp = l.p.expect("linkes P");
    // T = Tl·Qr + Pl·Tr
    let cap = cx.cap;
    let x = fmul(&l.t, &r.q, cap, chunk);
    drop(l.t);
    let y = fmul(&lp, &r.t, cap, chunk);
    drop(r.t);
    let t = fadd(x, y).trunc(cap);
    let q = fmul(&l.q, &r.q, cap, chunk);
    drop(l.q);
    drop(r.q);
    let p = need_p.then(|| fmul(&lp, r.p.as_ref().expect("rechtes P"), cap, chunk));
    // Fortschritt: Blätter ~70 %, Kombinationen ~30 % (grob nach Position des fertigen Knotens)
    let done = cx.done_terms.load(Ordering::Relaxed) as f64 / cx.total_terms as f64;
    let combined = (b as f64 / cx.total_terms as f64).min(1.0);
    (cx.progress)("SERIES", W_SERIES * (0.7 * done + 0.3 * combined));
    Ok(Node { p, q, t })
}

// ------------------------------------------------------------------ Newton-Verfahren

/// ≈ 2^(64p) / √a
fn invsqrt(a: u64, p: usize, plan: &Plan, cancel: &AtomicBool) -> Result<Natural, Cancelled> {
    if cancel.load(Ordering::Relaxed) {
        return Err(Cancelled);
    }
    if p <= (plan.chunk / 4).max(64) {
        let num = Natural::ONE << (128 * p as u64);
        return Ok((num / Natural::from(a)).floor_sqrt());
    }
    let ph = p / 2 + 2;
    let y = invsqrt(a, ph, plan, cancel)?;
    let mut asq = cmul(&y, &y, plan.chunk);
    asq *= Natural::from(a);
    // e' = 2^(128·ph) − a·y²  (klein: die führenden Limbs heben sich auf)
    let e = pow2_minus(2 * ph, asq);
    let neg = e < 0;
    let c = cmul_high(&y, e.unsigned_abs_ref(), 3 * ph - p, plan.chunk) >> 1u64;
    drop(e);
    let y0 = y << (64 * (p - ph) as u64);
    Ok(if neg { y0 - c } else { y0 + c })
}

/// ≈ 2^(64(p+q)) / x mit q = Limbs von x
fn recip(x: &Natural, p: usize, plan: &Plan, cancel: &AtomicBool) -> Result<Natural, Cancelled> {
    if cancel.load(Ordering::Relaxed) {
        return Err(Cancelled);
    }
    if p <= (plan.chunk / 4).max(64) {
        let (xr, _) = top_limbs(x, p + 2);
        let num = Natural::ONE << (64 * (p + limbs(&xr)) as u64);
        return Ok(num / xr);
    }
    let ph = p / 2 + 2;
    let r = recip(x, ph, plan, cancel)?;
    let (xp, _) = top_limbs(x, p + 2);
    let qp = limbs(&xp);
    let prod = cmul(&xp, &r, plan.chunk);
    drop(xp);
    let e = pow2_minus(ph + qp, prod);
    let neg = e < 0;
    let c = cmul_high(&r, e.unsigned_abs_ref(), 2 * ph + qp - p, plan.chunk);
    drop(e);
    let r0 = r << (64 * (p - ph) as u64);
    Ok(if neg { r0 - c } else { r0 + c })
}

// ------------------------------------------------------------------ Gesamtablauf

const W_SERIES: f64 = 0.50;
const W_ROOTS: f64 = 0.20;
const W_MUL: f64 = 0.05;

fn limbs_for_digits(d: usize) -> usize {
    ((d as f64 * LOG2_10) / 64.0).ceil() as usize + 2
}

/// Berechnet `digits` Nachkommastellen mit dem Speicherplan `plan`.
/// `progress(phase, anteil 0..1)`.
pub fn compute_pi_split(
    digits: usize,
    plan: Plan,
    cancel: &AtomicBool,
    progress: &(dyn Fn(&'static str, f64) + Sync),
) -> Result<Vec<u8>, Cancelled> {
    // Fortschritt nie rückwärts (Blätter und Kombinationen melden unterschiedlich grob)
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
    let d = digits + GUARD_DIGITS;
    let l = limbs_for_digits(d);
    let cap = l + 4;
    let n_terms = terms_for(digits) + 1;
    progress("SERIES", 0.0);

    // 1. Reihe
    let cx = SeriesCtx {
        plan,
        cap,
        cancel,
        total_terms: n_terms,
        done_terms: Default::default(),
        progress,
    };
    let root = bs_split(0, n_terms, false, &cx)?;
    let (q, t) = (root.q, root.t);
    debug_assert!(t.m > 0);

    // 2. 1/√10005 und 1/T
    progress("ROOTS", W_SERIES);
    let y = invsqrt(10005, l, &plan, cancel)?;
    progress("ROOTS", W_SERIES + W_ROOTS * 0.4);
    let tm = Natural::try_from(t.m).map_err(|_| Cancelled)?;
    let r = recip(&tm, l + 1, &plan, cancel)?;
    let tlen = limbs(&tm) as i64;
    drop(tm);

    // 3. π·2^(64L) = U·Y·R / 2^(64(L+1+len(T) − eQ + eT)),  U = Q·426880·10005
    progress("MULTIPLY", W_SERIES + W_ROOTS);
    let qm = Natural::try_from(q.m).map_err(|_| Cancelled)?;
    let (qm, su) = top_limbs(&qm, l + 4);
    let u = qm * Natural::from(426_880u64 * 10_005);
    let s1 = (limbs(&u) + limbs(&y)).saturating_sub(l + 5);
    let uy = cmul_high(&u, &y, s1, plan.chunk);
    drop(u);
    drop(y);
    let shift = (l as i64 + 1 + tlen) - q.e + t.e - su as i64 - s1 as i64;
    let fixed = if shift >= 0 {
        cmul_high(&uy, &r, shift as usize, plan.chunk)
    } else {
        cmul(&uy, &r, plan.chunk) << (64 * (-shift) as u64)
    };
    drop(uy);
    drop(r);
    if cancel.load(Ordering::Relaxed) {
        return Err(Cancelled);
    }
    let int_part = &fixed >> (64 * l as u64);
    assert_eq!(int_part, 3u32, "Ganzzahlteil muss 3 sein");
    let mut f = fixed.mod_power_of_2(64 * l as u64);

    // 4. Dezimalumwandlung blockweise: f·10^B = Ziffernblock + neuer Rest
    progress("CONVERT", W_SERIES + W_ROOTS + W_MUL);
    let bd = plan.block_digits.min(digits.max(1));
    let p10 = Natural::from(10u32).pow(bd as u64);
    let mut out: Vec<u8> = Vec::with_capacity(digits);
    let mut lc = l;
    while out.len() < digits {
        if cancel.load(Ordering::Relaxed) {
            return Err(Cancelled);
        }
        let prod = cmul(&f, &p10, plan.chunk);
        let hi = &prod >> (64 * lc as u64);
        f = prod.mod_power_of_2(64 * lc as u64);
        let block = natural_to_digits(hi, bd, cancel).ok_or(Cancelled)?;
        let take = bd.min(digits - out.len());
        out.extend_from_slice(&block[..take]);
        // Genauigkeit an die restlichen Stellen anpassen
        let rest = digits - out.len();
        let need = limbs_for_digits(rest + GUARD_DIGITS).max(2);
        if need < lc {
            f = shr_limbs(f, lc - need);
            lc = need;
        }
        let w0 = W_SERIES + W_ROOTS + W_MUL;
        progress("CONVERT", w0 + (1.0 - w0) * out.len() as f64 / digits as f64);
    }
    progress("CONVERT", 1.0);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chudnovsky::pi_digits;

    fn tiny_plan() -> Plan {
        // winzige Blöcke erzwingen alle Zerlegungs-Pfade schon bei kleinen Stellenzahlen
        Plan { budget: 1 << 20, chunk: 300, leaf_terms: 40, block_digits: 2000 }
    }

    #[test]
    fn cmul_matches_plain() {
        let a = Natural::from(7u32).pow(30_000);
        let b = Natural::from(13u32).pow(17_000);
        assert_eq!(cmul(&a, &b, 97), &a * &b);
        assert_eq!(cmul(&b, &a, 1000), &a * &b);
        assert_eq!(cmul(&a, &Natural::from(5u32), 50), &a * Natural::from(5u32));
    }

    #[test]
    fn cmul_high_close_to_exact() {
        let a = Natural::from(7u32).pow(40_000);
        let b = Natural::from(11u32).pow(25_000);
        let exact = &a * &b;
        for drop in [0usize, 5, 300, 1500, 2500] {
            let hi = cmul_high(&a, &b, drop, 200);
            let want = &exact >> (64 * drop as u64);
            let diff = if hi > want { &hi - &want } else { &want - &hi };
            assert!(diff < 1000u32, "drop={drop}");
        }
    }

    #[test]
    fn pow2_minus_both_signs() {
        let n = 3;
        let big = Natural::ONE << 192u64;
        assert_eq!(pow2_minus(n, Natural::from(5u32)), Integer::from(&big - Natural::from(5u32)));
        assert_eq!(pow2_minus(n, Natural::ZERO), Integer::from(big.clone()));
        let x = &big + Natural::from(77u32);
        assert_eq!(pow2_minus(n, x), Integer::from(-77));
    }

    #[test]
    fn split_matches_in_memory() {
        let cancel = AtomicBool::new(false);
        for n in [1usize, 50, 999, 20_000, 61_234] {
            let s = compute_pi_split(n, tiny_plan(), &cancel, &|_, _| {}).unwrap();
            assert_eq!(s, pi_digits(n), "n={n}");
        }
    }

    #[test]
    fn split_with_realistic_plan() {
        let cancel = AtomicBool::new(false);
        let plan = Plan { budget: 64 << 20, chunk: 20_000, leaf_terms: 3_000, block_digits: 100_000 };
        let n = 400_000;
        assert_eq!(compute_pi_split(n, plan, &cancel, &|_, _| {}).unwrap(), pi_digits(n));
    }

    #[test]
    fn cancel_stops() {
        let cancel = AtomicBool::new(true);
        assert!(compute_pi_split(100_000, tiny_plan(), &cancel, &|_, _| {}).is_err());
    }
}
