//! Schätzung von Rechenzeit und Speicherbedarf.
//!
//! Gemessen (Ryzen 7 7735HS, 16 Threads): 1 Mio. 0,12 s / 35 MB, 10 Mio. 1,55 s / 230 MB,
//! 100 Mio. 22,3 s / 2,67 GB. Die Zeit wächst wie n·log²(n), der Spitzen-RAM ≈ 27 Byte/Stelle.

use crate::chudnovsky::pi_digits;
use std::time::Instant;

pub const CALIB_DIGITS: usize = 300_000;

fn cost(n: usize) -> f64 {
    let n = n.max(1000) as f64;
    let lg = n.log2();
    n * lg * lg
}

/// Sekunden pro Kosteneinheit auf diesem Gerät (kurzer Probelauf, ~20–100 ms).
pub fn calibrate() -> f64 {
    let mut best = f64::MAX;
    for _ in 0..2 {
        let t = Instant::now();
        let d = pi_digits(CALIB_DIGITS);
        std::hint::black_box(&d);
        best = best.min(t.elapsed().as_secs_f64());
    }
    best / cost(CALIB_DIGITS)
}

pub fn estimate_seconds(digits: usize, k: f64) -> f64 {
    k * cost(digits)
}

/// Spitzenbedarf während der Rechnung in Byte (inkl. Ziffernpuffer der App).
pub fn estimate_peak_ram(digits: usize) -> u64 {
    28 * digits as u64 + 48 * 1024 * 1024
}

/// Dauerhafter Bedarf nach der Rechnung (Ziffern in Rust + Textur + Übertragung).
pub fn estimate_resident_ram(digits: usize) -> u64 {
    3 * digits as u64 + 64 * 1024 * 1024
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monotonic() {
        let k = 3e-10;
        assert!(estimate_seconds(10_000_000, k) > estimate_seconds(1_000_000, k) * 10.0);
        assert!(estimate_peak_ram(100_000_000) > 2_000_000_000);
    }
}
