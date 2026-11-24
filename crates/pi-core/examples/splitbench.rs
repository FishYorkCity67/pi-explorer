//! cargo run --release -p pi-core --example splitbench -- <stellen> <budget_mb>
use pi_core::chudnovsky::digits_to_string;
use pi_core::split::{compute_pi_split, Plan};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

fn main() {
    let mut a = std::env::args().skip(1);
    let n: usize = a.next().unwrap_or("10000000".into()).parse().unwrap();
    let mb: u64 = a.next().unwrap_or("512".into()).parse().unwrap();
    let plan = Plan::for_budget(mb << 20);
    println!("{plan:?}  max_digits={}", plan.max_digits());
    let cancel = AtomicBool::new(false);
    let t = Instant::now();
    let last = std::sync::Mutex::new(String::new());
    let d = compute_pi_split(n, plan, &cancel, &|ph, f| {
        let s = format!("{ph} {:.0}%", f * 100.0);
        let mut l = last.lock().unwrap();
        if *l != s {
            eprintln!("{s}  {:.1}s", t.elapsed().as_secs_f64());
            *l = s;
        }
    })
    .unwrap();
    println!(
        "{n} Stellen mit {mb} MB Budget: {:.1} s, Ende …{}",
        t.elapsed().as_secs_f64(),
        digits_to_string(&d[d.len() - 10..])
    );
    // Stichproben für den Vergleich mit der Referenz
    for p in [1usize, n / 3, n - 20] {
        println!("pos {p}: {}", digits_to_string(&d[p - 1..(p - 1 + 20).min(n)]));
    }
}
