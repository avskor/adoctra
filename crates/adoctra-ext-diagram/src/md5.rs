//! MD5 (RFC 1321) for asciidoctor-diagram's checksums (`diag-<type>-md5-…`
//! image names). Not used for security.

const S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

fn k(i: usize) -> u32 {
    // floor(abs(sin(i + 1)) * 2^32)
    ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32
}

/// An incremental MD5 digest (Ruby's `Digest::MD5` with `<<`).
#[derive(Clone)]
pub struct Md5 {
    state: [u32; 4],
    buffer: Vec<u8>,
    length: u64,
}

impl Default for Md5 {
    fn default() -> Self {
        Self::new()
    }
}

impl Md5 {
    pub fn new() -> Self {
        Self {
            state: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476],
            buffer: Vec::with_capacity(64),
            length: 0,
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u64);
        self.buffer.extend_from_slice(data);
        let full = self.buffer.len() / 64 * 64;
        let blocks: Vec<u8> = self.buffer.drain(..full).collect();
        for block in blocks.chunks_exact(64) {
            self.block(block);
        }
    }

    fn block(&mut self, block: &[u8]) {
        let mut m = [0u32; 16];
        for (i, word) in m.iter_mut().enumerate() {
            *word = u32::from_le_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        let [mut a, mut b, mut c, mut d] = self.state;
        for (i, shift) in S.iter().enumerate() {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(k(i)).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(*shift));
        }
        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
    }

    /// The lowercase hexadecimal digest (Ruby's `hexdigest`).
    pub fn hexdigest(mut self) -> String {
        let bits = self.length.wrapping_mul(8);
        let mut tail = vec![0x80u8];
        while (self.buffer.len() + tail.len()) % 64 != 56 {
            tail.push(0);
        }
        tail.extend_from_slice(&bits.to_le_bytes());
        let length = self.length;
        self.update(&tail);
        self.length = length;
        self.state
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::Md5;

    fn md5(parts: &[&str]) -> String {
        let mut digest = Md5::new();
        for part in parts {
            digest.update(part.as_bytes());
        }
        digest.hexdigest()
    }

    #[test]
    fn matches_rfc_1321_vectors() {
        assert_eq!(md5(&[""]), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5(&["a"]), "0cc175b9c0f1b6a831c399e269772661");
        assert_eq!(md5(&["abc"]), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(md5(&["message digest"]), "f96b697d7cb7938d525a2f31aaf161d0");
        assert_eq!(
            md5(&["abcdefghijklmnopqrstuvwxyz"]),
            "c3fcd3d76192e4007dfb496cca67e13b"
        );
        assert_eq!(
            md5(&[
                "12345678901234567890123456789012345678901234567890123456789012345678901234567890"
            ]),
            "57edf4a22be3c955ac49da2e2107b67a"
        );
        // Incremental updates across block boundaries.
        let long =
            "12345678901234567890123456789012345678901234567890123456789012345678901234567890";
        for split in [1, 31, 55, 56, 63, 64, 65] {
            assert_eq!(
                md5(&[&long[..split], &long[split..]]),
                "57edf4a22be3c955ac49da2e2107b67a",
                "{split}"
            );
        }
    }
}
