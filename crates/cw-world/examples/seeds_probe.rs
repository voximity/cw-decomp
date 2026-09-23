//! Prints the sub-seed table for a seed, to compare against the original's memory.
fn main() {
    let seed: i32 = std::env::args().nth(1).map(|s| s.parse().unwrap()).unwrap_or(26879);
    let s = cw_world::seeds::Seeds::from_seed(seed);
    println!("{:?}", s.slots());
}
