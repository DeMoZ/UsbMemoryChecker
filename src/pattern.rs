//! Test data: every 4 KiB block gets its own pseudo-random content, computed from the block's address and a
//! random salt of the run. A block that a fake drive stored somewhere else (wrap-around), dropped, or returned
//! from an old write never matches, and nothing has to be kept in memory to check it.

pub const BLOCK: usize = 4096;
const WORDS: usize = BLOCK / 8;

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn fill_block(block: &mut [u8], addr: u64, salt: u64) {
    let mut s = splitmix(salt ^ addr.wrapping_mul(0xD6E8_FEB8_6659_FD93)) | 1;
    for w in 0..WORDS {
        // xorshift64*
        s ^= s >> 12;
        s ^= s << 25;
        s ^= s >> 27;
        let v = s.wrapping_mul(0x2545_F491_4F6C_DD1D);
        block[w * 8..w * 8 + 8].copy_from_slice(&v.to_le_bytes());
    }
    // the address in clear text: handy when looking at a dump of a broken drive
    block[..8].copy_from_slice(&addr.to_le_bytes());
}

/// Fills `buf` (a multiple of [`BLOCK`]) as if it were written at address `start`.
pub fn fill(buf: &mut [u8], start: u64, salt: u64) {
    debug_assert!(buf.len().is_multiple_of(BLOCK));
    for (i, block) in buf.as_chunks_mut::<BLOCK>().0.iter_mut().enumerate() {
        fill_block(block, start + (i * BLOCK) as u64, salt);
    }
}

/// How many 4 KiB blocks of `buf` (read from address `start`) differ from what [`fill`] wrote there.
pub fn count_bad(buf: &[u8], start: u64, salt: u64) -> usize {
    let mut expected = [0u8; BLOCK];
    buf.as_chunks::<BLOCK>()
        .0
        .iter()
        .enumerate()
        .filter(|(i, block)| {
            fill_block(&mut expected, start + (*i * BLOCK) as u64, salt);
            **block != expected
        })
        .count()
}

/// Salt for a new run: time and process id are enough, it only has to differ between runs.
pub fn new_salt() -> u64 {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    splitmix(t as u64 ^ ((std::process::id() as u64) << 32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_depend_on_address_and_salt() {
        let mut a = vec![0u8; BLOCK * 4];
        fill(&mut a, 1 << 20, 42);
        assert_eq!(count_bad(&a, 1 << 20, 42), 0);
        assert_eq!(count_bad(&a, (1 << 20) + BLOCK as u64, 42), 4, "shifted by one block");
        assert_eq!(count_bad(&a, 1 << 20, 43), 4, "another run");
        a[BLOCK * 2 + 100] ^= 1;
        assert_eq!(count_bad(&a, 1 << 20, 42), 1);
        // same block written alone or as part of a bigger buffer
        let mut one = vec![0u8; BLOCK];
        fill(&mut one, (1 << 20) + 3 * BLOCK as u64, 42);
        assert_eq!(&one[..], &a[3 * BLOCK..]);
    }
}
