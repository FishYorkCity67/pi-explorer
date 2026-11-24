//! Formensuche im Schwarz-Weiß-Raster (0–4 = schwarz, 5–9 = weiß).
//!
//! Die Ziffern werden zeilenweise mit Breite `width` ausgelegt. Eine gemalte Form (max. 8×8)
//! wird auf ihre Bounding Box zugeschnitten. Gemalt = muss weiß sein, nicht gemalt (in der Box)
//! = muss schwarz sein. Mit `only_painted` zählen nur die gemalten Zellen.

use rayon::prelude::*;

pub const MAX_SHAPE: usize = 8;
const MAX_MATCHES: usize = 20_000;

#[derive(Debug, Clone)]
pub struct ShapeQuery {
    /// Zeilenweise, `size`×`size`
    pub cells: Vec<bool>,
    pub size: usize,
    pub width: usize,
    pub tolerance: u32,
    pub only_painted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShapeMatch {
    /// 0-basierter Ziffernindex der linken oberen Ecke der Bounding Box
    pub index: u64,
    pub mismatches: u32,
}

#[derive(Debug, Clone)]
pub struct ShapeResult {
    pub box_w: usize,
    pub box_h: usize,
    /// Versatz der Bounding Box innerhalb des gemalten Rasters
    pub box_x: usize,
    pub box_y: usize,
    pub checked_cells: u32,
    pub best: Option<ShapeMatch>,
    /// Treffer mit höchstens `tolerance` Abweichungen (gekappt), nach Position sortiert
    pub matches: Vec<ShapeMatch>,
    pub match_count: u64,
    pub positions: u64,
}

impl ShapeResult {
    pub fn percent(&self, m: &ShapeMatch) -> f64 {
        if self.checked_cells == 0 {
            return 0.0;
        }
        100.0 * (self.checked_cells - m.mismatches) as f64 / self.checked_cells as f64
    }
}

#[inline]
fn white(d: u8) -> u8 {
    (d >= 5) as u8
}

pub fn search_shape(digits: &[u8], q: &ShapeQuery) -> Result<ShapeResult, String> {
    search_shape_rows(digits, q, usize::MAX)
}

/// Wie `search_shape`, aber nur Positionen, deren obere Zeile < `max_top_rows` ist.
fn search_shape_rows(digits: &[u8], q: &ShapeQuery, max_top_rows: usize) -> Result<ShapeResult, String> {
    let s = q.size;
    if s == 0 || s > MAX_SHAPE || q.cells.len() != s * s {
        return Err("Invalid shape".into());
    }
    let painted = |x: usize, y: usize| q.cells[y * s + x];
    let (mut x0, mut y0, mut x1, mut y1) = (s, s, 0, 0);
    for y in 0..s {
        for x in 0..s {
            if painted(x, y) {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    if x0 > x1 {
        return Err("Paint at least one cell".into());
    }
    let (bw, bh) = (x1 - x0 + 1, y1 - y0 + 1);
    let w = q.width;
    if w < bw {
        return Err("Row width is smaller than the shape".into());
    }
    let mut want = [0u8; MAX_SHAPE];
    let mut care = [0u8; MAX_SHAPE];
    for r in 0..bh {
        for c in 0..bw {
            let p = painted(x0 + c, y0 + r);
            if p {
                want[r] |= 1 << c;
            }
            if p || !q.only_painted {
                care[r] |= 1 << c;
            }
        }
    }
    let checked: u32 = care[..bh].iter().map(|c| c.count_ones()).sum();
    let rows = digits.len() / w;
    let mut result = ShapeResult {
        box_w: bw,
        box_h: bh,
        box_x: x0,
        box_y: y0,
        checked_cells: checked,
        best: None,
        matches: Vec::new(),
        match_count: 0,
        positions: 0,
    };
    if rows < bh {
        return Ok(result);
    }
    let ys = (rows - bh + 1).min(max_top_rows);
    let xs = w - bw + 1;
    result.positions = ys as u64 * xs as u64;
    let tol = q.tolerance;
    let block = 32usize;

    struct Part {
        best: Option<ShapeMatch>,
        /// nach Abweichungen getrennt (Index = Abweichungen), jeweils in Positionsreihenfolge
        by_mm: Vec<Vec<ShapeMatch>>,
        count: u64,
    }

    let n_blocks = ys.div_ceil(block);
    let levels = (tol as usize).min(MAX_SHAPE * MAX_SHAPE) + 1;
    let per_cap = (MAX_MATCHES * 2 / n_blocks.max(1)).max(64);

    let parts: Vec<Part> = (0..n_blocks)
        .into_par_iter()
        .map(|b| {
            let ya = b * block;
            let yb = (ya + block).min(ys);
            // Fensterwerte für alle benötigten Zeilen dieses Blocks
            let nrows = yb - ya + bh - 1;
            let mut win = vec![0u8; nrows * xs];
            for r in 0..nrows {
                let row = &digits[(ya + r) * w..(ya + r + 1) * w];
                let out = &mut win[r * xs..(r + 1) * xs];
                let mut v = 0u8;
                for c in 0..bw {
                    v |= white(row[c]) << c;
                }
                out[0] = v;
                for x in 1..xs {
                    v = (v >> 1) | (white(row[x + bw - 1]) << (bw - 1));
                    out[x] = v;
                }
            }
            let mut part = Part { best: None, by_mm: vec![Vec::new(); levels], count: 0 };
            let mut best_mm = u32::MAX;
            for y in ya..yb {
                let base = y - ya;
                for x in 0..xs {
                    let mut mm = 0u32;
                    for r in 0..bh {
                        mm += ((win[(base + r) * xs + x] ^ want[r]) & care[r]).count_ones();
                    }
                    if mm < best_mm {
                        best_mm = mm;
                        part.best = Some(ShapeMatch { index: (y * w + x) as u64, mismatches: mm });
                    }
                    if mm <= tol {
                        part.count += 1;
                        let lvl = &mut part.by_mm[mm as usize];
                        if lvl.len() < per_cap {
                            lvl.push(ShapeMatch { index: (y * w + x) as u64, mismatches: mm });
                        }
                    }
                }
            }
            part
        })
        .collect();

    for p in &parts {
        if let Some(b) = p.best {
            if result.best.is_none_or(|cur| b.mismatches < cur.mismatches) {
                result.best = Some(b);
            }
        }
        result.match_count += p.count;
    }
    // Beste zuerst: nach Abweichungen, dann nach Position
    'outer: for lvl in 0..levels {
        for p in &parts {
            for m in &p.by_mm[lvl] {
                if result.matches.len() >= MAX_MATCHES {
                    break 'outer;
                }
                result.matches.push(*m);
            }
        }
    }
    Ok(result)
}

/// Formensuche fensterweise (Ziffern aus einer Datei): `read(offset, len)`.
pub fn search_shape_stream(
    n: u64,
    read: &dyn Fn(u64, usize) -> std::io::Result<Vec<u8>>,
    q: &ShapeQuery,
) -> Result<ShapeResult, String> {
    search_shape_stream_win(n, read, q, 64 << 20)
}

fn search_shape_stream_win(
    n: u64,
    read: &dyn Fn(u64, usize) -> std::io::Result<Vec<u8>>,
    q: &ShapeQuery,
    win: u64,
) -> Result<ShapeResult, String> {
    let w = q.width as u64;
    if w == 0 {
        return Err("Row width must be at least 1".into());
    }
    let rows = n / w;
    // Zeilen pro Fenster (~64 MB); dazu Überlappung für die Formhöhe
    let per = (win / w).max(1);
    let mut total: Option<ShapeResult> = None;
    let mut y0 = 0u64;
    while y0 < rows || total.is_none() {
        let ye = (y0 + per + MAX_SHAPE as u64 - 1).min(rows);
        let data = read(y0 * w, (ye.saturating_sub(y0) * w) as usize).map_err(|e| e.to_string())?;
        let mut part = search_shape_rows(&data, q, per as usize)?;
        let off = y0 * w;
        for m in part.matches.iter_mut() {
            m.index += off;
        }
        if let Some(b) = part.best.as_mut() {
            b.index += off;
        }
        total = Some(match total {
            None => part,
            Some(mut t) => {
                t.positions += part.positions;
                t.match_count += part.match_count;
                if let Some(b) = part.best {
                    if t.best.is_none_or(|c| b.mismatches < c.mismatches) {
                        t.best = Some(b);
                    }
                }
                t.matches.extend(part.matches);
                t.matches.sort_unstable_by_key(|m| (m.mismatches, m.index));
                t.matches.truncate(MAX_MATCHES);
                t
            }
        });
        y0 += per;
        if y0 >= rows {
            break;
        }
    }
    total.ok_or_else(|| "Not enough digits for this shape".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chudnovsky::pi_digits;

    fn brute(digits: &[u8], q: &ShapeQuery) -> (u32, u64) {
        let r = search_shape(digits, q).unwrap();
        let (bw, bh, x0, y0) = (r.box_w, r.box_h, r.box_x, r.box_y);
        let rows = digits.len() / q.width;
        let mut best = u32::MAX;
        let mut count = 0;
        for y in 0..=rows - bh {
            for x in 0..=q.width - bw {
                let mut mm = 0;
                for r in 0..bh {
                    for c in 0..bw {
                        let p = q.cells[(y0 + r) * q.size + x0 + c];
                        if !p && q.only_painted {
                            continue;
                        }
                        let wht = digits[(y + r) * q.width + x + c] >= 5;
                        if wht != p {
                            mm += 1;
                        }
                    }
                }
                best = best.min(mm);
                if mm <= q.tolerance {
                    count += 1;
                }
            }
        }
        (best, count)
    }

    #[test]
    fn matches_brute_force() {
        let d = pi_digits(30_000);
        let mut cells = vec![false; 36];
        for i in [7, 8, 9, 13, 16, 19, 20, 21, 27] {
            cells[i] = true;
        }
        for (width, tol, only) in [(100, 2, false), (173, 0, true), (61, 3, false), (250, 1, true)] {
            let q = ShapeQuery { cells: cells.clone(), size: 6, width, tolerance: tol, only_painted: only };
            let r = search_shape(&d, &q).unwrap();
            let (best, count) = brute(&d, &q);
            assert_eq!(r.best.unwrap().mismatches, best, "w={width}");
            assert_eq!(r.match_count, count, "w={width}");
            // jede gemeldete Fundstelle wirklich prüfen, beste zuerst
            for m in &r.matches {
                assert!(m.mismatches <= tol);
            }
            assert!(r.matches.windows(2).all(|p| (p[0].mismatches, p[0].index) < (p[1].mismatches, p[1].index)));
            if let Some(first) = r.matches.first() {
                assert_eq!(first.mismatches, best);
            }
        }
    }

    #[test]
    fn finds_planted_shape() {
        let w = 50;
        let mut d = vec![0u8; w * 40];
        // Kreuz bei x=20, y=10
        let pts = [(1, 0), (0, 1), (1, 1), (2, 1), (1, 2)];
        for (x, y) in pts {
            d[(10 + y) * w + 20 + x] = 9;
        }
        let mut cells = vec![false; 36];
        for (x, y) in pts {
            cells[(y + 2) * 6 + x + 3] = true;
        }
        let q = ShapeQuery { cells, size: 6, width: w, tolerance: 0, only_painted: false };
        let r = search_shape(&d, &q).unwrap();
        assert_eq!(r.match_count, 1);
        assert_eq!(r.best.unwrap().index as usize, 10 * w + 20);
        assert_eq!((r.box_w, r.box_h, r.box_x, r.box_y), (3, 3, 3, 2));
        assert_eq!(r.percent(&r.best.unwrap()), 100.0);
    }

    #[test]
    fn stream_equals_whole() {
        let d = pi_digits(60_000);
        let mut cells = vec![false; 36];
        for i in [7, 8, 13, 14, 20, 27] {
            cells[i] = true;
        }
        let q = ShapeQuery { cells, size: 6, width: 97, tolerance: 1, only_painted: false };
        let whole = search_shape(&d, &q).unwrap();
        let read = |o: u64, l: usize| -> std::io::Result<Vec<u8>> {
            let a = (o as usize).min(d.len());
            Ok(d[a..(a + l).min(d.len())].to_vec())
        };
        let s = search_shape_stream_win(d.len() as u64, &read, &q, 97 * 13).unwrap();
        assert_eq!(s.match_count, whole.match_count);
        assert_eq!(s.positions, whole.positions);
        assert_eq!(s.best.unwrap().mismatches, whole.best.unwrap().mismatches);
        assert_eq!(s.matches, whole.matches);
    }

    #[test]
    fn empty_shape_is_error() {
        let q = ShapeQuery { cells: vec![false; 36], size: 6, width: 10, tolerance: 0, only_painted: false };
        assert!(search_shape(&[0; 100], &q).is_err());
    }
}
