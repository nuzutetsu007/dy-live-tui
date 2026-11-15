//! 抖音 web 端 `a_bogus` 签名（纯 Rust，无第三方依赖）。
//!
//! 算法是 JS 版的等价移植：RC4 + SM3 + 抖音自定义 base64。
//! 只用于 `/webcast/web/partition/detail/room/v2/` 这类需要签名的只读接口。
//!
//! 注意：签名用的 query 必须和实际发送的 query **逐字节一致**，
//! `User-Agent` 也必须和签名时一致。

const SM3_IV: [u32; 8] = [
    1937774191, 1226093241, 388252375, 3666478592, 2842636476, 372324522, 3817729613, 2969243214,
];

const S3: &[u8] = b"ckdp1h4ZKsUB80/Mfvw36XIgR25+WQAlEi7NLboqYTOPuzmFjJnryx9HVGDaStCe";
const S4: &[u8] = b"Dkdpgh2ZmsQB80/MfvV36XI1R45-WUAlEixNLwoqYTOPuzKFjJnry79HbGcaStCe";

/// 与 Python/JS 参考实现一致的浏览器窗口环境串。
const WINDOW_ENV: &[u8] = b"1536|747|1536|834|0|30|0|0|1536|834|1536|864|1525|747|24|24|Win32";

fn to_i32(x: i64) -> i32 {
    x as i32
}

fn shr(a: i64, b: u32) -> i64 {
    (to_i32(a) >> (b & 31)) as i64
}

fn rol(e: u32, r: u32) -> u32 {
    e.rotate_left(r & 31)
}

// ------------------------------------------------------------------ RC4

fn rc4_encrypt(plaintext: &[u8], key: &[u8]) -> Vec<u8> {
    let mut s: [u8; 256] = std::array::from_fn(|i| i as u8);
    let mut j = 0usize;
    for i in 0..256 {
        let k = if key.is_empty() {
            0
        } else {
            key[i % key.len()]
        };
        j = (j + s[i] as usize + k as usize) % 256;
        s.swap(i, j);
    }
    let mut out = Vec::with_capacity(plaintext.len());
    let (mut i, mut j) = (0usize, 0usize);
    for &ch in plaintext {
        i = (i + 1) % 256;
        j = (j + s[i] as usize) % 256;
        s.swap(i, j);
        out.push(ch ^ s[(s[i] as usize + s[j] as usize) % 256]);
    }
    out
}

// ------------------------------------------------------------------ SM3

fn sm3_tj(j: usize) -> u32 {
    if j < 16 {
        2043430169
    } else {
        2055708042
    }
}

fn sm3_ff(j: usize, x: u32, y: u32, z: u32) -> u32 {
    if j < 16 {
        x ^ y ^ z
    } else {
        (x & y) | (x & z) | (y & z)
    }
}

fn sm3_gg(j: usize, x: u32, y: u32, z: u32) -> u32 {
    if j < 16 {
        x ^ y ^ z
    } else {
        (x & y) | (!x & z)
    }
}

fn sm3_compress(reg: &mut [u32; 8], block: &[u8]) {
    debug_assert_eq!(block.len(), 64);
    let mut f = [0u32; 132];
    for i in 0..16 {
        f[i] = ((block[4 * i] as u32) << 24)
            | ((block[4 * i + 1] as u32) << 16)
            | ((block[4 * i + 2] as u32) << 8)
            | (block[4 * i + 3] as u32);
    }
    for n in 16..68 {
        let a = f[n - 16] ^ f[n - 9] ^ rol(f[n - 3], 15);
        let a = a ^ rol(a, 15) ^ rol(a, 23);
        f[n] = a ^ rol(f[n - 13], 7) ^ f[n - 6];
    }
    for n in 0..64 {
        f[n + 68] = f[n] ^ f[n + 4];
    }

    let mut i = *reg;
    for c in 0..64 {
        let o = rol(i[0], 12)
            .wrapping_add(i[4])
            .wrapping_add(rol(sm3_tj(c), c as u32));
        let o = rol(o, 7);
        let s = o ^ rol(i[0], 12);
        let u = sm3_ff(c, i[0], i[1], i[2])
            .wrapping_add(i[3])
            .wrapping_add(s)
            .wrapping_add(f[c + 68]);
        let b = sm3_gg(c, i[4], i[5], i[6])
            .wrapping_add(i[7])
            .wrapping_add(o)
            .wrapping_add(f[c]);
        i[3] = i[2];
        i[2] = rol(i[1], 9);
        i[1] = i[0];
        i[0] = u;
        i[7] = i[6];
        i[6] = rol(i[5], 19);
        i[5] = i[4];
        i[4] = b ^ rol(b, 9) ^ rol(b, 17);
    }
    for l in 0..8 {
        reg[l] ^= i[l];
    }
}

fn sm3_hash(data: &[u8]) -> [u8; 32] {
    let mut reg = SM3_IV;
    let mut padded = Vec::with_capacity(data.len() + 72);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    let bits = (data.len() as u64).wrapping_mul(8);
    padded.extend_from_slice(&bits.to_be_bytes());

    for block in padded.chunks(64) {
        sm3_compress(&mut reg, block);
    }

    let mut out = [0u8; 32];
    for (i, v) in reg.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

// --------------------------------------------------- 抖音自定义 base64

fn result_encrypt(long_str: &[u8], table: &[u8]) -> Vec<u8> {
    // 原始 JS 的循环上界是浮点 `len / 3 * 4`，长度不是 3 的倍数时
    // 会少输出最后 4 个字符里的 1~2 个（而不是补 `=`）。必须保持同样语义。
    let out_len = (long_str.len() * 4).div_ceil(3);
    let mut out = Vec::with_capacity(out_len);
    for group in long_str.chunks(3) {
        let b0 = group[0] as u32;
        let b1 = *group.get(1).unwrap_or(&0) as u32;
        let b2 = *group.get(2).unwrap_or(&0) as u32;
        out.push(table[(b0 >> 2) as usize]);
        out.push(table[(((b0 & 3) << 4) | (b1 >> 4)) as usize]);
        out.push(table[(((b1 & 15) << 2) | (b2 >> 6)) as usize]);
        out.push(table[(b2 & 63) as usize]);
    }
    out.truncate(out_len);
    out
}

// -------------------------------------------------------------- a_bogus

fn gener_random(random_v: i64, option: [i64; 2]) -> [u8; 4] {
    let rv = to_i32(random_v) as i64;
    [
        ((rv & 255 & 170) | (option[0] & 85)) as u8,
        ((rv & 255 & 85) | (option[0] & 170)) as u8,
        ((shr(rv, 8) & 255 & 170) | (option[1] & 85)) as u8,
        ((shr(rv, 8) & 255 & 85) | (option[1] & 170)) as u8,
    ]
}

fn generate_random_str<R: FnMut() -> f64>(rand: &mut R) -> Vec<u8> {
    let mut out = Vec::with_capacity(12);
    out.extend_from_slice(&gener_random((rand() * 10000.0) as i64, [3, 45]));
    out.extend_from_slice(&gener_random((rand() * 10000.0) as i64, [1, 0]));
    out.extend_from_slice(&gener_random((rand() * 10000.0) as i64, [1, 5]));
    out
}

fn js_div_int(x: i64, div: f64) -> i64 {
    (x as f64 / div) as i64
}

fn generate_rc4_bb_str(query: &str, user_agent: &str, now: u64) -> Vec<u8> {
    let mut with_suffix = Vec::with_capacity(query.len() + 3);
    with_suffix.extend_from_slice(query.as_bytes());
    with_suffix.extend_from_slice(b"cus");

    let url_search_params_list = sm3_hash(&sm3_hash(&with_suffix));
    let cus = sm3_hash(&sm3_hash(b"cus"));
    let ua_rc4 = rc4_encrypt(user_agent.as_bytes(), &[0, 1, 14]);
    let ua_b64 = result_encrypt(&ua_rc4, S3);
    let ua = sm3_hash(&ua_b64);

    let start_time = now as i64;
    let end_time = now as i64;
    let page_id: i64 = 6241;
    let aid: i64 = 6383;
    let arguments: [i64; 3] = [0, 1, 14];

    let mut b = [0i64; 73];
    b[8] = 3;
    b[10] = end_time;
    b[16] = start_time;
    b[18] = 44;

    b[20] = shr(b[16], 24) & 255;
    b[21] = shr(b[16], 16) & 255;
    b[22] = shr(b[16], 8) & 255;
    b[23] = b[16] & 255;
    b[24] = shr(js_div_int(b[16], 4294967296.0), 0);
    b[25] = shr(js_div_int(b[16], 1099511627776.0), 0);

    b[26] = shr(arguments[0], 24) & 255;
    b[27] = shr(arguments[0], 16) & 255;
    b[28] = shr(arguments[0], 8) & 255;
    b[29] = arguments[0] & 255;
    b[30] = (arguments[1] / 256) & 255;
    b[31] = arguments[1] % 256;
    b[32] = shr(arguments[1], 24) & 255;
    b[33] = shr(arguments[1], 16) & 255;
    b[34] = shr(arguments[2], 24) & 255;
    b[35] = shr(arguments[2], 16) & 255;
    b[36] = shr(arguments[2], 8) & 255;
    b[37] = arguments[2] & 255;

    b[38] = url_search_params_list[21] as i64;
    b[39] = url_search_params_list[22] as i64;
    b[40] = cus[21] as i64;
    b[41] = cus[22] as i64;
    b[42] = ua[23] as i64;
    b[43] = ua[24] as i64;

    b[44] = shr(b[10], 24) & 255;
    b[45] = shr(b[10], 16) & 255;
    b[46] = shr(b[10], 8) & 255;
    b[47] = b[10] & 255;
    b[48] = b[8];
    b[49] = shr(js_div_int(b[10], 4294967296.0), 0);
    b[50] = shr(js_div_int(b[10], 1099511627776.0), 0);

    b[51] = page_id;
    b[52] = shr(page_id, 24) & 255;
    b[53] = shr(page_id, 16) & 255;
    b[54] = shr(page_id, 8) & 255;
    b[55] = page_id & 255;
    b[56] = aid;
    b[57] = aid & 255;
    b[58] = shr(aid, 8) & 255;
    b[59] = shr(aid, 16) & 255;
    b[60] = shr(aid, 24) & 255;

    b[64] = WINDOW_ENV.len() as i64;
    b[65] = b[64] & 255;
    b[66] = shr(b[64], 8) & 255;
    // b[69..71] 保持 0

    let xor_idx = [
        18, 20, 26, 30, 38, 40, 42, 21, 27, 31, 35, 39, 41, 43, 22, 28, 32, 36, 23, 29, 33, 37, 44,
        45, 46, 47, 48, 49, 50, 24, 25, 52, 53, 54, 55, 57, 58, 59, 60, 65, 66, 70, 71,
    ];
    let mut acc = 0i64;
    for idx in xor_idx {
        acc ^= b[idx];
    }
    b[72] = acc;

    let bb_idx = [
        18, 20, 52, 26, 30, 34, 58, 38, 40, 53, 42, 21, 27, 54, 55, 31, 35, 57, 39, 41, 43, 22, 28,
        32, 60, 36, 23, 29, 33, 37, 44, 45, 59, 46, 47, 48, 49, 50, 24, 25, 65, 66, 70, 71,
    ];
    let mut bb: Vec<u8> = bb_idx.iter().map(|&i| (b[i] & 255) as u8).collect();
    bb.extend_from_slice(WINDOW_ENV);
    bb.push((b[72] & 255) as u8);

    rc4_encrypt(&bb, b"y")
}

fn percent_encode(raw: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(raw.len() * 3);
    for &b in raw {
        let safe = b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b);
        if safe {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 15) as usize] as char);
        }
    }
    out
}

fn sign_with_rand<R: FnMut() -> f64>(
    query: &str,
    user_agent: &str,
    rand: &mut R,
    now: u64,
) -> String {
    let mut result_str = generate_random_str(rand);
    result_str.extend_from_slice(&generate_rc4_bb_str(query, user_agent, now));
    let mut raw = result_encrypt(&result_str, S4);
    raw.push(b'=');
    percent_encode(&raw)
}

fn xorshift64(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 生成可直接拼进 query 的 `a_bogus`（已 percent-encode）。
pub fn sign(query: &str, user_agent: &str) -> String {
    let now = now_ms();
    let mut state = now ^ (std::process::id() as u64) ^ 0x9E37_79B9_7F4A_7C15;
    if state == 0 {
        state = 1;
    }
    let mut rand = move || {
        let x = xorshift64(&mut state);
        (x >> 11) as f64 / (1u64 << 53) as f64
    };
    sign_with_rand(query, user_agent, &mut rand, now)
}

#[cfg(test)]
mod tests {
    use super::*;

    const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

    fn sign_fixed(query: &str, ua: &str, rands: &[f64], now: u64) -> String {
        let mut i = 0;
        let mut rand = || {
            let v = rands[i];
            i += 1;
            v
        };
        sign_with_rand(query, ua, &mut rand, now)
    }

    #[test]
    fn sm3_known_vector() {
        let got = sm3_hash(b"abc");
        let hex: String = got.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "66c7f0f462eeedd9d1f2d46bdc10e4e24167c4875cf2f7a2297da02b8f4ba8e0"
        );
    }

    #[test]
    fn rc4_known_vector() {
        let got = rc4_encrypt(b"Plaintext", b"Key");
        let hex: String = got.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "bbf316e8d940af0ad3");
    }

    #[test]
    fn golden_signature_vectors() {
        // 向量由 /tmp/opencode/abogus.py 在固定 rand/now 下生成。
        assert_eq!(
            sign_fixed("a=1&b=2", UA, &[0.5; 12], 1_700_000_000_000),
            "xfmZ%2FRLDDi2sDDWv56KLfY3q6lM3Yh9I0trEMD2fpxftqL39HMYD9exoIBGvXY8jwG%2F-IeYjy4hbT3ohrQ2y8qwf9W0L%2F25gsDSkKl12so0j53inCLf%2FE0iE5hsAtFH8svr4iKi8owICSYyhldAJ5kIlO62-zo0%2F9lR%3D"
        );
        assert_eq!(
            sign_fixed(
                "aid=6383&app_name=douyin_web&count=15&offset=0&partition=101&partition_type=4",
                UA,
                &[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 0.05, 0.15, 0.25],
                1_700_000_000_123,
            ),
            "O6mZQRhfDEITgDSk56KLfY3q6AF3Yh9I0trEMD2fpnftqL39HMTa9exoIBGvXFEjwG%2F-IeYjy4hbT3ohrQ2y8qwf9W0L%2F25gsDSkKl12so0j53inCLf%2FE0iE5hsAtFH8svr4iKi8owICSYyhldAJ5kIlO62-zo0%2F9AS%3D"
        );
        assert_eq!(
            sign_fixed("", UA, &[0.123456789; 12], 1_700_000_000_999),
            "E7mhBm0fDDVp6D6g56KLfY3q6fe3Yh9I0trEMD2fvxftq639HMPq9exoIBGvXIfjwG%2F-IeYjy4hbT3ohrQ2y8qwf9W0L%2F25gsDSkKl12so0j53inCLf%2FE0iE5hsAtFH8svr4iKi8owICSYyhldAJ5kIlO62-zo0%2F96m%3D"
        );
        assert_eq!(
            sign_fixed(
                "partition=1010017&partition_type=1&offset=45&count=15",
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/157.0.0.0 Safari/537.36",
                &[0.99, 0.01, 0.37, 0.42, 0.73, 0.11, 0.28, 0.64, 0.55, 0.9, 0.05, 0.77],
                1_763_200_000_123,
            ),
            "OfRqBdhgDDDiXDSg5fQLfY3q6Rl3YgK-0trEMD2f5VfzhL39HMTa9exog-0vHFEjiT%2FQIeYjy4hbT3ohrQ2y8qwf9W0L%2F25gsDSkKl12so0j53inCLf%2FE0iE5hsAtFH8svr4iKi8owICSYyhldAJ5kIlO62-zo0%2F9A8%3D"
        );
    }

    #[test]
    fn signature_changes_with_query_and_ua() {
        let rands = [0.3; 12];
        let a = sign_fixed("a=1", UA, &rands, 1_700_000_000_000);
        let b = sign_fixed("a=2", UA, &rands, 1_700_000_000_000);
        let c = sign_fixed("a=1", "OtherAgent/1.0", &rands, 1_700_000_000_000);
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert!(!a.contains('/'), "斜杠必须被 percent-encode: {a}");
        assert!(!a.contains('+'), "加号必须被 percent-encode: {a}");
    }

    #[test]
    fn production_sign_is_url_safe() {
        let sig = sign("aid=6383&count=15&offset=0", UA);
        assert!(!sig.is_empty());
        assert!(sig.len() > 100);
        assert!(sig
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.!~*'()%".contains(c)));
    }
}
