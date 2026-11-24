//! Ziffernstatistik – für Ziffern im RAM oder fensterweise aus einer Datei.

use rayon::prelude::*;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub counts: [u64; 10],
    /// Längste Wiederholung: Ziffer, Länge, 1-basierte Startposition
    pub run_digit: u8,
    pub run_len: u64,
    pub run_pos: u64,
}

/// Sammelt Statistik über aufeinanderfolgende Blöcke.
#[derive(Default)]
pub struct StatsAcc {
    s: Stats,
    seen: u64,
    last: Option<u8>,
    cur: u64,
    cur_start: u64,
}

impl StatsAcc {
    pub fn feed(&mut self, d: &[u8]) {
        let counts = d
            .par_chunks(1 << 20)
            .map(|c| {
                let mut k = [0u64; 10];
                for &x in c {
                    k[x as usize] += 1;
                }
                k
            })
            .reduce(
                || [0u64; 10],
                |mut a, b| {
                    for i in 0..10 {
                        a[i] += b[i];
                    }
                    a
                },
            );
        for i in 0..10 {
            self.s.counts[i] += counts[i];
        }
        for (i, &x) in d.iter().enumerate() {
            let pos = self.seen + i as u64;
            if Some(x) == self.last {
                self.cur += 1;
            } else {
                self.cur = 1;
                self.cur_start = pos;
                self.last = Some(x);
            }
            if self.cur > self.s.run_len {
                self.s.run_len = self.cur;
                self.s.run_digit = x;
                self.s.run_pos = self.cur_start + 1;
            }
        }
        self.seen += d.len() as u64;
    }

    pub fn finish(self) -> Stats {
        self.s
    }
}

pub fn digit_stats(d: &[u8]) -> Stats {
    let mut a = StatsAcc::default();
    a.feed(d);
    a.finish()
}

/// Statistik fensterweise aus einer Quelle mit `read(offset, len)`.
pub fn digit_stats_stream(n: u64, read: &dyn Fn(u64, usize) -> std::io::Result<Vec<u8>>) -> std::io::Result<Stats> {
    const WIN: usize = 64 << 20;
    let mut a = StatsAcc::default();
    let mut off = 0u64;
    while off < n {
        let w = (WIN as u64).min(n - off) as usize;
        a.feed(&read(off, w)?);
        off += w as u64;
    }
    Ok(a.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chudnovsky::pi_digits;

    #[test]
    fn feynman_is_longest_in_first_thousand() {
        let s = digit_stats(&pi_digits(1000));
        assert_eq!((s.run_digit, s.run_len, s.run_pos), (9, 6, 762));
        assert_eq!(s.counts.iter().sum::<u64>(), 1000);
    }

    #[test]
    fn known_first_million_counts() {
        // bekannte Häufigkeit der Ziffern in der ersten Million (0 … 9)
        let s = digit_stats(&pi_digits(1_000_000));
        assert_eq!(
            s.counts,
            [99959, 99758, 100026, 100229, 100230, 100359, 99548, 99800, 99985, 100106]
        );
    }

    #[test]
    fn stream_equals_whole() {
        let d = pi_digits(200_000);
        let mut a = StatsAcc::default();
        for c in d.chunks(777) {
            a.feed(c);
        }
        assert_eq!(a.finish(), digit_stats(&d));
    }
}
