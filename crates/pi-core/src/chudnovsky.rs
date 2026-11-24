//! Pi nach Chudnovsky mit Binary Splitting.
//!
//! 1/π = 12 Σ (-1)^k (6k)! (13591409 + 545140134k) / ((3k)! (k!)³ 640320^(3k+3/2))
//!
//! Nichts ist vorgespeichert: Die Reihe wird aufsummiert, √10005 per Ganzzahl-Wurzel
//! gezogen und das Ergebnis per Teile-und-herrsche in Dezimalziffern umgewandelt.

use malachite_base::num::arithmetic::traits::{DivMod, FloorSqrt, Pow};
use malachite_nz::integer::Integer;
use malachite_nz::natural::Natural;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

/// Jedes Reihenglied liefert log10(640320³/1728) ≈ 14,18 Stellen.
pub const DIGITS_PER_TERM: f64 = 14.181_647_462_725_477;
/// Schutzstellen gegen Rundungsfehler am Ende.
const GUARD: usize = 24;
/// 640320³ / 24
const C3_OVER_24: u64 = 10_939_058_860_032_000;
const A: u64 = 13_591_409;
const B: u64 = 545_140_134;
/// Ziffern pro Blatt bei der Dezimalumwandlung.
const LEAF_DIGITS: usize = 1 << 10;
const LEAF_TERMS: u64 = 16;
const PAR_TERMS: u64 = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Series,
    Divide,
    Convert,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Series => "SERIES",
            Phase::Divide => "DIVIDE",
            Phase::Convert => "CONVERT",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cancelled;

/// Zeitanteile der Phasen am Gesamtfortschritt (gemessen, siehe Benchmark).
pub const W_SERIES: f64 = 0.55;
pub const W_DIVIDE: f64 = 0.20;

#[derive(Debug, Clone, Default)]
pub struct Timings {
    pub series_s: f64,
    pub divide_s: f64,
    pub convert_s: f64,
    pub total_s: f64,
}

struct Ctx<'a> {
    cancel: &'a AtomicBool,
    progress: &'a (dyn Fn(Phase, f64) + Sync),
    work_done: AtomicU64,
    work_total: u64,
    last_report: AtomicU64,
}

impl Ctx<'_> {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    fn add_work(&self, w: u64, phase: Phase, base: f64, span: f64) {
        let done = self.work_done.fetch_add(w, Ordering::Relaxed) + w;
        // höchstens etwa 500 Meldungen pro Phase
        let step = (self.work_total / 500).max(1);
        let bucket = done / step;
        if self.last_report.fetch_max(bucket, Ordering::Relaxed) < bucket {
            let f = (done as f64 / self.work_total.max(1) as f64).min(1.0);
            (self.progress)(phase, base + span * f);
        }
    }
}

/// Anzahl der Reihenglieder für `digits` Nachkommastellen (inkl. Schutzstellen).
pub fn terms_for(digits: usize) -> u64 {
    ((digits + GUARD) as f64 / DIGITS_PER_TERM) as u64 + 2
}

fn node_work(n: u64) -> u64 {
    // Kosten eines Knotens ~ Größe · log(Größe)
    let lg = 64 - n.leading_zeros() as u64;
    n * lg.max(1)
}

fn total_work(a: u64, b: u64) -> u64 {
    if b - a <= LEAF_TERMS {
        return node_work(b - a);
    }
    let m = (a + b) / 2;
    node_work(b - a) + total_work(a, m) + total_work(m, b)
}

/// Ein Reihenglied k: (P, Q, T).
fn term(k: u64) -> (Integer, Integer, Integer) {
    if k == 0 {
        return (Integer::from(1u32), Integer::from(1u32), Integer::from(A));
    }
    let k128 = k as u128;
    let p = Integer::from((6 * k128 - 5) * (2 * k128 - 1) * (6 * k128 - 1));
    let q = Integer::from(k128 * k128 * k128) * Integer::from(C3_OVER_24);
    let mut t = &p * Integer::from(A as u128 + B as u128 * k128);
    if k & 1 == 1 {
        t = -t;
    }
    (p, q, t)
}

/// Binary Splitting über [a, b). P wird nur berechnet, wenn der Aufrufer es braucht.
fn bs(a: u64, b: u64, need_p: bool, ctx: &Ctx) -> Option<(Integer, Integer, Integer)> {
    if ctx.cancelled() {
        return None;
    }
    if b - a <= LEAF_TERMS {
        let (mut p, mut q, mut t) = term(a);
        for k in a + 1..b {
            let (pk, qk, tk) = term(k);
            // T = T·Qk + P·Tk ; P = P·Pk ; Q = Q·Qk
            t = &t * &qk + &p * &tk;
            p *= pk;
            q *= qk;
        }
        ctx.add_work(node_work(b - a), Phase::Series, 0.0, W_SERIES);
        return Some((p, q, t));
    }
    let m = (a + b) / 2;
    let (left, right) = if b - a >= PAR_TERMS {
        rayon::join(|| bs(a, m, true, ctx), || bs(m, b, need_p, ctx))
    } else {
        (bs(a, m, true, ctx), bs(m, b, need_p, ctx))
    };
    let (pam, qam, tam) = left?;
    let (pmb, qmb, tmb) = right?;
    let zero = || Integer::from(0u32);
    let (t, (p, q)) = if b - a >= PAR_TERMS * 4 {
        rayon::join(
            || &tam * &qmb + &pam * &tmb,
            || {
                rayon::join(
                    || if need_p { &pam * &pmb } else { zero() },
                    || &qam * &qmb,
                )
            },
        )
    } else {
        let t = &tam * &qmb + &pam * &tmb;
        let p = if need_p { &pam * &pmb } else { zero() };
        (t, (p, &qam * &qmb))
    };
    ctx.add_work(node_work(b - a), Phase::Series, 0.0, W_SERIES);
    Some((p, q, t))
}

fn to_natural(x: Integer) -> Natural {
    Natural::try_from(x).expect("positiver Wert erwartet")
}

/// Berechnet die ersten `digits` Nachkommastellen von π (Werte 0..=9, eine Ziffer pro Byte).
/// `progress(phase, anteil)` mit anteil in 0..=1 für die gesamte Rechnung.
pub fn compute_pi_digits(
    digits: usize,
    cancel: &AtomicBool,
    progress: &(dyn Fn(Phase, f64) + Sync),
) -> Result<(Vec<u8>, Timings), Cancelled> {
    let start = Instant::now();
    let mut timings = Timings::default();
    let n_terms = terms_for(digits);
    let prec = digits + GUARD;
    let ctx = Ctx {
        cancel,
        progress,
        work_done: AtomicU64::new(0),
        work_total: total_work(0, n_terms),
        last_report: AtomicU64::new(0),
    };
    if cancel.load(Ordering::Relaxed) {
        return Err(Cancelled);
    }
    progress(Phase::Series, 0.0);

    // Reihe und √(10005·10^(2·prec)) laufen parallel.
    let (series, sqrt_c) = rayon::join(
        || bs(0, n_terms, false, &ctx),
        || {
            let one_sq = Natural::from(10u32).pow(2 * prec as u64);
            (one_sq * Natural::from(10005u32)).floor_sqrt()
        },
    );
    let (_, q, t) = series.ok_or(Cancelled)?;
    timings.series_s = start.elapsed().as_secs_f64();
    if cancel.load(Ordering::Relaxed) {
        return Err(Cancelled);
    }

    progress(Phase::Divide, W_SERIES);
    let t0 = Instant::now();
    let q = to_natural(q);
    let t = to_natural(t);
    // π·10^prec = 426880 · √10005 · Q / T
    let num = (q * Natural::from(426_880u32)) * sqrt_c;
    let pi_scaled = num / t;
    timings.divide_s = t0.elapsed().as_secs_f64();
    if cancel.load(Ordering::Relaxed) {
        return Err(Cancelled);
    }

    progress(Phase::Convert, W_SERIES + W_DIVIDE);
    let t0 = Instant::now();
    let total_len = prec + 1; // "3" + prec Nachkommastellen
    let mut out = vec![0u8; total_len];
    let conv_ctx = Ctx {
        cancel,
        progress,
        work_done: AtomicU64::new(0),
        work_total: total_len as u64,
        last_report: AtomicU64::new(0),
    };
    let powers = powers_of_ten(total_len);
    to_decimal(pi_scaled, &mut out, &powers, &conv_ctx).ok_or(Cancelled)?;
    drop(powers);
    timings.convert_s = t0.elapsed().as_secs_f64();
    assert_eq!(out[0], 3, "Ganzzahlteil muss 3 sein");
    out.truncate(digits + 1);
    out.remove(0);
    out.shrink_to_fit();
    timings.total_s = start.elapsed().as_secs_f64();
    progress(Phase::Convert, 1.0);
    Ok((out, timings))
}

/// powers[i] = 10^(LEAF_DIGITS · 2^i), solange LEAF·2^i < len
fn powers_of_ten(len: usize) -> Vec<Natural> {
    let mut v = vec![Natural::from(10u32).pow(LEAF_DIGITS as u64)];
    while (LEAF_DIGITS << v.len()) < len {
        let last = v.last().unwrap();
        let sq = last * last;
        v.push(sq);
    }
    v
}

/// Schreibt `x` mit genau `out.len()` Ziffern (führende Nullen) in `out`.
fn to_decimal(x: Natural, out: &mut [u8], powers: &[Natural], ctx: &Ctx) -> Option<()> {
    if ctx.cancelled() {
        return None;
    }
    let len = out.len();
    if len <= LEAF_DIGITS {
        let s = x.to_string();
        let s = s.as_bytes();
        let pad = len.saturating_sub(s.len());
        out[..pad].fill(0);
        for (o, c) in out[pad..].iter_mut().zip(s.iter()) {
            *o = c - b'0';
        }
        ctx.add_work(len as u64, Phase::Convert, W_SERIES + W_DIVIDE, 1.0 - W_SERIES - W_DIVIDE);
        return Some(());
    }
    // größte Stufe, deren Länge kleiner als len ist
    let mut i = 0;
    while i + 1 < powers.len() && (LEAF_DIGITS << (i + 1)) < len {
        i += 1;
    }
    let low_len = LEAF_DIGITS << i;
    let (hi, lo) = x.div_mod(&powers[i]);
    let (hi_out, lo_out) = out.split_at_mut(len - low_len);
    if len > 200_000 {
        let (a, b) = rayon::join(
            || to_decimal(hi, hi_out, powers, ctx),
            || to_decimal(lo, lo_out, powers, ctx),
        );
        a?;
        b?;
    } else {
        to_decimal(hi, hi_out, powers, ctx)?;
        to_decimal(lo, lo_out, powers, ctx)?;
    }
    Some(())
}

/// Reihenabschnitt [a, b) komplett im RAM (für den Plattenmodus). `done(n)` meldet fertige Glieder.
pub(crate) fn series_range(
    a: u64,
    b: u64,
    need_p: bool,
    cancel: &AtomicBool,
    done: &(dyn Fn(f64) + Sync),
) -> Option<(Integer, Integer, Integer)> {
    let report = |_: Phase, f: f64| done(f / W_SERIES);
    let ctx = Ctx {
        cancel,
        progress: &report,
        work_done: AtomicU64::new(0),
        work_total: total_work(a, b),
        last_report: AtomicU64::new(0),
    };
    bs(a, b, need_p, &ctx)
}

/// Ganze Zahl → genau `len` Dezimalziffern (führende Nullen), im RAM.
pub(crate) fn natural_to_digits(x: Natural, len: usize, cancel: &AtomicBool) -> Option<Vec<u8>> {
    let noop = |_: Phase, _: f64| {};
    let ctx = Ctx {
        cancel,
        progress: &noop,
        work_done: AtomicU64::new(0),
        work_total: len.max(1) as u64,
        last_report: AtomicU64::new(0),
    };
    let mut out = vec![0u8; len];
    let powers = powers_of_ten(len);
    to_decimal(x, &mut out, &powers, &ctx)?;
    Some(out)
}

/// Bequeme Variante ohne Abbruch und Fortschritt.
pub fn pi_digits(digits: usize) -> Vec<u8> {
    let cancel = AtomicBool::new(false);
    compute_pi_digits(digits, &cancel, &|_, _| {}).unwrap().0
}

pub fn digits_to_string(d: &[u8]) -> String {
    d.iter().map(|&x| (b'0' + x) as char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST_100: &str = "1415926535897932384626433832795028841971693993751058209749445923078164062862089986280348253421170679";

    #[test]
    fn first_100_digits() {
        assert_eq!(digits_to_string(&pi_digits(100)), FIRST_100);
    }

    #[test]
    fn small_lengths_are_prefixes() {
        for n in [1, 2, 5, 13, 14, 15, 29, 64, 99] {
            assert_eq!(digits_to_string(&pi_digits(n)), &FIRST_100[..n], "n={n}");
        }
    }

    #[test]
    fn feynman_point() {
        let d = pi_digits(1000);
        // Position 762 (1-basiert) = Index 761
        assert_eq!(digits_to_string(&d[761..767]), "999999");
        assert_ne!(d[760], 9);
        assert_ne!(d[767], 9);
    }

    #[test]
    fn prefix_independent_of_length() {
        // gleiche Ziffern unabhängig von der Gesamtlänge (prüft Schutzstellen und Umwandlung)
        let a = pi_digits(50_000);
        let b = pi_digits(53_337);
        assert_eq!(a[..], b[..50_000]);
    }

    #[test]
    fn cancel_works() {
        let cancel = AtomicBool::new(true);
        assert!(compute_pi_digits(100_000, &cancel, &|_, _| {}).is_err());
    }
}
