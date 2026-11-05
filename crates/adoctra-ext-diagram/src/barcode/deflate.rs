//! zlib 1.3.1's `deflate` at the default level (6) with Ruby's
//! `Zlib::Deflate.deflate` settings (window bits 15, memory level 8,
//! default strategy), byte for byte: the zlib header, `deflate_slow` with
//! lazy matching and hash chains, the window with its sliding and
//! `high_water` zeroing, trees.c's Huffman trees and block choice, Adler-32.
//! chunky_png compresses PNG image data with it.
//!
//! This is an altered version of zlib (a Rust translation of deflate.c and
//! trees.c), not the original software; zlib license in
//! `LICENSE-third-party`.

const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;
const MIN_LOOKAHEAD: usize = MAX_MATCH + MIN_MATCH + 1;
const W_BITS: usize = 15;
const W_SIZE: usize = 1 << W_BITS;
const W_MASK: usize = W_SIZE - 1;
const WINDOW_SIZE: usize = 2 * W_SIZE;
const MAX_DIST: usize = W_SIZE - MIN_LOOKAHEAD;
const MEM_LEVEL: usize = 8;
const HASH_BITS: usize = MEM_LEVEL + 7;
const HASH_SIZE: usize = 1 << HASH_BITS;
const HASH_MASK: usize = HASH_SIZE - 1;
const HASH_SHIFT: usize = HASH_BITS.div_ceil(MIN_MATCH);
const LIT_BUFSIZE: usize = 1 << (MEM_LEVEL + 6);
const SYM_END: usize = (LIT_BUFSIZE - 1) * 3;
const WIN_INIT: usize = MAX_MATCH;
const TOO_FAR: usize = 4096;
// Level 6 (`configuration_table`).
const GOOD_MATCH: usize = 8;
const MAX_LAZY: usize = 16;
const NICE_MATCH: usize = 128;
const MAX_CHAIN: usize = 128;

const LENGTH_CODES: usize = 29;
const LITERALS: usize = 256;
const L_CODES: usize = LITERALS + 1 + LENGTH_CODES;
const D_CODES: usize = 30;
const BL_CODES: usize = 19;
const HEAP_SIZE: usize = 2 * L_CODES + 1;
const MAX_BITS: usize = 15;
const MAX_BL_BITS: usize = 7;
const END_BLOCK: usize = 256;
const REP_3_6: usize = 16;
const REPZ_3_10: usize = 17;
const REPZ_11_138: usize = 18;

const EXTRA_LBITS: [usize; LENGTH_CODES] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const EXTRA_DBITS: [usize; D_CODES] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
const EXTRA_BLBITS: [usize; BL_CODES] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 3, 7];
const BL_ORDER: [usize; BL_CODES] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// A tree node: `fc` is the frequency, then the code; `dl` the parent,
/// then the bit length (trees.c's unions).
#[derive(Clone, Copy, Default)]
struct Node {
    fc: u16,
    dl: u16,
}

/// trees.c's static tables (`tr_static_init`).
struct Static {
    ltree: Vec<Node>,
    dtree: Vec<Node>,
    length_code: [u8; 256],
    dist_code: [u8; 512],
    base_length: [usize; LENGTH_CODES],
    base_dist: [usize; D_CODES],
}

fn bi_reverse(mut code: u32, len: usize) -> u32 {
    let mut res = 0;
    for _ in 0..len {
        res |= code & 1;
        code >>= 1;
        res <<= 1;
    }
    res >> 1
}

/// `gen_codes`.
fn gen_codes(tree: &mut [Node], max_code: isize, bl_count: &[u16; MAX_BITS + 1]) {
    let mut next_code = [0u16; MAX_BITS + 1];
    let mut code: u32 = 0;
    for bits in 1..=MAX_BITS {
        code = (code + u32::from(bl_count[bits - 1])) << 1;
        next_code[bits] = code as u16;
    }
    for n in 0..=max_code {
        let len = tree[n as usize].dl as usize;
        if len == 0 {
            continue;
        }
        tree[n as usize].fc = bi_reverse(u32::from(next_code[len]), len) as u16;
        next_code[len] = next_code[len].wrapping_add(1);
    }
}

impl Static {
    fn new() -> Self {
        let mut base_length = [0; LENGTH_CODES];
        let mut length_code = [0u8; 256];
        let mut length = 0;
        for code in 0..LENGTH_CODES - 1 {
            base_length[code] = length;
            for _ in 0..(1 << EXTRA_LBITS[code]) {
                length_code[length] = code as u8;
                length += 1;
            }
        }
        length_code[length - 1] = (LENGTH_CODES - 1) as u8;
        let mut base_dist = [0; D_CODES];
        let mut dist_code = [0u8; 512];
        let mut dist = 0;
        for code in 0..16 {
            base_dist[code] = dist;
            for _ in 0..(1 << EXTRA_DBITS[code]) {
                dist_code[dist] = code as u8;
                dist += 1;
            }
        }
        dist >>= 7;
        for code in 16..D_CODES {
            base_dist[code] = dist << 7;
            for _ in 0..(1 << (EXTRA_DBITS[code] - 7)) {
                dist_code[256 + dist] = code as u8;
                dist += 1;
            }
        }
        let mut bl_count = [0u16; MAX_BITS + 1];
        let mut ltree = vec![Node::default(); L_CODES + 2];
        for (n, node) in ltree.iter_mut().enumerate() {
            let len = match n {
                0..=143 => 8,
                144..=255 => 9,
                256..=279 => 7,
                _ => 8,
            };
            node.dl = len;
            bl_count[len as usize] += 1;
        }
        gen_codes(&mut ltree, (L_CODES + 1) as isize, &bl_count);
        let dtree = (0..D_CODES)
            .map(|n| Node {
                fc: bi_reverse(n as u32, 5) as u16,
                dl: 5,
            })
            .collect();
        Static {
            ltree,
            dtree,
            length_code,
            dist_code,
            base_length,
            base_dist,
        }
    }

    fn d_code(&self, dist: usize) -> usize {
        if dist < 256 {
            self.dist_code[dist] as usize
        } else {
            self.dist_code[256 + (dist >> 7)] as usize
        }
    }
}

/// Which tree `build_tree` works on, with its static description.
#[derive(Clone, Copy)]
enum Which {
    Literal,
    Distance,
    BitLength,
}

struct State<'a> {
    input: &'a [u8],
    next_in: usize,
    adler: (u32, u32),
    window: Vec<u8>,
    prev: Vec<u16>,
    head: Vec<u16>,
    ins_h: usize,
    high_water: usize,
    block_start: isize,
    strstart: usize,
    match_start: usize,
    lookahead: usize,
    prev_length: usize,
    match_length: usize,
    match_available: bool,
    prev_match: usize,
    insert: usize,
    dyn_ltree: Vec<Node>,
    dyn_dtree: Vec<Node>,
    bl_tree: Vec<Node>,
    l_max_code: isize,
    d_max_code: isize,
    bl_max_code: isize,
    bl_count: [u16; MAX_BITS + 1],
    heap: [usize; HEAP_SIZE],
    heap_len: usize,
    heap_max: usize,
    depth: [u8; HEAP_SIZE],
    sym_buf: Vec<u8>,
    opt_len: i64,
    static_len: i64,
    out: Vec<u8>,
    bi_buf: u32,
    bi_valid: usize,
    tables: Static,
}

/// `Zlib::Deflate.deflate(data)`: the zlib stream.
pub(crate) fn deflate(data: &[u8]) -> Vec<u8> {
    let mut s = State {
        input: data,
        next_in: 0,
        adler: (1, 0),
        window: vec![0; WINDOW_SIZE],
        prev: vec![0; W_SIZE],
        head: vec![0; HASH_SIZE],
        ins_h: 0,
        high_water: 0,
        block_start: 0,
        strstart: 0,
        match_start: 0,
        lookahead: 0,
        prev_length: MIN_MATCH - 1,
        match_length: MIN_MATCH - 1,
        match_available: false,
        prev_match: 0,
        insert: 0,
        dyn_ltree: vec![Node::default(); HEAP_SIZE],
        dyn_dtree: vec![Node::default(); 2 * D_CODES + 1],
        bl_tree: vec![Node::default(); 2 * BL_CODES + 1],
        l_max_code: 0,
        d_max_code: 0,
        bl_max_code: 0,
        bl_count: [0; MAX_BITS + 1],
        heap: [0; HEAP_SIZE],
        heap_len: 0,
        heap_max: 0,
        depth: [0; HEAP_SIZE],
        sym_buf: Vec::with_capacity(SYM_END),
        opt_len: 0,
        static_len: 0,
        out: Vec::new(),
        bi_buf: 0,
        bi_valid: 0,
        tables: Static::new(),
    };
    s.init_block();
    // The header: deflate, 32K window, level 6 (`level_flags` 2).
    let mut header = (8 + ((W_BITS - 8) << 4)) << 8 | (2 << 6);
    header += 31 - (header % 31);
    s.out.extend([(header >> 8) as u8, header as u8]);
    s.deflate_slow();
    let adler = (s.adler.1 << 16) | s.adler.0;
    s.out.extend(adler.to_be_bytes());
    s.out
}

impl State<'_> {
    fn update_hash(&mut self, c: u8) {
        self.ins_h = ((self.ins_h << HASH_SHIFT) ^ usize::from(c)) & HASH_MASK;
    }

    /// `INSERT_STRING`: the previous head of the string's hash chain.
    fn insert_string(&mut self, at: usize) -> usize {
        self.update_hash(self.window[at + MIN_MATCH - 1]);
        let head = self.head[self.ins_h];
        self.prev[at & W_MASK] = head;
        self.head[self.ins_h] = at as u16;
        usize::from(head)
    }

    /// `read_buf` with Adler-32.
    fn read_buf(&mut self, at: usize, size: usize) -> usize {
        let len = (self.input.len() - self.next_in).min(size);
        let chunk = &self.input[self.next_in..self.next_in + len];
        self.window[at..at + len].copy_from_slice(chunk);
        let (mut a, mut b) = self.adler;
        for byte in chunk {
            a = (a + u32::from(*byte)) % 65521;
            b = (b + a) % 65521;
        }
        self.adler = (a, b);
        self.next_in += len;
        len
    }

    fn avail_in(&self) -> usize {
        self.input.len() - self.next_in
    }

    fn slide_hash(&mut self) {
        for entry in self.head.iter_mut().chain(self.prev.iter_mut()) {
            let m = usize::from(*entry);
            *entry = if m >= W_SIZE { (m - W_SIZE) as u16 } else { 0 };
        }
    }

    fn fill_window(&mut self) {
        loop {
            let mut more = WINDOW_SIZE - self.lookahead - self.strstart;
            if self.strstart >= W_SIZE + MAX_DIST {
                self.window.copy_within(W_SIZE..W_SIZE + W_SIZE - more, 0);
                self.match_start = self.match_start.wrapping_sub(W_SIZE);
                self.strstart -= W_SIZE;
                self.block_start -= W_SIZE as isize;
                if self.insert > self.strstart {
                    self.insert = self.strstart;
                }
                self.slide_hash();
                more += W_SIZE;
            }
            if self.avail_in() == 0 {
                break;
            }
            let n = self.read_buf(self.strstart + self.lookahead, more);
            self.lookahead += n;
            if self.lookahead + self.insert >= MIN_MATCH {
                let mut at = self.strstart - self.insert;
                self.ins_h = usize::from(self.window[at]);
                self.update_hash(self.window[at + 1]);
                while self.insert > 0 {
                    self.update_hash(self.window[at + MIN_MATCH - 1]);
                    self.prev[at & W_MASK] = self.head[self.ins_h];
                    self.head[self.ins_h] = at as u16;
                    at += 1;
                    self.insert -= 1;
                    if self.lookahead + self.insert < MIN_MATCH {
                        break;
                    }
                }
            }
            if !(self.lookahead < MIN_LOOKAHEAD && self.avail_in() != 0) {
                break;
            }
        }
        if self.high_water < WINDOW_SIZE {
            let curr = self.strstart + self.lookahead;
            if self.high_water < curr {
                let init = (WINDOW_SIZE - curr).min(WIN_INIT);
                self.window[curr..curr + init].fill(0);
                self.high_water = curr + init;
            } else if self.high_water < curr + WIN_INIT {
                let init = (curr + WIN_INIT - self.high_water).min(WINDOW_SIZE - self.high_water);
                self.window[self.high_water..self.high_water + init].fill(0);
                self.high_water += init;
            }
        }
    }

    /// `longest_match` (the portable version, which may compare bytes past
    /// the data that the window holds).
    fn longest_match(&mut self, mut cur_match: usize) -> usize {
        let mut chain_length = MAX_CHAIN;
        let scan = self.strstart;
        let mut best_len = self.prev_length;
        let mut nice_match = NICE_MATCH;
        let limit = self.strstart.saturating_sub(MAX_DIST);
        let strend = self.strstart + MAX_MATCH;
        let w = &self.window;
        let mut scan_end1 = w[scan + best_len - 1];
        let mut scan_end = w[scan + best_len];
        if self.prev_length >= GOOD_MATCH {
            chain_length >>= 2;
        }
        if nice_match > self.lookahead {
            nice_match = self.lookahead;
        }
        loop {
            let m = cur_match;
            if w[m + best_len] == scan_end
                && w[m + best_len - 1] == scan_end1
                && w[m] == w[scan]
                && w[m + 1] == w[scan + 1]
            {
                // `scan += 2, match++` then compare from `*++scan`.
                let mut s = scan + 2;
                let mut mm = m + 2;
                loop {
                    let mut matched = true;
                    for _ in 0..8 {
                        s += 1;
                        mm += 1;
                        if w[s] != w[mm] {
                            matched = false;
                            break;
                        }
                    }
                    if !matched || s >= strend {
                        break;
                    }
                }
                let len = MAX_MATCH - (strend - s);
                if len > best_len {
                    self.match_start = cur_match;
                    best_len = len;
                    if len >= nice_match {
                        break;
                    }
                    scan_end1 = w[scan + best_len - 1];
                    scan_end = w[scan + best_len];
                }
            }
            cur_match = usize::from(self.prev[cur_match & W_MASK]);
            if cur_match <= limit {
                break;
            }
            chain_length -= 1;
            if chain_length == 0 {
                break;
            }
        }
        best_len.min(self.lookahead)
    }

    fn tally_lit(&mut self, c: u8) -> bool {
        self.sym_buf.extend([0, 0, c]);
        self.dyn_ltree[usize::from(c)].fc += 1;
        self.sym_buf.len() == SYM_END
    }

    fn tally_dist(&mut self, distance: usize, length: usize) -> bool {
        self.sym_buf
            .extend([distance as u8, (distance >> 8) as u8, length as u8]);
        let dist = distance - 1;
        let lc = self.tables.length_code[length] as usize;
        self.dyn_ltree[lc + LITERALS + 1].fc += 1;
        let dc = self.tables.d_code(dist);
        self.dyn_dtree[dc].fc += 1;
        self.sym_buf.len() == SYM_END
    }

    fn flush_block(&mut self, last: bool) {
        let stored: Option<Vec<u8>> = (self.block_start >= 0)
            .then(|| self.window[self.block_start as usize..self.strstart].to_vec());
        let stored_len = (self.strstart as isize - self.block_start) as usize;
        self.tr_flush_block(stored.as_deref(), stored_len, last);
        self.block_start = self.strstart as isize;
    }

    fn deflate_slow(&mut self) {
        loop {
            if self.lookahead < MIN_LOOKAHEAD {
                self.fill_window();
                if self.lookahead == 0 {
                    break;
                }
            }
            let mut hash_head = 0;
            if self.lookahead >= MIN_MATCH {
                hash_head = self.insert_string(self.strstart);
            }
            self.prev_length = self.match_length;
            self.prev_match = self.match_start;
            self.match_length = MIN_MATCH - 1;
            if hash_head != 0
                && self.prev_length < MAX_LAZY
                && self.strstart - hash_head <= MAX_DIST
            {
                self.match_length = self.longest_match(hash_head);
                if self.match_length <= 5
                    && self.match_length == MIN_MATCH
                    && self.strstart - self.match_start > TOO_FAR
                {
                    self.match_length = MIN_MATCH - 1;
                }
            }
            if self.prev_length >= MIN_MATCH && self.match_length <= self.prev_length {
                let max_insert = self.strstart + self.lookahead - MIN_MATCH;
                let bflush = self.tally_dist(
                    self.strstart - 1 - self.prev_match,
                    self.prev_length - MIN_MATCH,
                );
                self.lookahead -= self.prev_length - 1;
                self.prev_length -= 2;
                loop {
                    self.strstart += 1;
                    if self.strstart <= max_insert {
                        self.insert_string(self.strstart);
                    }
                    self.prev_length -= 1;
                    if self.prev_length == 0 {
                        break;
                    }
                }
                self.match_available = false;
                self.match_length = MIN_MATCH - 1;
                self.strstart += 1;
                if bflush {
                    self.flush_block(false);
                }
            } else if self.match_available {
                let bflush = self.tally_lit(self.window[self.strstart - 1]);
                if bflush {
                    self.flush_block(false);
                }
                self.strstart += 1;
                self.lookahead -= 1;
            } else {
                self.match_available = true;
                self.strstart += 1;
                self.lookahead -= 1;
            }
        }
        if self.match_available {
            self.tally_lit(self.window[self.strstart - 1]);
            self.match_available = false;
        }
        self.insert = self.strstart.min(MIN_MATCH - 1);
        self.flush_block(true);
    }

    // ---- trees.c ----

    fn init_block(&mut self) {
        for node in self.dyn_ltree.iter_mut().take(L_CODES) {
            node.fc = 0;
        }
        for node in self.dyn_dtree.iter_mut().take(D_CODES) {
            node.fc = 0;
        }
        for node in self.bl_tree.iter_mut().take(BL_CODES) {
            node.fc = 0;
        }
        self.dyn_ltree[END_BLOCK].fc = 1;
        self.opt_len = 0;
        self.static_len = 0;
        self.sym_buf.clear();
    }

    fn send_bits(&mut self, value: u32, length: usize) {
        self.bi_buf |= value << self.bi_valid;
        self.bi_valid += length;
        while self.bi_valid >= 8 {
            self.out.push(self.bi_buf as u8);
            self.bi_buf >>= 8;
            self.bi_valid -= 8;
        }
    }

    fn bi_windup(&mut self) {
        if self.bi_valid > 0 {
            self.out.push(self.bi_buf as u8);
        }
        self.bi_buf = 0;
        self.bi_valid = 0;
    }

    fn tree(&mut self, which: Which) -> &mut Vec<Node> {
        match which {
            Which::Literal => &mut self.dyn_ltree,
            Which::Distance => &mut self.dyn_dtree,
            Which::BitLength => &mut self.bl_tree,
        }
    }

    fn smaller(tree: &[Node], depth: &[u8], n: usize, m: usize) -> bool {
        tree[n].fc < tree[m].fc || (tree[n].fc == tree[m].fc && depth[n] <= depth[m])
    }

    fn pqdownheap(&mut self, which: Which, mut k: usize) {
        let v = self.heap[k];
        let mut j = k << 1;
        let heap_len = self.heap_len;
        let tree = match which {
            Which::Literal => &self.dyn_ltree,
            Which::Distance => &self.dyn_dtree,
            Which::BitLength => &self.bl_tree,
        };
        while j <= heap_len {
            if j < heap_len && Self::smaller(tree, &self.depth, self.heap[j + 1], self.heap[j]) {
                j += 1;
            }
            if Self::smaller(tree, &self.depth, v, self.heap[j]) {
                break;
            }
            self.heap[k] = self.heap[j];
            k = j;
            j <<= 1;
        }
        self.heap[k] = v;
    }

    /// The static description: (static tree lengths, extra bits, extra
    /// base, elements, maximum length).
    fn describe(which: Which) -> (Option<bool>, &'static [usize], usize, usize, usize) {
        match which {
            Which::Literal => (Some(true), &EXTRA_LBITS, LITERALS + 1, L_CODES, MAX_BITS),
            Which::Distance => (Some(false), &EXTRA_DBITS, 0, D_CODES, MAX_BITS),
            Which::BitLength => (None, &EXTRA_BLBITS, 0, BL_CODES, MAX_BL_BITS),
        }
    }

    fn static_len_of(&self, which: Which, n: usize) -> Option<i64> {
        match which {
            Which::Literal => Some(i64::from(self.tables.ltree[n].dl)),
            Which::Distance => Some(i64::from(self.tables.dtree[n].dl)),
            Which::BitLength => None,
        }
    }

    fn build_tree(&mut self, which: Which) {
        let (_, _, _, elems, _) = Self::describe(which);
        let mut max_code: isize = -1;
        self.heap_len = 0;
        self.heap_max = HEAP_SIZE;
        for n in 0..elems {
            if self.tree(which)[n].fc != 0 {
                self.heap_len += 1;
                self.heap[self.heap_len] = n;
                max_code = n as isize;
                self.depth[n] = 0;
            } else {
                self.tree(which)[n].dl = 0;
            }
        }
        while self.heap_len < 2 {
            let node = if max_code < 2 {
                max_code += 1;
                max_code as usize
            } else {
                0
            };
            self.heap_len += 1;
            self.heap[self.heap_len] = node;
            self.tree(which)[node].fc = 1;
            self.depth[node] = 0;
            self.opt_len -= 1;
            if let Some(len) = self.static_len_of(which, node) {
                self.static_len -= len;
            }
        }
        match which {
            Which::Literal => self.l_max_code = max_code,
            Which::Distance => self.d_max_code = max_code,
            Which::BitLength => self.bl_max_code = max_code,
        }
        let mut n = self.heap_len / 2;
        while n >= 1 {
            self.pqdownheap(which, n);
            n -= 1;
        }
        let mut node = elems;
        loop {
            let n = self.heap[1];
            self.heap[1] = self.heap[self.heap_len];
            self.heap_len -= 1;
            self.pqdownheap(which, 1);
            let m = self.heap[1];
            self.heap_max -= 1;
            self.heap[self.heap_max] = n;
            self.heap_max -= 1;
            self.heap[self.heap_max] = m;
            let (fn_, fm) = (self.tree(which)[n].fc, self.tree(which)[m].fc);
            self.tree(which)[node].fc = fn_.wrapping_add(fm);
            self.depth[node] = self.depth[n].max(self.depth[m]) + 1;
            self.tree(which)[n].dl = node as u16;
            self.tree(which)[m].dl = node as u16;
            self.heap[1] = node;
            node += 1;
            self.pqdownheap(which, 1);
            if self.heap_len < 2 {
                break;
            }
        }
        self.heap_max -= 1;
        self.heap[self.heap_max] = self.heap[1];
        self.gen_bitlen(which, max_code);
        let bl_count = self.bl_count;
        gen_codes(self.tree(which), max_code, &bl_count);
    }

    fn gen_bitlen(&mut self, which: Which, max_code: isize) {
        let (_, extra, base, _, max_length) = Self::describe(which);
        let mut overflow = 0;
        self.bl_count = [0; MAX_BITS + 1];
        let root = self.heap[self.heap_max];
        self.tree(which)[root].dl = 0;
        let mut h = self.heap_max + 1;
        while h < HEAP_SIZE {
            let n = self.heap[h];
            let dad = self.tree(which)[n].dl as usize;
            let mut bits = self.tree(which)[dad].dl as usize + 1;
            if bits > max_length {
                bits = max_length;
                overflow += 1;
            }
            self.tree(which)[n].dl = bits as u16;
            h += 1;
            if n as isize > max_code {
                continue;
            }
            self.bl_count[bits] += 1;
            let xbits = if n >= base { extra[n - base] } else { 0 };
            let f = i64::from(self.tree(which)[n].fc);
            self.opt_len += f * (bits + xbits) as i64;
            if let Some(len) = self.static_len_of(which, n) {
                self.static_len += f * (len + xbits as i64);
            }
        }
        if overflow == 0 {
            return;
        }
        loop {
            let mut bits = max_length - 1;
            while self.bl_count[bits] == 0 {
                bits -= 1;
            }
            self.bl_count[bits] -= 1;
            self.bl_count[bits + 1] += 2;
            self.bl_count[max_length] -= 1;
            overflow -= 2;
            if overflow <= 0 {
                break;
            }
        }
        let mut h = HEAP_SIZE;
        for bits in (1..=max_length).rev() {
            let mut n = self.bl_count[bits];
            while n != 0 {
                h -= 1;
                let m = self.heap[h];
                if m as isize > max_code {
                    continue;
                }
                let len = self.tree(which)[m].dl as usize;
                if len != bits {
                    let f = i64::from(self.tree(which)[m].fc);
                    self.opt_len += (bits as i64 - len as i64) * f;
                    self.tree(which)[m].dl = bits as u16;
                }
                n -= 1;
            }
        }
    }

    fn scan_tree(&mut self, which: Which, max_code: isize) {
        let mut prevlen: isize = -1;
        let mut nextlen = self.tree(which)[0].dl as isize;
        let mut count = 0;
        let (mut max_count, mut min_count) = if nextlen == 0 { (138, 3) } else { (7, 4) };
        self.tree(which)[(max_code + 1) as usize].dl = 0xffff;
        for n in 0..=max_code {
            let curlen = nextlen;
            nextlen = self.tree(which)[(n + 1) as usize].dl as isize;
            count += 1;
            if count < max_count && curlen == nextlen {
                continue;
            } else if count < min_count {
                self.bl_tree[curlen as usize].fc += count as u16;
            } else if curlen != 0 {
                if curlen != prevlen {
                    self.bl_tree[curlen as usize].fc += 1;
                }
                self.bl_tree[REP_3_6].fc += 1;
            } else if count <= 10 {
                self.bl_tree[REPZ_3_10].fc += 1;
            } else {
                self.bl_tree[REPZ_11_138].fc += 1;
            }
            count = 0;
            prevlen = curlen;
            (max_count, min_count) = if nextlen == 0 {
                (138, 3)
            } else if curlen == nextlen {
                (6, 3)
            } else {
                (7, 4)
            };
        }
    }

    fn send_code(&mut self, c: usize, which: Which) {
        let node = self.tree(which)[c];
        self.send_bits(u32::from(node.fc), node.dl as usize);
    }

    fn send_tree(&mut self, which: Which, max_code: isize) {
        let mut prevlen: isize = -1;
        let mut nextlen = self.tree(which)[0].dl as isize;
        let mut count: isize = 0;
        let (mut max_count, mut min_count) = if nextlen == 0 { (138, 3) } else { (7, 4) };
        for n in 0..=max_code {
            let curlen = nextlen;
            nextlen = self.tree(which)[(n + 1) as usize].dl as isize;
            count += 1;
            if count < max_count && curlen == nextlen {
                continue;
            } else if count < min_count {
                loop {
                    self.send_code(curlen as usize, Which::BitLength);
                    count -= 1;
                    if count == 0 {
                        break;
                    }
                }
            } else if curlen != 0 {
                if curlen != prevlen {
                    self.send_code(curlen as usize, Which::BitLength);
                    count -= 1;
                }
                self.send_code(REP_3_6, Which::BitLength);
                self.send_bits((count - 3) as u32, 2);
            } else if count <= 10 {
                self.send_code(REPZ_3_10, Which::BitLength);
                self.send_bits((count - 3) as u32, 3);
            } else {
                self.send_code(REPZ_11_138, Which::BitLength);
                self.send_bits((count - 11) as u32, 7);
            }
            count = 0;
            prevlen = curlen;
            (max_count, min_count) = if nextlen == 0 {
                (138, 3)
            } else if curlen == nextlen {
                (6, 3)
            } else {
                (7, 4)
            };
        }
    }

    fn build_bl_tree(&mut self) -> usize {
        self.scan_tree(Which::Literal, self.l_max_code);
        self.scan_tree(Which::Distance, self.d_max_code);
        self.build_tree(Which::BitLength);
        let mut max_blindex = BL_CODES - 1;
        while max_blindex >= 3 {
            if self.bl_tree[BL_ORDER[max_blindex]].dl != 0 {
                break;
            }
            max_blindex -= 1;
        }
        self.opt_len += 3 * (max_blindex as i64 + 1) + 5 + 5 + 4;
        max_blindex
    }

    fn send_all_trees(&mut self, lcodes: usize, dcodes: usize, blcodes: usize) {
        self.send_bits((lcodes - 257) as u32, 5);
        self.send_bits((dcodes - 1) as u32, 5);
        self.send_bits((blcodes - 4) as u32, 4);
        for code in BL_ORDER.iter().take(blcodes) {
            let len = self.bl_tree[*code].dl;
            self.send_bits(u32::from(len), 3);
        }
        self.send_tree(Which::Literal, lcodes as isize - 1);
        self.send_tree(Which::Distance, dcodes as isize - 1);
    }

    fn compress_block(&mut self, fixed: bool) {
        let symbols = std::mem::take(&mut self.sym_buf);
        let code = |s: &Self, tree: Which, c: usize| -> (u32, usize) {
            let node = if fixed {
                match tree {
                    Which::Literal => s.tables.ltree[c],
                    _ => s.tables.dtree[c],
                }
            } else {
                match tree {
                    Which::Literal => s.dyn_ltree[c],
                    _ => s.dyn_dtree[c],
                }
            };
            (u32::from(node.fc), node.dl as usize)
        };
        for sym in symbols.chunks(3) {
            let mut dist = usize::from(sym[0]) | usize::from(sym[1]) << 8;
            let mut lc = usize::from(sym[2]);
            if dist == 0 {
                let (c, l) = code(self, Which::Literal, lc);
                self.send_bits(c, l);
            } else {
                let lcode = self.tables.length_code[lc] as usize;
                let (c, l) = code(self, Which::Literal, lcode + LITERALS + 1);
                self.send_bits(c, l);
                let extra = EXTRA_LBITS[lcode];
                if extra != 0 {
                    lc -= self.tables.base_length[lcode];
                    self.send_bits(lc as u32, extra);
                }
                dist -= 1;
                let dcode = self.tables.d_code(dist);
                let (c, l) = code(self, Which::Distance, dcode);
                self.send_bits(c, l);
                let extra = EXTRA_DBITS[dcode];
                if extra != 0 {
                    dist -= self.tables.base_dist[dcode];
                    self.send_bits(dist as u32, extra);
                }
            }
        }
        let (c, l) = code(self, Which::Literal, END_BLOCK);
        self.send_bits(c, l);
        self.sym_buf = symbols;
    }

    fn tr_flush_block(&mut self, buf: Option<&[u8]>, stored_len: usize, last: bool) {
        self.build_tree(Which::Literal);
        self.build_tree(Which::Distance);
        let max_blindex = self.build_bl_tree();
        let mut opt_lenb = ((self.opt_len + 3 + 7) >> 3) as usize;
        let static_lenb = ((self.static_len + 3 + 7) >> 3) as usize;
        if static_lenb <= opt_lenb {
            opt_lenb = static_lenb;
        }
        let last_bit = u32::from(last);
        match buf {
            Some(buf) if stored_len + 4 <= opt_lenb => {
                self.send_bits(last_bit, 3);
                self.bi_windup();
                self.out.extend((stored_len as u16).to_le_bytes());
                self.out.extend((!(stored_len as u16)).to_le_bytes());
                self.out.extend_from_slice(buf);
            }
            _ if static_lenb == opt_lenb => {
                self.send_bits((1 << 1) + last_bit, 3);
                self.compress_block(true);
            }
            _ => {
                self.send_bits((2 << 1) + last_bit, 3);
                let (l, d) = (self.l_max_code, self.d_max_code);
                self.send_all_trees(l as usize + 1, d as usize + 1, max_blindex + 1);
                self.compress_block(false);
            }
        }
        self.init_block();
        if last {
            self.bi_windup();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_zlib_byte_for_byte() {
        // `Zlib::Deflate.deflate(s)` in Ruby (zlib 1.3.1).
        assert_eq!(
            deflate(b""),
            [0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01]
        );
        assert_eq!(
            deflate(b"hello hello hello"),
            [
                0x78, 0x9c, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x57, 0xc8, 0x40, 0x90, 0x00, 0x3a, 0x2e,
                0x06, 0x7d
            ]
        );
    }
}
