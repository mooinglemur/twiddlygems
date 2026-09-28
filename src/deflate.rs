//! A gzip encoder, for the server to compress the site it carries.
//!
//! Here rather than taken from a crate because this repository has no
//! dependencies and is not about to grow one for something this well defined:
//! DEFLATE is a 1996 specification that has not moved since, and what is
//! needed is the writing half of it, which is the half you are allowed to do
//! badly. A decoder must understand every stream anyone has ever produced. An
//! encoder only has to produce one that decoders accept, and may leave any
//! amount of compression on the table while doing so.
//!
//! So this does the straightforward thing: LZ77 over a 32 KiB window, then
//! the fixed Huffman code from section 3.2.6 of RFC 1951. Fixed rather than
//! dynamic because a dynamic code has to be built, then written out as a code
//! for writing codes, and for that work it buys perhaps a tenth off a file
//! that is already a third of its original size. The whole site here comes out
//! near what `gzip -6` manages, and the difference costs nothing anybody
//! waiting for a page would notice.
//!
//! This is only ever asked to compress the same dozen files, once, when the
//! process starts. It is not on any request path.
//!
//! The one thing it must get right is being *correct*, and that cannot be
//! proved here. The tests below check the frame around the compressed data and
//! nothing about the data itself, because there is no decoder in this
//! repository to read it back with, and one written beside this would only
//! prove that the two agree with each other.
//!
//! What proves it is `tools/site_smoke.mjs`: it asks the running server for a
//! compressed file and compares it against the uncompressed one through
//! somebody else's zlib, and then loads the game in a browser that has to
//! accept every byte to start. This is not a theoretical distinction. Writing
//! a distance code with the bits the wrong way round leaves every test in this
//! file passing and produces a stream no decoder on earth will read.

/// Biggest distance back a match may point, which is the window size.
const WINDOW: usize = 32_768;
/// The longest and shortest runs the format can name.
const LONGEST_MATCH: usize = 258;
const SHORTEST_MATCH: usize = 3;
/// How far down a hash chain to look before taking the best so far.
///
/// The whole reason to have a number here is that chains get long in files
/// that repeat themselves, and this runs on a megabyte of JavaScript at
/// startup. At this depth the site compresses in a few milliseconds.
const MOST_TRIES: usize = 192;

/// How many buckets the match finder hashes into.
const BUCKETS: usize = 1 << 15;

/// Where each length code starts, for codes 257 through 285.
const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];

/// And where each distance code starts, for codes 0 through 29.
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

/// Writes bits the way DEFLATE wants them.
///
/// Two conventions live here and they disagree, which is the single easiest
/// thing to get wrong in this format. Bits fill a byte from its least
/// significant end upward. But a Huffman code is written starting from its
/// *most* significant bit, so a code has to be reversed on the way in, while
/// the extra bits that follow it do not.
struct Bits {
    out: Vec<u8>,
    /// Bits waiting for a byte to fill, held at the bottom of this word.
    held: u32,
    count: u32,
}

impl Bits {
    fn new(capacity: usize) -> Bits {
        Bits { out: Vec::with_capacity(capacity), held: 0, count: 0 }
    }

    /// Adds `len` bits, least significant first. For extra bits and headers.
    fn put(&mut self, value: u32, len: u32) {
        debug_assert!(len <= 16 && self.count < 8);
        self.held |= (value & ((1 << len) - 1)) << self.count;
        self.count += len;
        while self.count >= 8 {
            self.out.push(self.held as u8);
            self.held >>= 8;
            self.count -= 8;
        }
    }

    /// Adds a Huffman code, most significant bit first.
    fn put_code(&mut self, code: u32, len: u32) {
        let mut flipped = 0;
        for bit in 0..len {
            flipped |= ((code >> bit) & 1) << (len - 1 - bit);
        }
        self.put(flipped, len);
    }

    fn finish(mut self) -> Vec<u8> {
        if self.count > 0 {
            self.out.push(self.held as u8);
        }
        self.out
    }
}

/// The fixed literal and length code: a symbol's bit pattern and its width.
///
/// Straight out of the table in RFC 1951 section 3.2.6. Four ranges, each
/// starting at a number the specification names, which is why this is
/// arithmetic rather than a 288 entry table.
fn fixed_literal(symbol: u16) -> (u32, u32) {
    match symbol {
        0..=143 => (0x30 + symbol as u32, 8),
        144..=255 => (0x190 + symbol as u32 - 144, 9),
        256..=279 => (symbol as u32 - 256, 7),
        _ => (0xc0 + symbol as u32 - 280, 8),
    }
}

/// Which length code covers a run this long, and what it leaves to extra bits.
fn length_code(length: usize) -> (u16, u32, u32) {
    let mut at = LENGTH_BASE.len() - 1;
    while LENGTH_BASE[at] as usize > length {
        at -= 1;
    }
    let extra = LENGTH_EXTRA[at] as u32;
    (257 + at as u16, (length - LENGTH_BASE[at] as usize) as u32, extra)
}

/// The same for a distance back.
fn distance_code(distance: usize) -> (u16, u32, u32) {
    let mut at = DIST_BASE.len() - 1;
    while DIST_BASE[at] as usize > distance {
        at -= 1;
    }
    let extra = DIST_EXTRA[at] as u32;
    (at as u16, (distance - DIST_BASE[at] as usize) as u32, extra)
}

/// Compresses to a gzip stream, header and trailer and all.
///
/// The wrapper is RFC 1952: a fixed ten byte header saying "deflate, nothing
/// optional, no timestamp, unknown operating system", then the compressed
/// data, then the checksum and the original length so a decoder can say it got
/// back what was put in.
pub fn gzip(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len() / 2 + 64);
    out.extend_from_slice(&[0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff]);
    out.extend_from_slice(&deflate(input));
    out.extend_from_slice(&crc32(input).to_le_bytes());
    out.extend_from_slice(&(input.len() as u32).to_le_bytes());
    out
}

/// The compressed data on its own, with no wrapper of any kind.
fn deflate(input: &[u8]) -> Vec<u8> {
    let mut bits = Bits::new(input.len() / 2 + 16);
    // One block for the whole input: final, fixed Huffman. Splitting into
    // blocks only pays when different parts of a file want different codes,
    // and with a fixed code there is no different code to want.
    bits.put(1, 1);
    bits.put(1, 2);

    // Where the most recent occurrence of each three byte sequence was, and
    // the one before that, and so on back. `NONE` rather than an Option so the
    // chain is a flat array of numbers.
    const NONE: u32 = u32::MAX;
    let mut head = vec![NONE; BUCKETS];
    let mut prev = vec![NONE; input.len().max(1)];

    let mut at = 0;
    while at < input.len() {
        let (mut best_length, mut best_distance) = (0usize, 0usize);
        if at + SHORTEST_MATCH <= input.len() {
            let key = bucket(&input[at..]);
            let mut candidate = head[key];
            let floor = at.saturating_sub(WINDOW);
            let mut tries = MOST_TRIES;
            while candidate != NONE && (candidate as usize) >= floor && tries > 0 {
                let from = candidate as usize;
                let length = run_length(input, from, at);
                if length > best_length {
                    best_length = length;
                    best_distance = at - from;
                    if length >= LONGEST_MATCH {
                        break;
                    }
                }
                candidate = prev[from];
                tries -= 1;
            }
            // Linked in after being searched, so a position never matches
            // itself.
            prev[at] = head[key];
            head[key] = at as u32;
        }

        if best_length >= SHORTEST_MATCH {
            let (code, extra, width) = length_code(best_length);
            let (symbol, bits_len) = fixed_literal(code);
            bits.put_code(symbol, bits_len);
            bits.put(extra, width);
            let (dist, dist_extra, dist_width) = distance_code(best_distance);
            // Distances use their own five bit fixed code, which is simply the
            // code number written out as it stands.
            bits.put_code(dist as u32, 5);
            bits.put(dist_extra, dist_width);
            // Every position inside a match still has to be hashed, or the
            // next search starts with no idea what just went past.
            for skip in 1..best_length {
                let here = at + skip;
                if here + SHORTEST_MATCH <= input.len() {
                    let key = bucket(&input[here..]);
                    prev[here] = head[key];
                    head[key] = here as u32;
                }
            }
            at += best_length;
        } else {
            let (symbol, len) = fixed_literal(input[at] as u16);
            bits.put_code(symbol, len);
            at += 1;
        }
    }

    // End of block.
    let (symbol, len) = fixed_literal(256);
    bits.put_code(symbol, len);
    bits.finish()
}

/// Which bucket the three bytes at the front of this slice belong in.
fn bucket(from: &[u8]) -> usize {
    let key = (from[0] as u32) << 16 | (from[1] as u32) << 8 | from[2] as u32;
    // Knuth's multiplicative hash, taking the top bits of the product, which
    // are the ones that depend on all of the input.
    (key.wrapping_mul(0x9e37_79b1) >> (32 - 15)) as usize
}

/// How far the bytes at `from` and `at` agree, capped by the format's longest.
fn run_length(input: &[u8], from: usize, at: usize) -> usize {
    let most = LONGEST_MATCH.min(input.len() - at);
    let mut length = 0;
    while length < most && input[from + length] == input[at + length] {
        length += 1;
    }
    length
}

/// The checksum gzip carries, which is the ordinary one.
///
/// Computed a bit at a time rather than from a table, because this runs once
/// per file at startup and a 256 entry table would be more code than the four
/// lines it saves.
pub fn crc32(input: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in input {
        crc ^= *byte as u32;
        for _ in 0..8 {
            let carry = crc & 1;
            crc >>= 1;
            if carry != 0 {
                crc ^= 0xedb8_8320;
            }
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a gzip stream must look like from the outside.
    ///
    /// These do not prove it decompresses: nothing here can, because there is
    /// no decoder in this repository to read it back with, and one written
    /// beside the encoder would only prove the two agree with each other.
    /// `tools/site_smoke.mjs` is what proves it, by handing the output to zlib
    /// and to a browser. What these check is the frame around that.
    fn framing(input: &[u8]) {
        let out = gzip(input);
        assert_eq!(&out[..3], &[0x1f, 0x8b, 8], "not a gzip header");
        assert_eq!(out[8], 0, "the extra flags byte should say nothing");
        assert_eq!(out[9], 0xff, "the operating system should be unknown");
        let tail = out.len() - 8;
        assert_eq!(
            u32::from_le_bytes(out[tail..tail + 4].try_into().unwrap()),
            crc32(input),
            "the checksum in the trailer is not the checksum of the input",
        );
        assert_eq!(
            u32::from_le_bytes(out[tail + 4..].try_into().unwrap()),
            input.len() as u32,
            "the length in the trailer is not the length of the input",
        );
    }

    #[test]
    fn the_wrapper_says_what_it_should_about_anything() {
        framing(b"");
        framing(b"a");
        framing(b"hello hello hello hello");
        framing(&(0..=255u8).collect::<Vec<u8>>());
        framing(&vec![0u8; 100_000]);
    }

    #[test]
    fn the_checksum_is_the_one_everybody_elses_is() {
        // Known answers, so a rewrite of the polynomial loop cannot quietly
        // produce a self-consistent checksum nobody else agrees with.
        assert_eq!(crc32(b""), 0x0000_0000);
        assert_eq!(crc32(b"a"), 0xe8b7_be43);
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b"The quick brown fox jumps over the lazy dog"), 0x414f_a339);
    }

    #[test]
    fn repetition_is_what_it_is_for() {
        // The point of the window. A file that says the same thing over and
        // over must come out far smaller, or the match finder is not finding
        // anything and every byte is going out as a literal.
        let same = "the same sentence again and again. ".repeat(500);
        let out = gzip(same.as_bytes());
        assert!(
            out.len() < same.len() / 40,
            "17 KiB of one repeated sentence compressed to {} bytes",
            out.len(),
        );
    }

    #[test]
    fn real_text_compresses_by_something_worth_having() {
        // Not a ratio to defend to the last percent: this is a fixed Huffman
        // encoder and it is meant to be beaten by a real one. What it must not
        // do is fail to compress ordinary source at all, which is what a
        // broken match finder looks like from the outside.
        let source = include_str!("deflate.rs");
        let out = gzip(source.as_bytes());
        assert!(
            out.len() < source.len() / 2,
            "this file compressed from {} to {} bytes",
            source.len(),
            out.len(),
        );
    }

    #[test]
    fn nothing_incompressible_is_grown_beyond_reason() {
        // Random bytes cannot be compressed, and a fixed Huffman code spends
        // eight or nine bits on each of them, so the result is a little larger
        // than the input. That is expected and the server checks for it before
        // serving anything; what would not be expected is a runaway.
        let mut noise = Vec::new();
        let mut seed = 0x243f_6a88_85a3_08d3u64;
        for _ in 0..20_000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            noise.push(seed as u8);
        }
        let out = gzip(&noise);
        assert!(out.len() < noise.len() * 11 / 10, "noise grew to {} bytes", out.len());
    }

    #[test]
    fn a_length_or_a_distance_lands_in_the_right_code() {
        // Every length and every distance the format can name has to fall in
        // the band whose base it is at or above and whose next base it is
        // below. An off by one here writes a stream that decodes to something
        // else entirely rather than failing.
        for length in SHORTEST_MATCH..=LONGEST_MATCH {
            let (code, extra, width) = length_code(length);
            let at = (code - 257) as usize;
            assert_eq!(LENGTH_BASE[at] as usize + extra as usize, length, "length {length}");
            assert!(extra < (1 << width), "length {length} overflows its extra bits");
        }
        for distance in 1..=WINDOW {
            let (code, extra, width) = distance_code(distance);
            let at = code as usize;
            assert_eq!(DIST_BASE[at] as usize + extra as usize, distance, "distance {distance}");
            assert!(extra < (1 << width), "distance {distance} overflows its extra bits");
        }
    }

    #[test]
    fn a_code_goes_out_most_significant_bit_first() {
        // The one convention in this format that is easy to get backwards, and
        // getting it backwards produces a stream that is valid-looking and
        // wrong. Symbol 0 is eight bits of `00110000`, so into a stream filled
        // from the bottom of each byte it must land as `0000_1100`.
        let mut bits = Bits::new(4);
        let (code, len) = fixed_literal(0);
        assert_eq!((code, len), (0x30, 8));
        bits.put_code(code, len);
        assert_eq!(bits.finish(), vec![0b0000_1100]);
    }
}
