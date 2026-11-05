//! QR codes as barby's `QrCode` makes them with rqrcode_core 1.2.0: the
//! version from barby's byte capacities at level L, the segment mode
//! (numeric, alphanumeric or bytes), Reed-Solomon blocks, the mask with the
//! fewest demerit points. Polynomials keep rqrcode_core's `nil` tail.

use super::barby::Failure;

/// barby's `QrCode::SIZES` at level L: the byte capacity of each version.
const CAPACITY_L: [usize; 40] = [
    17, 32, 53, 78, 106, 134, 154, 192, 230, 271, 321, 367, 425, 458, 520, 586, 644, 718, 792, 858,
    929, 1003, 1091, 1171, 1273, 1367, 1465, 1528, 1628, 1732, 1840, 1952, 2068, 2188, 2303, 2431,
    2563, 2699, 2809, 2953,
];

/// rqrcode_core's `QRERRORCORRECTLEVEL[:l]`.
const LEVEL_L: u32 = 1;

/// barby's `QrCode#encoding`: rows of `1` (dark) and `0`.
pub(crate) fn barby_qr(data: &str) -> Result<Vec<String>, Failure> {
    let version = CAPACITY_L
        .iter()
        .position(|max| *max >= data.len())
        .map(|i| i + 1)
        .ok_or(Failure::Invalid)?;
    let modules = QrCode::new(data, version)
        .map_err(Failure::Message)?
        .modules;
    Ok(modules
        .iter()
        .map(|row| {
            row.iter()
                .map(|m| if *m == Some(true) { '1' } else { '0' })
                .collect()
        })
        .collect())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Number,
    AlphaNumeric,
    Byte,
}

impl Mode {
    fn bits(self) -> u32 {
        match self {
            Mode::Number => 1,
            Mode::AlphaNumeric => 2,
            Mode::Byte => 4,
        }
    }
    /// `QRUtil.get_length_in_bits`.
    fn length_bits(self, version: usize) -> usize {
        let index = match version {
            1..=9 => 0,
            10..=26 => 1,
            _ => 2,
        };
        match self {
            Mode::Number => [10, 12, 14][index],
            Mode::AlphaNumeric => [9, 11, 13][index],
            Mode::Byte => [8, 16, 16][index],
        }
    }
}

const ALPHANUMERIC: &str = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:";

struct BitBuffer {
    buffer: Vec<u8>,
    length: usize,
}

impl BitBuffer {
    fn put(&mut self, num: u32, length: usize) {
        for i in 0..length {
            self.put_bit((num >> (length - i - 1)) & 1 == 1);
        }
    }
    fn put_bit(&mut self, bit: bool) {
        let index = self.length / 8;
        if self.buffer.len() <= index {
            self.buffer.push(0);
        }
        if bit {
            self.buffer[index] |= 0x80 >> (self.length % 8);
        }
        self.length += 1;
    }
}

/// `QRMath` tables.
struct Galois {
    exp: [i32; 256],
    log: [i32; 256],
}

impl Galois {
    fn new() -> Self {
        let mut exp = [0i32; 256];
        let mut log = [0i32; 256];
        for (i, slot) in exp.iter_mut().enumerate().take(8) {
            *slot = 1 << i;
        }
        for i in 8..256 {
            exp[i] = exp[i - 4] ^ exp[i - 5] ^ exp[i - 6] ^ exp[i - 8];
        }
        for i in 0..255 {
            log[exp[i] as usize] = i as i32;
        }
        Self { exp, log }
    }
    /// `glog`: `nil` and 0 fail like Ruby.
    fn glog(&self, n: Option<i32>) -> Result<i32, String> {
        match n {
            None => Err("undefined method `<' for nil".to_owned()),
            Some(n) if n < 1 => Err(format!("glog({n})")),
            Some(n) => Ok(self.log[n as usize]),
        }
    }
    fn gexp(&self, mut n: i32) -> i32 {
        while n < 0 {
            n += 255;
        }
        while n >= 256 {
            n -= 255;
        }
        self.exp[n as usize]
    }
}

/// `QRPolynomial`: coefficients, `None` where Ruby's array holds `nil`.
#[derive(Clone)]
struct Polynomial(Vec<Option<i32>>);

impl Polynomial {
    fn new(num: &[Option<i32>], shift: usize) -> Self {
        let offset = num.iter().take_while(|n| **n == Some(0)).count();
        let mut out: Vec<Option<i32>> = num[offset..].to_vec();
        out.extend(std::iter::repeat(None).take(shift));
        Polynomial(out)
    }
    fn len(&self) -> usize {
        self.0.len()
    }
    fn multiply(&self, e: &Polynomial, g: &Galois) -> Result<Polynomial, String> {
        let mut num: Vec<Option<i32>> = vec![None; self.len() + e.len() - 1];
        for i in 0..self.len() {
            for j in 0..e.len() {
                let tmp = num[i + j].unwrap_or(0);
                num[i + j] = Some(tmp ^ g.gexp(g.glog(self.0[i])? + g.glog(e.0[j])?));
            }
        }
        Ok(Polynomial::new(&num, 0))
    }
    fn modulo(&self, e: &Polynomial, g: &Galois) -> Result<Polynomial, String> {
        let mut current = self.clone();
        loop {
            if current.len() < e.len() {
                return Ok(current);
            }
            let ratio = g.glog(current.0[0])? - g.glog(e.0[0])?;
            let mut num = current.0.clone();
            for (slot, coefficient) in num.iter_mut().zip(&e.0) {
                let tmp = slot.unwrap_or(0);
                *slot = Some(tmp ^ g.gexp(g.glog(*coefficient)? + ratio));
            }
            current = Polynomial::new(&num, 0);
        }
    }
}

struct QrCode {
    modules: Vec<Vec<Option<bool>>>,
}

include!("qr_rs_blocks.rs");

const PATTERN_POSITIONS: [&[usize]; 40] = [
    &[],
    &[6, 18],
    &[6, 22],
    &[6, 26],
    &[6, 30],
    &[6, 34],
    &[6, 22, 38],
    &[6, 24, 42],
    &[6, 26, 46],
    &[6, 28, 50],
    &[6, 30, 54],
    &[6, 32, 58],
    &[6, 34, 62],
    &[6, 26, 46, 66],
    &[6, 26, 48, 70],
    &[6, 26, 50, 74],
    &[6, 30, 54, 78],
    &[6, 30, 56, 82],
    &[6, 30, 58, 86],
    &[6, 34, 62, 90],
    &[6, 28, 50, 72, 94],
    &[6, 26, 50, 74, 98],
    &[6, 30, 54, 78, 102],
    &[6, 28, 54, 80, 106],
    &[6, 32, 58, 84, 110],
    &[6, 30, 58, 86, 114],
    &[6, 34, 62, 90, 118],
    &[6, 26, 50, 74, 98, 122],
    &[6, 30, 54, 78, 102, 126],
    &[6, 26, 52, 78, 104, 130],
    &[6, 30, 56, 82, 108, 134],
    &[6, 34, 60, 86, 112, 138],
    &[6, 30, 58, 86, 114, 142],
    &[6, 34, 62, 90, 118, 146],
    &[6, 30, 54, 78, 102, 126, 150],
    &[6, 24, 50, 76, 102, 128, 154],
    &[6, 28, 54, 80, 106, 132, 158],
    &[6, 32, 58, 84, 110, 136, 162],
    &[6, 26, 54, 82, 110, 138, 166],
    &[6, 30, 58, 86, 114, 142, 170],
];

const G15: u32 = (1 << 10) | (1 << 8) | (1 << 5) | (1 << 4) | (1 << 2) | (1 << 1) | 1;
const G18: u32 = (1 << 12) | (1 << 11) | (1 << 10) | (1 << 9) | (1 << 8) | (1 << 5) | (1 << 2) | 1;
const G15_MASK: u32 = (1 << 14) | (1 << 12) | (1 << 10) | (1 << 4) | (1 << 1);

fn bch_digit(mut data: u32) -> i32 {
    let mut digit = 0;
    while data != 0 {
        digit += 1;
        data >>= 1;
    }
    digit
}

fn bch_format_info(data: u32) -> u32 {
    let mut d = data << 10;
    while bch_digit(d) - bch_digit(G15) >= 0 {
        d ^= G15 << (bch_digit(d) - bch_digit(G15));
    }
    ((data << 10) | d) ^ G15_MASK
}

fn bch_version(data: u32) -> u32 {
    let mut d = data << 12;
    while bch_digit(d) - bch_digit(G18) >= 0 {
        d ^= G18 << (bch_digit(d) - bch_digit(G18));
    }
    (data << 12) | d
}

fn mask(pattern: usize, i: usize, j: usize) -> bool {
    match pattern {
        0 => (i + j) % 2 == 0,
        1 => i % 2 == 0,
        2 => j % 3 == 0,
        3 => (i + j) % 3 == 0,
        4 => (i / 2 + j / 3) % 2 == 0,
        5 => (i * j) % 2 + (i * j) % 3 == 0,
        6 => ((i * j) % 2 + (i * j) % 3) % 2 == 0,
        _ => ((i * j) % 3 + (i + j) % 2) % 2 == 0,
    }
}

impl QrCode {
    fn new(data: &str, version: usize) -> Result<Self, String> {
        let mode = if data.chars().all(|c| c.is_ascii_digit()) {
            Mode::Number
        } else if data.chars().all(|c| ALPHANUMERIC.contains(c)) {
            Mode::AlphaNumeric
        } else {
            Mode::Byte
        };
        let count = version * 4 + 17;
        let mut common = vec![vec![None; count]; count];
        place_probe(&mut common, 0, 0);
        place_probe(&mut common, count - 7, 0);
        place_probe(&mut common, 0, count - 7);
        let positions = PATTERN_POSITIONS[version - 1];
        for &row in positions {
            for &col in positions {
                if common[row][col].is_some() {
                    continue;
                }
                for r in -2i32..=2 {
                    for c in -2i32..=2 {
                        common[(row as i32 + r) as usize][(col as i32 + c) as usize] =
                            Some(r.abs() == 2 || c.abs() == 2 || (r == 0 && c == 0));
                    }
                }
            }
        }
        for (i, row) in common.iter_mut().enumerate().take(count - 8).skip(8) {
            row[6] = Some(i % 2 == 0);
        }
        for (i, slot) in common[6].iter_mut().enumerate().take(count - 8).skip(8) {
            *slot = Some(i % 2 == 0);
        }
        let data_bytes = create_data(version, data, mode)?;
        let make = |test: bool, pattern: usize| {
            let mut modules = common.clone();
            place_format_info(&mut modules, test, pattern);
            if version >= 7 {
                place_version_info(&mut modules, test, version);
            }
            map_data(&mut modules, &data_bytes, pattern);
            modules
        };
        let mut best = 0;
        let mut min_lost = 0;
        for pattern in 0..8 {
            let lost = lost_points(&make(true, pattern));
            if pattern == 0 || min_lost > lost {
                min_lost = lost;
                best = pattern;
            }
        }
        Ok(QrCode {
            modules: make(false, best),
        })
    }
}

fn place_probe(modules: &mut [Vec<Option<bool>>], row: usize, col: usize) {
    let count = modules.len() as i32;
    for r in -1i32..=7 {
        if !(0..count).contains(&(row as i32 + r)) {
            continue;
        }
        for c in -1i32..=7 {
            if !(0..count).contains(&(col as i32 + c)) {
                continue;
            }
            let vertical = (0..=6).contains(&r) && (c == 0 || c == 6);
            let horizontal = (0..=6).contains(&c) && (r == 0 || r == 6);
            let square = (2..=4).contains(&r) && (2..=4).contains(&c);
            modules[(row as i32 + r) as usize][(col as i32 + c) as usize] =
                Some(vertical || horizontal || square);
        }
    }
}

fn place_format_info(modules: &mut [Vec<Option<bool>>], test: bool, pattern: usize) {
    let count = modules.len();
    let bits = bch_format_info((LEVEL_L << 3) | pattern as u32);
    for i in 0..15 {
        let m = Some(!test && (bits >> i) & 1 == 1);
        let row = if i < 6 {
            i
        } else if i < 8 {
            i + 1
        } else {
            count - 15 + i
        };
        modules[row][8] = m;
        let col = if i < 8 {
            count - i - 1
        } else if i < 9 {
            15 - i
        } else {
            15 - i - 1
        };
        modules[8][col] = m;
    }
    modules[count - 8][8] = Some(!test);
}

fn place_version_info(modules: &mut [Vec<Option<bool>>], test: bool, version: usize) {
    let count = modules.len();
    let bits = bch_version(version as u32);
    for i in 0..18 {
        let m = Some(!test && (bits >> i) & 1 == 1);
        modules[i / 3][i % 3 + count - 8 - 3] = m;
        modules[i % 3 + count - 8 - 3][i / 3] = m;
    }
}

fn map_data(modules: &mut [Vec<Option<bool>>], data: &[Option<i32>], pattern: usize) {
    let count = modules.len() as i32;
    let mut inc = -1i32;
    let mut row = count - 1;
    let mut bit_index = 7i32;
    let mut byte_index = 0usize;
    let mut col = count - 1;
    while col >= 1 {
        let column = if col <= 6 { col - 1 } else { col };
        loop {
            for c in 0..2 {
                let x = (column - c) as usize;
                if modules[row as usize][x].is_none() {
                    let mut dark = false;
                    if let Some(Some(byte)) = data.get(byte_index) {
                        dark = (byte >> bit_index) & 1 == 1;
                    }
                    if mask(pattern, row as usize, x) {
                        dark = !dark;
                    }
                    modules[row as usize][x] = Some(dark);
                    bit_index -= 1;
                    if bit_index == -1 {
                        byte_index += 1;
                        bit_index = 7;
                    }
                }
            }
            row += inc;
            if row < 0 || count <= row {
                row -= inc;
                inc = -inc;
                break;
            }
        }
        col -= 2;
    }
}

fn create_data(version: usize, data: &str, mode: Mode) -> Result<Vec<Option<i32>>, String> {
    let table = RS_BLOCK_TABLE[(version - 1) * 4];
    let mut blocks = Vec::new();
    for group in table.chunks(3) {
        for _ in 0..group[0] {
            blocks.push((group[1], group[2]));
        }
    }
    let max_bits: usize = blocks.iter().map(|(_, data)| data).sum::<usize>() * 8;
    let mut buffer = BitBuffer {
        buffer: Vec::new(),
        length: 0,
    };
    buffer.put(mode.bits(), 4);
    match mode {
        Mode::Number => {
            let chars: Vec<char> = data.chars().collect();
            buffer.put(chars.len() as u32, mode.length_bits(version));
            for chunk in chars.chunks(3) {
                let text: String = chunk.iter().collect();
                let bits = [0, 4, 7, 10][chunk.len()];
                buffer.put(text.parse().unwrap_or(0), bits);
            }
        }
        Mode::AlphaNumeric => {
            let values: Vec<u32> = data
                .chars()
                .map(|c| ALPHANUMERIC.find(c).unwrap_or(0) as u32)
                .collect();
            buffer.put(values.len() as u32, mode.length_bits(version));
            for pair in values.chunks(2) {
                match pair {
                    [a, b] => buffer.put(a * 45 + b, 11),
                    [a] => buffer.put(*a, 6),
                    _ => {}
                }
            }
        }
        Mode::Byte => {
            buffer.put(data.len() as u32, mode.length_bits(version));
            for b in data.bytes() {
                buffer.put(u32::from(b), 8);
            }
        }
    }
    if buffer.length + 4 <= max_bits {
        buffer.put(0, 4);
    }
    if buffer.length > max_bits {
        return Err(format!(
            "code length overflow. ({}>{max_bits}). (Try a larger size!)",
            buffer.length
        ));
    }
    while buffer.length % 8 != 0 {
        buffer.put_bit(false);
    }
    while buffer.length < max_bits {
        buffer.put(0xec, 8);
        if buffer.length < max_bits {
            buffer.put(0x11, 8);
        }
    }
    let g = Galois::new();
    let mut offset = 0;
    let mut dc = Vec::new();
    let mut ec = Vec::new();
    for (total, data_count) in &blocks {
        let block: Vec<Option<i32>> = buffer.buffer[offset..offset + data_count]
            .iter()
            .map(|b| Some(i32::from(*b)))
            .collect();
        offset += data_count;
        let ec_count = total - data_count;
        let mut rs = Polynomial::new(&[Some(1)], 0);
        for i in 0..ec_count {
            rs = rs.multiply(&Polynomial::new(&[Some(1), Some(g.gexp(i as i32))], 0), &g)?;
        }
        let raw = Polynomial::new(&block, rs.len() - 1);
        let modulo = raw.modulo(&rs, &g)?;
        let size = rs.len() - 1;
        let ec_block: Vec<Option<i32>> = (0..size)
            .map(|i| {
                let index = i as i64 + modulo.len() as i64 - size as i64;
                if index >= 0 {
                    modulo.0[index as usize]
                } else {
                    Some(0)
                }
            })
            .collect();
        dc.push(block);
        ec.push(ec_block);
    }
    let mut out = Vec::new();
    for list in [&dc, &ec] {
        let longest = list.iter().map(Vec::len).max().unwrap_or(0);
        for i in 0..longest {
            for block in list.iter() {
                if let Some(value) = block.get(i) {
                    out.push(*value);
                }
            }
        }
    }
    Ok(out)
}

/// `QRUtil.get_lost_points`.
fn lost_points(modules: &[Vec<Option<bool>>]) -> i64 {
    let n = modules.len();
    let at = |r: usize, c: usize| modules[r][c] == Some(true);
    let mut points = 0i64;
    for row in 0..n {
        for col in 0..n {
            let mut same = 0;
            let dark = modules[row][col];
            for r in -1i32..=1 {
                let rr = row as i32 + r;
                if rr < 0 || rr >= n as i32 {
                    continue;
                }
                for c in -1i32..=1 {
                    let cc = col as i32 + c;
                    if cc < 0 || cc >= n as i32 || (r == 0 && c == 0) {
                        continue;
                    }
                    if dark == modules[rr as usize][cc as usize] {
                        same += 1;
                    }
                }
            }
            if same > 5 {
                points += 3 + same - 5;
            }
        }
    }
    for row in 0..n - 1 {
        for col in 0..n - 1 {
            let count = [
                at(row, col),
                at(row + 1, col),
                at(row, col + 1),
                at(row + 1, col + 1),
            ]
            .iter()
            .filter(|d| **d)
            .count();
            if count == 0 || count == 4 {
                points += 3;
            }
        }
    }
    let pattern = |get: &dyn Fn(usize) -> bool| {
        get(0) && !get(1) && get(2) && get(3) && get(4) && !get(5) && get(6)
    };
    for row in 0..n {
        for col in 0..n.saturating_sub(6) {
            if pattern(&|k| at(row, col + k)) {
                points += 40;
            }
        }
    }
    for col in 0..n {
        for row in 0..n.saturating_sub(6) {
            if pattern(&|k| at(row + k, col)) {
                points += 40;
            }
        }
    }
    let dark = modules
        .iter()
        .flatten()
        .filter(|m| **m == Some(true))
        .count() as i64;
    let ratio = dark / (n * n) as i64;
    points + (100 * ratio - 50).abs() / 5 * 10
}
