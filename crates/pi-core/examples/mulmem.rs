//! Misst Zeit und Speicher einer Multiplikation zweier k-Limb-Zahlen (für die Blockgröße).
//! cargo run --release -p pi-core --example mulmem -- 1000000
use malachite_nz::natural::Natural;
use std::time::Instant;

fn main() {
    let k: usize = std::env::args().nth(1).unwrap_or("1000000".into()).parse().unwrap();
    let a: Vec<u64> = (0..k as u64).map(|i| i.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1).collect();
    let b: Vec<u64> = (0..k as u64).map(|i| i.wrapping_mul(0xD1B5_4A32_D192_ED03) | 3).collect();
    let a = Natural::from_limbs_asc(&a);
    let b = Natural::from_limbs_asc(&b);
    println!("ready");
    std::thread::sleep(std::time::Duration::from_millis(800));
    let t = Instant::now();
    let c = &a * &b;
    println!("k={k} limbs  {:.2} s  result {} limbs", t.elapsed().as_secs_f64(), c.limb_count());
}
