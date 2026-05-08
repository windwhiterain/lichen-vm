pub mod arena;
pub mod stable_vec;
pub const fn log2_int(x: usize) -> u32 {
    debug_assert!(x > 0);
    (usize::BITS - 1) - x.leading_zeros()
}

pub const fn exp2_int(n: u32) -> usize {
    1 << n
}
pub const fn mod_exp2(l: usize, r: usize) -> usize {
    l & (r - 1)
}
