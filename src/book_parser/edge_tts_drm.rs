// Adapted from msedge-tts 0.4.1, src/tts/drm.rs (MIT).
// Upstream: hs-CN/msedge-tts@79d6d83c8e1ac6507b37f775055e7a4abbe78a82.
// See third-party/msedge-tts-LICENSE.txt for the copyright and permission notice.

// Sec-MS-GEC token generation.
//
// Since October 2026 Microsoft requires the v2 `Sec-MS-GEC` token format, a
// `<16 hex>-<64 hex>` string combining a CS64 digest and a salted SHA-256
// digest. The legacy v1 token (a plain 64 hex SHA-256) is rejected with HTTP
// 403 during the WebSocket handshake, which makes the Read aloud endpoint fail
// worldwide.
//
// This is a faithful, self-contained port of `generate_sec_ms_gec_v2` and its
// helpers from edge-tts `drm.py`.
//
// Solution source:
//   https://github.com/rany2/edge-tts/issues/490
//   https://github.com/rany2/edge-tts/pull/492 (merged; released as edge-tts 7.3.0)
//
// Every operation is a wrapping 32-bit operation, mirroring the original
// Python integer arithmetic masked with `& 0xFFFFFFFF`.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Seconds between the Windows file-time epoch (1601-01-01) and the Unix epoch.
const WIN_EPOCH: u64 = 11_644_473_600;

/// Client token embedded in the hashed string.
const TRUSTED_CLIENT_TOKEN: &str = "6A5AA1D4EAFF4E9FB37E23D68491D6F4";

// FIPS 180-4 SHA-256 constants:
// https://nvlpubs.nist.gov/nistpubs/FIPS/NIST.FIPS.180-4.pdf
const SHA256_INITIAL: [u32; 8] = [
    0x6A09E667, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A, 0x510E527F, 0x9B05688C, 0x1F83D9AB, 0x5BE0CD19,
];

const SHA256_STATE: [u32; 8] = [
    0xFD9FBEA2, 0x91243CE7, 0xBE499094, 0xCD2E1E0A, 0x6C006F49, 0x04BD35AB, 0xFEB0A7E4, 0x03EFFBD2,
];

const SHA256_SCHEDULE: [u32; 14] = [
    0xEB24BF9E, 0x1558691F, 0xA0C09EE2, 0x0CFA32BA, 0xA622EE8C, 0xD1DD9491, 0xC0DD25A0, 0xF9E70AB6,
    0xF0C31573, 0x7058CE4E, 0x7188E606, 0x00E5AB33, 0xD258B060, 0x72766564,
];

const SHA256_ROUND_CONSTANTS: [u32; 64] = [
    0x428A2F98, 0x71374491, 0xB5C0FBCF, 0xE9B5DBA5, 0x3956C25B, 0x59F111F1, 0x923F82A4, 0xAB1C5ED5,
    0xD807AA98, 0x12835B01, 0x243185BE, 0x550C7DC3, 0x72BE5D74, 0x80DEB1FE, 0x9BDC06A7, 0xC19BF174,
    0xE49B69C1, 0xEFBE4786, 0x0FC19DC6, 0x240CA1CC, 0x2DE92C6F, 0x4A7484AA, 0x5CB0A9DC, 0x76F988DA,
    0x983E5152, 0xA831C66D, 0xB00327C8, 0xBF597FC7, 0xC6E00BF3, 0xD5A79147, 0x06CA6351, 0x14292967,
    0x27B70A85, 0x2E1B2138, 0x4D2C6DFC, 0x53380D13, 0x650A7354, 0x766A0ABB, 0x81C2C92E, 0x92722C85,
    0xA2BFE8A1, 0xA81A664B, 0xC24B8B70, 0xC76C51A3, 0xD192E819, 0xD6990624, 0xF40E3585, 0x106AA070,
    0x19A4C116, 0x1E376C08, 0x2748774C, 0x34B0BCB5, 0x391C0CB3, 0x4ED8AA4A, 0x5B9CCA4F, 0x682E6FF3,
    0x748F82EE, 0x78A5636F, 0x84C87814, 0x8CC70208, 0x90BEFFFA, 0xA4506CEB, 0xBEF9A3F7, 0xC67178F2,
];

const SHA256_ADJUSTMENT: u32 = 0xC9DC06A7;
const SHA256_SIGMA0_ADJUSTMENT: u32 = 0x059C0B80;
const SHA256_SIGMA1_ADJUSTMENT: u32 = 0x000B92C0;

/// `_SHA256_ROUND_CONSTANTS[..14] + (SHA256_ADJUSTMENT,) + [15..]`.
const SHA256_BLOCK_CONSTANTS: [u32; 64] = [
    0x428A2F98, 0x71374491, 0xB5C0FBCF, 0xE9B5DBA5, 0x3956C25B, 0x59F111F1, 0x923F82A4, 0xAB1C5ED5,
    0xD807AA98, 0x12835B01, 0x243185BE, 0x550C7DC3, 0x72BE5D74, 0x80DEB1FE, 0xC9DC06A7, 0xC19BF174,
    0xE49B69C1, 0xEFBE4786, 0x0FC19DC6, 0x240CA1CC, 0x2DE92C6F, 0x4A7484AA, 0x5CB0A9DC, 0x76F988DA,
    0x983E5152, 0xA831C66D, 0xB00327C8, 0xBF597FC7, 0xC6E00BF3, 0xD5A79147, 0x06CA6351, 0x14292967,
    0x27B70A85, 0x2E1B2138, 0x4D2C6DFC, 0x53380D13, 0x650A7354, 0x766A0ABB, 0x81C2C92E, 0x92722C85,
    0xA2BFE8A1, 0xA81A664B, 0xC24B8B70, 0xC76C51A3, 0xD192E819, 0xD6990624, 0xF40E3585, 0x106AA070,
    0x19A4C116, 0x1E376C08, 0x2748774C, 0x34B0BCB5, 0x391C0CB3, 0x4ED8AA4A, 0x5B9CCA4F, 0x682E6FF3,
    0x748F82EE, 0x78A5636F, 0x84C87814, 0x8CC70208, 0x90BEFFFA, 0xA4506CEB, 0xBEF9A3F7, 0xC67178F2,
];

const CS64_COEFFICIENTS: [(u32, u32); 14] = [
    (0xE82E91B1, 0xB32700D0),
    (0x44DE24C6, 0x88DFF51D),
    (0x3BEB65BC, 0x29514038),
    (0xC89C5933, 0xB328CBDF),
    (0x8D1EC211, 0xD405B4D5),
    (0x314EF5D5, 0x26E55F3C),
    (0xE92491B1, 0x607801C6),
    (0xF0A5B9D5, 0x82B2C1FB),
    (0x06064CF3, 0xFA51A3DA),
    (0xDA79FA60, 0x207CC464),
    (0xBED20560, 0x3C3C74F3),
    (0x89F9C9F8, 0xD2D14B61),
    (0x35FA38C6, 0xBB04A2C1),
    (0xCB49E3C6, 0xC5626167),
];

const CS64_FACTOR: u32 = 0x82893933;
const CS64_ADJUSTMENT0: u32 = 0xF2F90B1A;
const CS64_ADJUSTMENT1: u32 = 0x0B1A0000;

/// Rotate a 32-bit value left by 16 bits.
#[inline]
fn rol16(value: u32) -> u32 {
    value.rotate_left(16)
}

/// SHA-256 small sigma zero, truncating the combined rotations once.
#[inline]
fn sigma0(value: u32) -> u32 {
    (value >> 7) ^ (value << 25) ^ (value >> 18) ^ (value << 14) ^ (value >> 3)
}

/// SHA-256 small sigma one, truncating the combined rotations once.
#[inline]
fn sigma1(value: u32) -> u32 {
    (value >> 17) ^ (value << 15) ^ (value >> 19) ^ (value << 13) ^ (value >> 10)
}

/// `Ch(e, f, g) = g ^ (e & (f ^ g))`.
#[inline]
fn ch(x: u32, y: u32, z: u32) -> u32 {
    z ^ (x & (y ^ z))
}

/// `Maj(a, b, c) = (a & b) | (c & (a | b))`.
#[inline]
fn maj(x: u32, y: u32, z: u32) -> u32 {
    (x & y) | (z & (x | y))
}

/// `ROTR(x, 6) ^ ROTR(x, 11) ^ ROTR(x, 25)`.
#[inline]
fn big_sigma_e(x: u32) -> u32 {
    x.rotate_right(6) ^ x.rotate_right(11) ^ x.rotate_right(25)
}

/// `ROTR(x, 2) ^ ROTR(x, 13) ^ ROTR(x, 22)`.
#[inline]
fn big_sigma_a(x: u32) -> u32 {
    x.rotate_right(2) ^ x.rotate_right(13) ^ x.rotate_right(22)
}

/// Split data into little-endian 32-bit words, zero-padded to an even count.
fn words_le4_even(data: &[u8]) -> Vec<u32> {
    let mut padded = data.to_vec();
    padded.resize(padded.len().div_ceil(4) * 4, 0);
    let mut words: Vec<u32> = padded
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect();
    if !words.len().is_multiple_of(2) {
        words.push(0);
    }
    words
}

/// Run SHA-256 rounds, optionally resuming inside the first block.
fn sha256_rounds(state: [u32; 8], words: &[u32], first_round: usize) -> [u32; 8] {
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
    let constants: &[u32; 64] = if first_round == 14 {
        &SHA256_BLOCK_CONSTANTS
    } else {
        &SHA256_ROUND_CONSTANTS
    };
    let aligned = std::cmp::min(64, first_round.div_ceil(8) * 8);

    let mut index = first_round;
    while index < aligned {
        let t1 = h
            .wrapping_add(big_sigma_e(e))
            .wrapping_add(ch(e, f, g))
            .wrapping_add(constants[index])
            .wrapping_add(words[index]);
        let t2 = big_sigma_a(a).wrapping_add(maj(a, b, c));
        (a, b, c, d, e, f, g, h) = (t1.wrapping_add(t2), a, b, c, d.wrapping_add(t1), e, f, g);
        index += 1;
    }

    // Eight rounds return the working-register names to their original order.
    while index < 64 {
        let mut t1 = h
            .wrapping_add(big_sigma_e(e))
            .wrapping_add(ch(e, f, g))
            .wrapping_add(constants[index])
            .wrapping_add(words[index]);
        d = d.wrapping_add(t1);
        h = t1.wrapping_add(big_sigma_a(a)).wrapping_add(maj(a, b, c));

        t1 = g
            .wrapping_add(big_sigma_e(d))
            .wrapping_add(ch(d, e, f))
            .wrapping_add(constants[index + 1])
            .wrapping_add(words[index + 1]);
        c = c.wrapping_add(t1);
        g = t1.wrapping_add(big_sigma_a(h)).wrapping_add(maj(h, a, b));

        t1 = f
            .wrapping_add(big_sigma_e(c))
            .wrapping_add(ch(c, d, e))
            .wrapping_add(constants[index + 2])
            .wrapping_add(words[index + 2]);
        b = b.wrapping_add(t1);
        f = t1.wrapping_add(big_sigma_a(g)).wrapping_add(maj(g, h, a));

        t1 = e
            .wrapping_add(big_sigma_e(b))
            .wrapping_add(ch(b, c, d))
            .wrapping_add(constants[index + 3])
            .wrapping_add(words[index + 3]);
        a = a.wrapping_add(t1);
        e = t1.wrapping_add(big_sigma_a(f)).wrapping_add(maj(f, g, h));

        t1 = d
            .wrapping_add(big_sigma_e(a))
            .wrapping_add(ch(a, b, c))
            .wrapping_add(constants[index + 4])
            .wrapping_add(words[index + 4]);
        h = h.wrapping_add(t1);
        d = t1.wrapping_add(big_sigma_a(e)).wrapping_add(maj(e, f, g));

        t1 = c
            .wrapping_add(big_sigma_e(h))
            .wrapping_add(ch(h, a, b))
            .wrapping_add(constants[index + 5])
            .wrapping_add(words[index + 5]);
        g = g.wrapping_add(t1);
        c = t1.wrapping_add(big_sigma_a(d)).wrapping_add(maj(d, e, f));

        t1 = b
            .wrapping_add(big_sigma_e(g))
            .wrapping_add(ch(g, h, a))
            .wrapping_add(constants[index + 6])
            .wrapping_add(words[index + 6]);
        f = f.wrapping_add(t1);
        b = t1.wrapping_add(big_sigma_a(c)).wrapping_add(maj(c, d, e));

        t1 = a
            .wrapping_add(big_sigma_e(f))
            .wrapping_add(ch(f, g, h))
            .wrapping_add(constants[index + 7])
            .wrapping_add(words[index + 7]);
        e = e.wrapping_add(t1);
        a = t1.wrapping_add(big_sigma_a(b)).wrapping_add(maj(b, c, d));

        index += 8;
    }

    [a, b, c, d, e, f, g, h]
}

/// Expand SHA-256 words, combining rotations before modular addition.
fn expand_sha256_schedule(words: &mut Vec<u32>, first_word: usize) {
    for index in first_word..64 {
        let x = words[index - 2];
        let y = words[index - 15];
        let s1 = sigma1(x);
        let s0 = sigma0(y);
        words.push(
            s1.wrapping_add(words[index - 7])
                .wrapping_add(s0)
                .wrapping_add(words[index - 16]),
        );
    }
}

/// Compute the initial SHA-256 state for token data (first seven bytes are salt).
fn sha256_init(data: &[u8]) -> [u32; 8] {
    let mut words = vec![0u32; 16];
    words[14] = (u32::from(data[0]) << 16) | (u32::from(data[1]) << 8) | u32::from(data[2]);
    words[15] = u32::from_be_bytes([data[3], data[4], data[5], data[6]]);

    for index in 16..30 {
        let mut value = SHA256_SCHEDULE[index - 16];
        if index - 2 >= 14 {
            let s = sigma1(words[index - 2]);
            value = value.wrapping_add(if index == 16 {
                s ^ SHA256_SIGMA1_ADJUSTMENT
            } else {
                s
            });
        }
        if index - 7 >= 14 {
            value = value.wrapping_add(words[index - 7]);
        }
        if index - 15 >= 14 {
            let s = sigma0(words[index - 15]);
            value = value.wrapping_add(if index == 29 {
                s ^ SHA256_SIGMA0_ADJUSTMENT
            } else {
                s
            });
        }
        words.push(value);
    }

    words.push(
        sigma1(words[28])
            .wrapping_add(words[23])
            .wrapping_add(sigma0(words[15]))
            .wrapping_add(words[14])
            .wrapping_add(SHA256_ADJUSTMENT)
            .wrapping_sub(SHA256_ROUND_CONSTANTS[14]),
    );

    expand_sha256_schedule(&mut words, 31);
    let working = sha256_rounds(SHA256_STATE, &words, 14);

    let mut state = [0u32; 8];
    for (index, item) in state.iter_mut().enumerate() {
        *item = SHA256_INITIAL[index].wrapping_add(working[index]);
    }
    state
}

/// Compute the SHA-256 digest for token data.
fn sha256_digest(data: &[u8]) -> [u8; 32] {
    let length = 57 + data.len();
    let mut padded = data[7..].to_vec();
    padded.push(0x80);
    let zeros = (((55isize - length as isize) % 64 + 64) % 64) as usize;
    padded.extend(std::iter::repeat_n(0u8, zeros));
    padded.extend_from_slice(&((length as u64) * 8).to_be_bytes());

    let mut state = sha256_init(&data[..7]);
    for chunk in padded.as_chunks::<64>().0 {
        let mut words: Vec<u32> = chunk
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_be_bytes(*c))
            .collect();
        expand_sha256_schedule(&mut words, 16);
        let working = sha256_rounds(state, &words, 0);
        for (index, item) in state.iter_mut().enumerate() {
            *item = item.wrapping_add(working[index]);
        }
    }

    let mut digest = [0u8; 32];
    for (index, word) in state.iter().enumerate() {
        digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    digest
}

/// Compute the CS64 digest words for token data.
fn cs64_digest(data: &[u8], digest: &[u8; 32]) -> [u32; 2] {
    let key_a = u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]]) | 1;
    let key_b = u32::from_le_bytes([digest[4], digest[5], digest[6], digest[7]]) | 1;
    let factors = [
        key_a.wrapping_mul(CS64_FACTOR),
        key_b.wrapping_mul(CS64_FACTOR),
    ];

    let mut terms: Vec<u32> = CS64_COEFFICIENTS
        .iter()
        .enumerate()
        .map(|(index, (coefficient, _))| coefficient.wrapping_mul(factors[index % 2]))
        .collect();
    let mut offsets: Vec<u32> = CS64_COEFFICIENTS
        .iter()
        .map(|(_, offset)| *offset)
        .collect();

    let mut input = Vec::with_capacity(data.len() + 1);
    input.push(0);
    input.extend_from_slice(data);

    for (index, word) in words_le4_even(&input).into_iter().enumerate() {
        let mut term = word.wrapping_mul(if index % 2 == 0 { key_a } else { key_b });
        let mut offset = rol16(word).wrapping_mul(if index % 2 == 0 {
            0xEF0569FB
        } else {
            0xC31713DB
        });
        if index == 0 {
            term = term.wrapping_add(CS64_ADJUSTMENT0.wrapping_mul(factors[0]));
            offset = offset.wrapping_add(CS64_ADJUSTMENT1);
        }
        terms.push(term);
        offsets.push(offset);
    }

    let mut swap_acc = 0u32;
    let mut swap_last = 0u32;
    let mut state = 0u32;
    let mut acc = 0u32;
    let mut index = 0;
    while index < terms.len() {
        let x = terms[index];
        let y = terms[index + 1];

        let mut t = rol16(offsets[index].wrapping_add(x));
        let mut q = t.wrapping_mul(0x76F50F34);
        swap_acc = swap_acc.wrapping_add(rol16(q).wrapping_mul(0xC3EFEA97).wrapping_add(q));
        t = rol16(offsets[index + 1].wrapping_add(y));
        q = t.wrapping_mul(0x1881CE3C);
        swap_last = rol16(q).wrapping_mul(0x35BD1EC9).wrapping_add(q);
        swap_acc = swap_acc.wrapping_add(swap_last);

        state = state.wrapping_mul(key_a).wrapping_add(x);
        state = rol16(state).wrapping_mul(0xCF98B111);
        state = rol16(state).wrapping_mul(0x87085B9F);
        state = rol16(state).wrapping_mul(0x12CEB96D);
        state = rol16(state).wrapping_mul(0x257E1D83);
        acc = acc.wrapping_add(state);

        t = state.wrapping_mul(key_b).wrapping_add(y);
        t = rol16(t).wrapping_mul(0xA27416F5);
        t = rol16(t).wrapping_mul(0xD38396FF);
        state = rol16(rol16(t).wrapping_mul(0x7C932B89)).wrapping_mul(0xBFA49F69);
        acc = acc.wrapping_add(state);

        index += 2;
    }

    [swap_last ^ state, swap_acc ^ acc]
}

fn hex_upper(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push_str(&format!("{byte:02X}"));
    }
    output
}

/// Compute the token hash components: `(cs64, sha256)` as upper-case hex.
fn token_hash(data: &[u8]) -> (String, String) {
    let digest = sha256_digest(data);
    let cs64 = cs64_digest(data, &digest);
    let mut cs64_bytes = [0u8; 8];
    cs64_bytes[..4].copy_from_slice(&cs64[0].to_le_bytes());
    cs64_bytes[4..].copy_from_slice(&cs64[1].to_le_bytes());
    (hex_upper(&cs64_bytes), hex_upper(&digest))
}

/// Generate a v2 `Sec-MS-GEC` token from Windows file-time ticks.
///
/// `ticks` are 100 ns intervals since 1601-01-01, already rounded down to a
/// five minute window. The result is formatted as `<16 hex>-<64 hex>`.
pub(crate) fn generate_sec_ms_gec_v2(ticks: u128) -> String {
    let str_to_hash = format!("{ticks}{TRUSTED_CLIENT_TOKEN}");
    let (cs64, sha256) = token_hash(str_to_hash.as_bytes());
    format!("{cs64}-{sha256}")
}

/// Generate a v2 `Sec-MS-GEC` token for the current time.
pub(crate) fn generate_sec_ms_gec() -> String {
    generate_sec_ms_gec_v2(current_ticks())
}

/// Windows file-time ticks (100 ns) for now, rounded down to a five minute window.
fn current_ticks() -> u128 {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before the Unix epoch")
        + Duration::from_secs(WIN_EPOCH);
    let ticks = duration.as_nanos() / 100;
    ticks - ticks % 3_000_000_000
}

#[cfg(test)]
mod tests {
    use super::generate_sec_ms_gec_v2;

    #[test]
    fn matches_edge_tts_7_3_0_reference_vectors() {
        // Generated with the independent upstream Python implementation.
        assert_eq!(
            generate_sec_ms_gec_v2(116444736000000000),
            "52BC7A17B50DFEC9-28C4394775C8751CD2F6859C030C06DB1489ACAB3B3A7651831FC26895177ED6"
        );
        assert_eq!(
            generate_sec_ms_gec_v2(133444734000000000),
            "2CD23A2CA0B2595A-F3C846C370E05F9CC83ACC5F995C5F5D87A4D0F14CE8936A0F902E64099E38A2"
        );
        assert_eq!(
            generate_sec_ms_gec_v2(134366889000000000),
            "F57B75CCA5B435C7-70822FD6D7536D9A10C418A7F11A94B0CF8BE3BDBCDCAE753B52A151890E9CBC"
        );
    }
}
