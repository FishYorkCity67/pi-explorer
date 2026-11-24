//! cargo run --release -p pi-core --example oocbench -- <stellen> <budget_mb> <arbeitsordner>
use pi_core::ooc::{compute_pi_ooc, read_digit_file, OocPlan};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

fn main() {
    let mut a = std::env::args().skip(1);
    let n: usize = a.next().unwrap_or("10000000".into()).parse().unwrap();
    let mb: u64 = a.next().unwrap_or("256".into()).parse().unwrap();
    let dir = PathBuf::from(a.next().unwrap_or_else(|| std::env::temp_dir().join("pi-ooc").to_string_lossy().into()));
    let plan = OocPlan::for_budget(mb << 20);
    println!("{plan:?}");
    let work = dir.join("work");
    let out = dir.join("digits.bin");
    let cancel = AtomicBool::new(false);
    let t = Instant::now();
    let last = std::sync::Mutex::new(String::new());
    let r = compute_pi_ooc(n, plan, &work, &out, &cancel, &|ph, f| {
        let s = format!("{ph} {:.0}%", f * 100.0);
        let mut l = last.lock().unwrap();
        if *l != s {
            eprintln!("{s}  {:.1}s", t.elapsed().as_secs_f64());
            *l = s;
        }
    })
    .unwrap();
    println!(
        "{n} Stellen, {mb} MB RAM-Budget: {:.1} s, Platte in der Spitze {} MB",
        t.elapsed().as_secs_f64(),
        r.disk_peak >> 20
    );
    for p in [1u64, n as u64 / 3, n as u64 - 19] {
        let d = read_digit_file(&out, p - 1, 20).unwrap();
        println!("pos {p}: {}", d.iter().map(|x| (b'0' + x) as char).collect::<String>());
    }
    let _ = std::fs::remove_file(&out);
}
