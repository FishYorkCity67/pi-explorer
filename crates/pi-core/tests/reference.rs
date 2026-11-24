//! Abgleich mit einer externen Referenz (MIT, „pi-billion.txt“, Format "3." + 10^9 Stellen).
//! Die Referenz wird NUR hier im Test geladen (per curl, HTTP-Range), nie in der App.
//!
//!   cargo test -p pi-core --release --test reference -- --ignored --nocapture
//!
//! Gecacht wird unter target/pi-reference/.

use pi_core::chudnovsky::{digits_to_string, pi_digits};
use std::path::PathBuf;
use std::process::Command;

const URL: &str = "https://stuff.mit.edu/afs/sipb/contrib/pi/pi-billion.txt";

fn cache_dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/pi-reference");
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Nachkommastellen [pos, pos+len) mit 1-basierter Position.
fn reference(pos: usize, len: usize) -> String {
    let file = cache_dir().join(format!("ref_{pos}_{len}.txt"));
    if let Ok(s) = std::fs::read_to_string(&file) {
        if s.len() == len {
            return s;
        }
    }
    // Byte 0 = '3', Byte 1 = '.', Byte 2 = Position 1
    let from = pos + 1;
    let to = from + len - 1;
    let out = Command::new("curl")
        .args(["-sSf", "--retry", "3", "-r", &format!("{from}-{to}"), URL, "-o"])
        .arg(&file)
        .status()
        .expect("curl nicht gefunden");
    assert!(out.success(), "Download der Referenz fehlgeschlagen");
    let s = std::fs::read_to_string(&file).unwrap();
    assert_eq!(s.len(), len, "Referenz unvollständig");
    assert!(s.bytes().all(|b| b.is_ascii_digit()));
    s
}

#[test]
#[ignore]
fn ten_million_digits_match_reference() {
    let n = 10_000_000;
    let ours = digits_to_string(&pi_digits(n));
    let theirs = reference(1, n);
    if ours != theirs {
        let first = ours.bytes().zip(theirs.bytes()).position(|(a, b)| a != b).unwrap();
        panic!("erste Abweichung an Position {}", first + 1);
    }
    println!("10.000.000 Stellen stimmen mit der Referenz überein");
}

#[test]
#[ignore]
fn hundred_million_windows_match_reference() {
    let n = 100_000_000;
    let d = pi_digits(n);
    // Stichproben quer durch, inkl. Ende
    let windows = [1usize, 12_345_678, 33_333_333, 50_000_000, 77_777_777, 99_990_001];
    for &p in &windows {
        let len = (n - p + 1).min(10_000);
        let ours = digits_to_string(&d[p - 1..p - 1 + len]);
        assert_eq!(ours, reference(p, len), "Fenster ab Position {p}");
    }
    println!("100.000.000 Stellen: alle Stichproben inkl. der letzten 10.000 stimmen");
}
