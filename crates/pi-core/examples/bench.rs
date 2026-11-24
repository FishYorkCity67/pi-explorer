//! cargo run --release -p pi-core --example bench -- 1000000 10000000
use pi_core::chudnovsky::{compute_pi_digits, digits_to_string};
use std::sync::atomic::AtomicBool;

fn main() {
    let args: Vec<usize> = std::env::args()
        .skip(1)
        .map(|a| a.replace(['_', '.', ','], "").parse().expect("Zahl erwartet"))
        .collect();
    let sizes = if args.is_empty() { vec![1_000_000, 10_000_000] } else { args };
    println!("Threads: {}", rayon::current_num_threads());
    for n in sizes {
        let cancel = AtomicBool::new(false);
        let (d, t) = compute_pi_digits(n, &cancel, &|_, _| {}).unwrap();
        let tail = digits_to_string(&d[d.len().saturating_sub(10)..]);
        println!(
            "{n:>12} Stellen: {:7.2} s gesamt  (Reihe {:.2} s · Division {:.2} s · Umwandlung {:.2} s)  {:>6.2} Mio. Stellen/s  Ende …{tail}",
            t.total_s,
            t.series_s,
            t.divide_s,
            t.convert_s,
            n as f64 / t.total_s / 1e6
        );
    }
}
