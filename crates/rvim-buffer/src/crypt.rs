//! Vim-compatible file encryption (reference: crypt.c, crypt_zip.c,
//! blowfish.c, sha256.c).
//!
//! The on-disk format is Vim's:
//!
//! ```text
//! "VimCrypt~01!"                    (zip: no salt/seed)
//! "VimCrypt~02!" <salt:8> <seed:8>  (blowfish)
//! "VimCrypt~03!" <salt:8> <seed:8>  (blowfish2)
//! <encrypted body>
//! ```
//!
//! The body is the ordinary file bytes (BOM/EOL included) run through the
//! method's stream cipher.  Blowfish key derivation follows Vim exactly:
//! 1001 SHA-256 hex digests (password/salt first, then the previous digest as
//! ASCII plus the salt), decoded to 32 key bytes.  This makes the ciphertext
//! readable by Vim and vice versa.

use rvim_core::{VimError, VimResult};

use crate::blowfish_tables::{PAX_INIT, SBX_INIT};

/// Vim's `'cryptmethod'` values that RustVim implements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CryptMethod {
    Zip,
    Blowfish,
    Blowfish2,
}

impl CryptMethod {
    pub fn name(self) -> &'static str {
        match self {
            CryptMethod::Zip => "zip",
            CryptMethod::Blowfish => "blowfish",
            CryptMethod::Blowfish2 => "blowfish2",
        }
    }

    pub fn from_name(name: &str) -> VimResult<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "" | "blowfish2" => Ok(CryptMethod::Blowfish2),
            "zip" => Ok(CryptMethod::Zip),
            "blowfish" | "bf" => Ok(CryptMethod::Blowfish),
            other => Err(VimError::new(
                "E474",
                format!("Invalid cryptmethod: {other}"),
            )),
        }
    }

    /// The 12-byte magic header Vim writes at the start of the file.
    pub fn magic(self) -> &'static [u8; 12] {
        match self {
            CryptMethod::Zip => b"VimCrypt~01!",
            CryptMethod::Blowfish => b"VimCrypt~02!",
            CryptMethod::Blowfish2 => b"VimCrypt~03!",
        }
    }

    fn salt_len(self) -> usize {
        match self {
            CryptMethod::Zip => 0,
            CryptMethod::Blowfish | CryptMethod::Blowfish2 => 8,
        }
    }

    fn seed_len(self) -> usize {
        self.salt_len()
    }

    pub fn header_len(self) -> usize {
        12 + self.salt_len() + self.seed_len()
    }

    pub fn from_magic(bytes: &[u8]) -> Option<Self> {
        [
            CryptMethod::Zip,
            CryptMethod::Blowfish,
            CryptMethod::Blowfish2,
        ]
        .into_iter()
        .find(|method| bytes.starts_with(method.magic()))
    }
}

/// Does this file start with one of Vim's encryption magics?
pub fn is_encrypted(bytes: &[u8]) -> bool {
    CryptMethod::from_magic(bytes).is_some()
}

/// Encrypt `plain` using `key`.  The returned bytes include the Vim header.
pub fn encrypt(plain: &[u8], key: &str, method: CryptMethod) -> Vec<u8> {
    let mut out = Vec::with_capacity(method.header_len() + plain.len());
    out.extend_from_slice(method.magic());
    let salt_len = method.salt_len();
    let seed_len = method.seed_len();
    if salt_len > 0 {
        let mut salt = [0u8; 8];
        let mut seed = [0u8; 8];
        fill_random(&mut salt);
        fill_random(&mut seed);
        out.extend_from_slice(&salt[..salt_len]);
        out.extend_from_slice(&seed[..seed_len]);
        let salt = &out[12..12 + salt_len];
        let seed = &out[12 + salt_len..12 + salt_len + seed_len];
        let mut cipher = StreamCipher::new(method, key, salt, seed);
        out.extend_from_slice(&cipher.encrypt(plain));
    } else {
        let mut cipher = StreamCipher::new(method, key, &[], &[]);
        out.extend_from_slice(&cipher.encrypt(plain));
    }
    out
}

/// Decrypt a Vim-encrypted byte stream.  Files without a recognised magic are
/// returned unchanged with `None`.
pub fn decrypt_if_needed(data: &[u8], key: &str) -> VimResult<(Vec<u8>, Option<CryptMethod>)> {
    let Some(method) = CryptMethod::from_magic(data) else {
        if data.starts_with(b"VimCrypt~") {
            return Err(VimError::new(
                "E821",
                "File is encrypted with an unsupported method",
            ));
        }
        return Ok((data.to_vec(), None));
    };
    if key.is_empty() {
        return Err(VimError::new(
            "E576",
            "Encrypted file requires an encryption key",
        ));
    }
    let header_len = method.header_len();
    if data.len() < header_len {
        return Err(VimError::new("E822", "Corrupt encrypted file header"));
    }
    let salt_len = method.salt_len();
    let seed_len = method.seed_len();
    let salt = &data[12..12 + salt_len];
    let seed = &data[12 + salt_len..12 + salt_len + seed_len];
    let mut cipher = StreamCipher::new(method, key, salt, seed);
    Ok((cipher.decrypt(&data[header_len..]), Some(method)))
}

// ---------------------------------------------------------------------------
// Stream ciphers
// ---------------------------------------------------------------------------

enum StreamCipher {
    Zip(ZipCipher),
    Blowfish(Box<BlowfishCfb>),
}

impl StreamCipher {
    fn new(method: CryptMethod, key: &str, salt: &[u8], seed: &[u8]) -> Self {
        match method {
            CryptMethod::Zip => StreamCipher::Zip(ZipCipher::new(key)),
            CryptMethod::Blowfish => {
                StreamCipher::Blowfish(Box::new(BlowfishCfb::new(method, key, salt, seed)))
            }
            CryptMethod::Blowfish2 => {
                StreamCipher::Blowfish(Box::new(BlowfishCfb::new(method, key, salt, seed)))
            }
        }
    }

    fn encrypt(&mut self, data: &[u8]) -> Vec<u8> {
        match self {
            StreamCipher::Zip(c) => c.encrypt(data),
            StreamCipher::Blowfish(c) => c.encrypt(data),
        }
    }

    fn decrypt(&mut self, data: &[u8]) -> Vec<u8> {
        match self {
            StreamCipher::Zip(c) => c.decrypt(data),
            StreamCipher::Blowfish(c) => c.decrypt(data),
        }
    }
}

// ---------------------------------------------------------------------------
// PKZIP stream cipher (crypt_zip.c)
// ---------------------------------------------------------------------------

struct ZipCipher {
    keys: [u32; 3],
}

fn crc_table() -> &'static [u32; 256] {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [0u32; 256];
        for (i, slot) in table.iter_mut().enumerate() {
            let mut v = i as u32;
            for _ in 0..8 {
                v = (v >> 1) ^ if v & 1 != 0 { 0xedb8_8320 } else { 0 };
            }
            *slot = v;
        }
        table
    })
}

fn crc32(crc: u32, byte: u8) -> u32 {
    crc_table()[((crc ^ u32::from(byte)) & 0xff) as usize] ^ (crc >> 8)
}

impl ZipCipher {
    fn new(key: &str) -> Self {
        let mut cipher = Self {
            keys: [305_419_896, 591_751_049, 878_082_192],
        };
        for byte in key.bytes() {
            cipher.update(byte);
        }
        cipher
    }

    fn update(&mut self, byte: u8) {
        self.keys[0] = crc32(self.keys[0], byte);
        self.keys[1] = self.keys[1]
            .wrapping_add(self.keys[0] & 0xff)
            .wrapping_mul(134_775_813)
            .wrapping_add(1);
        self.keys[2] = crc32(self.keys[2], (self.keys[1] >> 24) as u8);
    }

    fn next_byte(&self) -> u8 {
        let temp = (self.keys[2] | 2) as u16;
        (((u32::from(temp) * u32::from(temp ^ 1)) >> 8) & 0xff) as u8
    }

    fn encrypt(&mut self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len());
        for &plain in data {
            let t = self.next_byte();
            self.update(plain);
            out.push(t ^ plain);
        }
        out
    }

    fn decrypt(&mut self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len());
        for &cipher in data {
            let t = self.next_byte();
            let plain = cipher ^ t;
            self.update(plain);
            out.push(plain);
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Blowfish CFB (blowfish.c)
// ---------------------------------------------------------------------------

const BF_BLOCK: usize = 8;
const BF_MAX_CFB_LEN: usize = 64;

struct BlowfishCfb {
    pax: [u32; 18],
    sbx: [[u32; 256]; 4],
    cfb: Vec<u8>,
    rand_offset: usize,
    update_offset: usize,
    /// 8 for blowfish2, 64 for the old blowfish method.
    cfb_len: usize,
}

impl BlowfishCfb {
    fn new(method: CryptMethod, key: &str, salt: &[u8], seed: &[u8]) -> Self {
        let (pax, sbx) = bf_key_init(key, salt);
        let cfb_len = if method == CryptMethod::Blowfish {
            BF_MAX_CFB_LEN
        } else {
            BF_BLOCK
        };
        let mut cipher = Self {
            pax,
            sbx,
            cfb: vec![0; cfb_len],
            rand_offset: 0,
            update_offset: 0,
            cfb_len,
        };
        if !seed.is_empty() {
            let count = seed.len().max(cfb_len);
            for i in 0..count {
                cipher.cfb[i % cfb_len] ^= seed[i % seed.len()];
            }
        }
        cipher
    }

    fn encrypt_block(&self, block: &mut [u8]) {
        debug_assert_eq!(block.len(), BF_BLOCK);
        // Vim keeps the words in native endianness (little-endian on the
        // platforms RustVim targets), not network order.
        let mut left = u32::from_le_bytes([block[0], block[1], block[2], block[3]]);
        let mut right = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
        for i in 0..16 {
            left ^= self.pax[i];
            right ^= bf_f(&self.sbx, left);
            std::mem::swap(&mut left, &mut right);
        }
        std::mem::swap(&mut left, &mut right);
        right ^= self.pax[16];
        left ^= self.pax[17];
        block[0..4].copy_from_slice(&left.to_le_bytes());
        block[4..8].copy_from_slice(&right.to_le_bytes());
    }

    fn ran_byte(&mut self) -> u8 {
        if self.rand_offset % BF_BLOCK == 0 {
            let offset = self.rand_offset;
            let mut block = [0u8; BF_BLOCK];
            block.copy_from_slice(&self.cfb[offset..offset + BF_BLOCK]);
            self.encrypt_block(&mut block);
            self.cfb[offset..offset + BF_BLOCK].copy_from_slice(&block);
        }
        let byte = self.cfb[self.rand_offset];
        self.rand_offset = (self.rand_offset + 1) % self.cfb_len;
        byte
    }

    fn update(&mut self, byte: u8) {
        self.cfb[self.update_offset] ^= byte;
        self.update_offset = (self.update_offset + 1) % self.cfb_len;
    }

    fn encrypt(&mut self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len());
        for &plain in data {
            let t = self.ran_byte();
            self.update(plain);
            out.push(t ^ plain);
        }
        out
    }

    fn decrypt(&mut self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len());
        for &cipher in data {
            let t = self.ran_byte();
            let plain = cipher ^ t;
            self.update(plain);
            out.push(plain);
        }
        out
    }
}

fn bf_f(sbx: &[[u32; 256]; 4], x: u32) -> u32 {
    let a = (x >> 24) as usize;
    let b = ((x >> 16) & 0xff) as usize;
    let c = ((x >> 8) & 0xff) as usize;
    let d = (x & 0xff) as usize;
    (sbx[0][a].wrapping_add(sbx[1][b]) ^ sbx[2][c]).wrapping_add(sbx[3][d])
}

fn bf_encrypt_block(
    pax: &mut [u32; 18],
    sbx: &[[u32; 256]; 4],
    left: u32,
    right: u32,
) -> (u32, u32) {
    let mut l = left;
    let mut r = right;
    for &p in pax.iter().take(16) {
        l ^= p;
        r ^= bf_f(sbx, l);
        std::mem::swap(&mut l, &mut r);
    }
    std::mem::swap(&mut l, &mut r);
    r ^= pax[16];
    l ^= pax[17];
    (l, r)
}

/// Vim's bf_key_init(): SHA-256 key strengthening then the Blowfish key
/// schedule.
fn bf_key_init(password: &str, salt: &[u8]) -> ([u32; 18], [[u32; 256]; 4]) {
    let key = vim_key(password, salt);
    let mut pax = PAX_INIT;
    let mut sbx = SBX_INIT;
    let mut keypos = 0usize;
    for slot in pax.iter_mut() {
        let mut val = 0u32;
        for _ in 0..4 {
            val = (val << 8) | u32::from(key[keypos % key.len()]);
            keypos += 1;
        }
        *slot ^= val;
    }
    let (mut left, mut right) = (0u32, 0u32);
    let mut i = 0;
    while i < 18 {
        let (l, r) = bf_encrypt_block(&mut pax, &sbx, left, right);
        left = l;
        right = r;
        pax[i] = l;
        pax[i + 1] = r;
        i += 2;
    }
    for box_index in 0..4 {
        let mut j = 0;
        while j < 256 {
            let (l, r) = bf_encrypt_block(&mut pax, &sbx, left, right);
            left = l;
            right = r;
            sbx[box_index][j] = l;
            sbx[box_index][j + 1] = r;
            j += 2;
        }
    }
    (pax, sbx)
}

/// Vim's sha256_key() chain: 1001 hashes, decoded from hex to 32 bytes.
fn vim_key(password: &str, salt: &[u8]) -> [u8; 32] {
    let mut hex = sha256_hex(password.as_bytes(), salt);
    for _ in 0..1000 {
        hex = sha256_hex(hex.as_bytes(), salt);
    }
    let mut key = [0u8; 32];
    for (i, slot) in key.iter_mut().enumerate() {
        let pair = &hex.as_bytes()[i * 2..i * 2 + 2];
        let text = std::str::from_utf8(pair).unwrap_or("00");
        *slot = u8::from_str_radix(text, 16).unwrap_or(0);
    }
    key
}

// ---------------------------------------------------------------------------
// SHA-256 (sha256.c)
// ---------------------------------------------------------------------------

const SHA_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut state = [
        0x6a09e667u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut a = state[0];
        let mut b = state[1];
        let mut c = state[2];
        let mut d = state[3];
        let mut e = state[4];
        let mut f = state[5];
        let mut g = state[6];
        let mut h = state[7];
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(SHA_K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
        state[5] = state[5].wrapping_add(f);
        state[6] = state[6].wrapping_add(g);
        state[7] = state[7].wrapping_add(h);
    }
    let mut out = [0u8; 32];
    for (i, word) in state.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// `sha256(data || salt)` rendered as 64 lowercase hex characters.
fn sha256_hex(data: &[u8], salt: &[u8]) -> String {
    let mut input = Vec::with_capacity(data.len() + salt.len());
    input.extend_from_slice(data);
    input.extend_from_slice(salt);
    let digest = sha256(&input);
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Fill a buffer with unpredictable-enough bytes without a rand dependency.
fn fill_random(out: &mut [u8]) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut seed = Vec::new();
    seed.extend_from_slice(&now.to_le_bytes());
    seed.extend_from_slice(&std::process::id().to_le_bytes());
    seed.extend_from_slice(&count.to_le_bytes());
    let digest = sha256(&seed);
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = digest[i % digest.len()];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_the_standard_vectors() {
        let hex = |data: &[u8]| {
            let digest = sha256(data);
            digest
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        };
        assert_eq!(
            hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn blowfish_self_test_vector() {
        // Vim blowfish.c bf_test_data[0]: password "password", salt "salt",
        // plain "plaintxt" -> ad 3d fa 7f e8 ea 40 f6.
        let (pax, sbx) = bf_key_init("password", b"salt");
        let mut block = *b"plaintxt";
        // Vim keeps the words in native endianness (little-endian on the
        // platforms RustVim targets), not network order.
        let mut left = u32::from_le_bytes([block[0], block[1], block[2], block[3]]);
        let mut right = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
        for &p in pax.iter().take(16) {
            left ^= p;
            right ^= bf_f(&sbx, left);
            std::mem::swap(&mut left, &mut right);
        }
        std::mem::swap(&mut left, &mut right);
        right ^= pax[16];
        left ^= pax[17];
        block[0..4].copy_from_slice(&left.to_le_bytes());
        block[4..8].copy_from_slice(&right.to_le_bytes());
        assert_eq!(block, [0xad, 0x3d, 0xfa, 0x7f, 0xe8, 0xea, 0x40, 0xf6]);
    }

    #[test]
    fn every_method_round_trips() {
        for method in [
            CryptMethod::Zip,
            CryptMethod::Blowfish,
            CryptMethod::Blowfish2,
        ] {
            let plain = b"alpha\nbeta\n";
            let data = encrypt(plain, "secret", method);
            assert_eq!(&data[..12], method.magic());
            let (back, found) = decrypt_if_needed(&data, "secret").unwrap();
            assert_eq!(back, plain);
            assert_eq!(found, Some(method));
        }
    }

    #[test]
    fn plain_files_are_returned_unchanged() {
        assert_eq!(
            decrypt_if_needed(b"alpha\n", "secret").unwrap(),
            (b"alpha\n".to_vec(), None)
        );
    }

    #[test]
    fn wrong_key_produces_garbage_not_a_panic() {
        let data = encrypt(b"alpha\n", "secret", CryptMethod::Blowfish2);
        let (wrong, _) = decrypt_if_needed(&data, "wrong").unwrap();
        assert_ne!(wrong, b"alpha\n");
    }
}
