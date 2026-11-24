//! Ziffernfolgen suchen. Positionen sind 1-basiert ab der ersten Nachkommastelle.

use memchr::memmem;
use rayon::prelude::*;

const CHUNK: usize = 4 << 20;

/// Höchstens so viele Fundstellen werden gespeichert (gezählt werden alle).
pub const MAX_STORED_HITS: usize = 2_000_000;

/// Treffer (überlappend) in einem Block; `collect` = Positionen sammeln, sonst nur zählen.
fn scan_chunk(digits: &[u8], finder: &memmem::Finder, plen: usize, c: usize, out: Option<&mut Vec<u64>>) -> u64 {
    let start = c * CHUNK;
    // Überlappung, damit Treffer über Blockgrenzen gefunden werden
    let end = (start + CHUNK + plen - 1).min(digits.len());
    let hay = &digits[start..end];
    // find_iter liefert nur nicht-überlappende Treffer, wir wollen alle
    let mut n = 0;
    let mut i = 0;
    let mut out = out;
    while let Some(p) = finder.find(&hay[i..]) {
        if let Some(o) = out.as_deref_mut() {
            o.push((start + i + p + 1) as u64);
        }
        n += 1;
        i += p + 1;
        if i + plen > hay.len() {
            break;
        }
    }
    n
}

/// Fundstellen (1-basiert, aufsteigend) von `pat` (Werte 0..=9): die ersten `cap` Positionen
/// und die Gesamtzahl.
pub fn find_capped(digits: &[u8], pat: &[u8], cap: usize) -> (Vec<u64>, u64) {
    if pat.is_empty() || pat.len() > digits.len() {
        return (Vec::new(), 0);
    }
    let finder = memmem::Finder::new(pat);
    let n_chunks = digits.len().div_ceil(CHUNK);
    // 1. parallel zählen
    let counts: Vec<u64> = (0..n_chunks)
        .into_par_iter()
        .map(|c| scan_chunk(digits, &finder, pat.len(), c, None))
        .collect();
    let total = counts.iter().sum();
    // 2. Positionen nur aus den ersten Blöcken holen, bis `cap` erreicht ist
    let mut need = 0usize;
    let mut upto = 0;
    while upto < n_chunks && need < cap {
        need += counts[upto] as usize;
        upto += 1;
    }
    let parts: Vec<Vec<u64>> = (0..upto)
        .into_par_iter()
        .map(|c| {
            let mut v = Vec::with_capacity(counts[c] as usize);
            scan_chunk(digits, &finder, pat.len(), c, Some(&mut v));
            v
        })
        .collect();
    let mut all = parts.concat();
    all.truncate(cap);
    (all, total)
}

/// Alle Fundstellen (für Tests und kleine Mengen).
pub fn find_all(digits: &[u8], pat: &[u8]) -> Vec<u64> {
    find_capped(digits, pat, usize::MAX).0
}

/// Mehrere Muster: nach Position sortierte Treffer (gekappt) mit Musterindex, plus Anzahl je Muster.
pub fn find_multi(digits: &[u8], pats: &[Vec<u8>], cap: usize) -> (Vec<(u64, u8)>, Vec<u64>) {
    let mut all: Vec<(u64, u8)> = Vec::new();
    let mut counts = Vec::new();
    for (i, p) in pats.iter().enumerate() {
        let (pos, n) = find_capped(digits, p, cap);
        counts.push(n);
        all.extend(pos.into_iter().map(|pos| (pos, i as u8)));
    }
    all.sort_unstable();
    all.truncate(cap);
    (all, counts)
}

/// Fensterweise Suche für Ziffern, die nicht im RAM liegen (Datei).
/// `read(offset, len)` liefert Ziffern ab 0-basiertem `offset`.
pub fn find_multi_stream(
    n: u64,
    read: &dyn Fn(u64, usize) -> std::io::Result<Vec<u8>>,
    pats: &[Vec<u8>],
    cap: usize,
) -> std::io::Result<(Vec<(u64, u8)>, Vec<u64>)> {
    find_multi_stream_win(n, read, pats, cap, 64 << 20)
}

fn find_multi_stream_win(
    n: u64,
    read: &dyn Fn(u64, usize) -> std::io::Result<Vec<u8>>,
    pats: &[Vec<u8>],
    cap: usize,
    win_size: usize,
) -> std::io::Result<(Vec<(u64, u8)>, Vec<u64>)> {
    let maxlen = pats.iter().map(|p| p.len()).max().unwrap_or(1).max(1);
    let mut counts = vec![0u64; pats.len()];
    let mut stored = vec![0usize; pats.len()];
    let mut all: Vec<(u64, u8)> = Vec::new();
    let mut off = 0u64;
    while off < n {
        let win = (win_size as u64).min(n - off) as usize;
        let data = read(off, win + maxlen - 1)?;
        for (i, p) in pats.iter().enumerate() {
            if p.is_empty() {
                continue;
            }
            // Treffer, die in diesem Fenster beginnen; jedes Muster wird für sich gekappt
            let hay = &data[..data.len().min(win + p.len() - 1)];
            let room = cap.saturating_sub(stored[i]);
            let (pos, cnt) = find_capped(hay, p, room);
            counts[i] += cnt;
            stored[i] += pos.len();
            all.extend(pos.into_iter().map(|x| (x + off, i as u8)));
        }
        off += win as u64;
    }
    all.sort_unstable();
    all.truncate(cap);
    Ok((all, counts))
}

/// "0511" → [0,5,1,1]; Leerzeichen/Punkte/Striche werden ignoriert.
pub fn parse_digits(s: &str) -> Result<Vec<u8>, char> {
    s.chars()
        .filter(|c| !matches!(c, ' ' | '.' | ',' | '-' | '_' | '\''))
        .map(|c| c.to_digit(10).map(|d| d as u8).ok_or(c))
        .collect()
}

/// Wort → Ziffern mit A=01 … Z=26. Umlaute: Ä→AE, Ö→OE, Ü→UE, ß→SS.
/// Gibt (normalisierte Buchstaben, Ziffern) zurück; unbekannte Zeichen sind ein Fehler.
pub fn word_to_digits(word: &str) -> Result<(String, Vec<u8>), char> {
    let mut letters = String::new();
    for c in word.chars() {
        match c {
            ' ' | '-' | '\'' | '.' => continue,
            'ä' | 'Ä' => letters.push_str("AE"),
            'ö' | 'Ö' => letters.push_str("OE"),
            'ü' | 'Ü' => letters.push_str("UE"),
            'ß' | 'ẞ' => letters.push_str("SS"),
            c if c.is_ascii_alphabetic() => letters.push(c.to_ascii_uppercase()),
            other => return Err(other),
        }
    }
    let mut d = Vec::with_capacity(letters.len() * 2);
    for b in letters.bytes() {
        let v = b - b'A' + 1;
        d.push(v / 10);
        d.push(v % 10);
    }
    Ok((letters, d))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatePattern {
    pub label: &'static str,
    pub digits: Vec<u8>,
}

/// "05.11.1995" → TTMMJJJJ, TTMMJJ, JJJJMMTT. Erlaubt auch "/", "-" oder Leerzeichen als Trenner.
pub fn birthday_patterns(s: &str) -> Result<Vec<DatePattern>, String> {
    let parts: Vec<&str> = s
        .trim()
        .split(['.', '/', '-', ' '])
        .filter(|p| !p.is_empty())
        .collect();
    let (d, m, y) = match parts.as_slice() {
        [d, m, y] => (*d, *m, *y),
        [one] if one.len() == 8 && one.bytes().all(|b| b.is_ascii_digit()) => {
            (&one[0..2], &one[2..4], &one[4..8])
        }
        _ => return Err("Use the format DD.MM.YYYY".into()),
    };
    let num = |x: &str| x.parse::<u32>().map_err(|_| "Use the format DD.MM.YYYY".to_string());
    let (dd, mm, yy) = (num(d)?, num(m)?, num(y)?);
    if y.len() != 4 {
        return Err("Please use a four-digit year".into());
    }
    let leap = (yy % 4 == 0 && yy % 100 != 0) || yy % 400 == 0;
    let mdays = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if !(1..=12).contains(&mm) || dd == 0 || dd > mdays[(mm - 1) as usize] {
        return Err("That date does not exist".into());
    }
    let to_d = |s: String| s.bytes().map(|b| b - b'0').collect::<Vec<u8>>();
    Ok(vec![
        DatePattern { label: "DDMMYYYY", digits: to_d(format!("{dd:02}{mm:02}{yy:04}")) },
        DatePattern { label: "DDMMYY", digits: to_d(format!("{dd:02}{mm:02}{:02}", yy % 100)) },
        DatePattern { label: "YYYYMMDD", digits: to_d(format!("{yy:04}{mm:02}{dd:02}")) },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chudnovsky::pi_digits;

    #[test]
    fn finds_known_positions() {
        let d = pi_digits(10_000);
        assert_eq!(find_all(&d, &[1, 4, 1, 5])[0], 1);
        // Feynman-Punkt
        assert_eq!(find_all(&d, &[9, 9, 9, 9, 9, 9]), vec![762]);
        // "999999" überlappend: 5 Treffer für "99" im Feynman-Punkt
        let nn = find_all(&d, &[9, 9]);
        assert!(nn.windows(5).any(|w| w == [762, 763, 764, 765, 766]));
    }

    #[test]
    fn capped_counts_everything() {
        let d = pi_digits(100_000);
        let (pos, n) = find_capped(&d, &[7], 50);
        assert_eq!(pos.len(), 50);
        assert_eq!(n as usize, d.iter().filter(|&&x| x == 7).count());
        assert_eq!(pos, find_all(&d, &[7])[..50].to_vec());
    }

    #[test]
    fn stream_equals_whole() {
        let d = pi_digits(100_000);
        let pats = vec![vec![9u8, 9], vec![1, 4, 1], vec![2]];
        let read = |o: u64, l: usize| -> std::io::Result<Vec<u8>> {
            let a = (o as usize).min(d.len());
            Ok(d[a..(a + l).min(d.len())].to_vec())
        };
        for cap in [usize::MAX, 100] {
            let s = find_multi_stream_win(d.len() as u64, &read, &pats, cap, 1234).unwrap();
            assert_eq!(s, find_multi(&d, &pats, cap));
        }
    }

    #[test]
    fn chunk_boundaries() {
        let mut d = vec![0u8; CHUNK * 2 + 10];
        d[CHUNK - 2..CHUNK + 2].copy_from_slice(&[1, 2, 3, 4]);
        assert_eq!(find_all(&d, &[1, 2, 3, 4]), vec![(CHUNK - 1) as u64]);
        // Paare, die eine der vier Nicht-Null-Zellen berühren: 5
        assert_eq!(find_all(&d, &[0, 0]).len(), d.len() - 1 - 5);
    }

    #[test]
    fn brute_force_agrees() {
        let d = pi_digits(20_000);
        for pat in [&[2u8, 6][..], &[0, 0, 0], &[3, 1, 4], &[5]] {
            let brute: Vec<u64> = (0..=d.len() - pat.len())
                .filter(|&i| &d[i..i + pat.len()] == pat)
                .map(|i| i as u64 + 1)
                .collect();
            assert_eq!(find_all(&d, pat), brute);
        }
    }

    #[test]
    fn words() {
        let (l, d) = word_to_digits("Pi").unwrap();
        assert_eq!(l, "PI");
        assert_eq!(d, vec![1, 6, 0, 9]);
        let (l, _) = word_to_digits("Größe").unwrap();
        assert_eq!(l, "GROESSE");
        assert_eq!(word_to_digits("a1"), Err('1'));
        assert_eq!(word_to_digits("Z").unwrap().1, vec![2, 6]);
    }

    #[test]
    fn birthdays() {
        let p = birthday_patterns("05.11.1995").unwrap();
        assert_eq!(p[0].digits, vec![0, 5, 1, 1, 1, 9, 9, 5]);
        assert_eq!(p[1].digits, vec![0, 5, 1, 1, 9, 5]);
        assert_eq!(p[2].digits, vec![1, 9, 9, 5, 1, 1, 0, 5]);
        assert!(birthday_patterns("31.02.2000").is_err());
        assert!(birthday_patterns("29.02.2000").is_ok());
        assert!(birthday_patterns("29.02.1900").is_err());
        assert!(birthday_patterns("5/11/1995").is_ok());
        assert!(birthday_patterns("05111995").is_ok());
    }
}
