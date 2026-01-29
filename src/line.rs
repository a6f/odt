/// Helper for translating between byte offsets and (line, column) positions in a text buffer.
/// Lines and columns start at one, not zero.  Column values count codepoints, not bytes.
pub struct LineTable<'a> {
    buf: &'a str,
    cache: Vec<LCB>,
}

const SCALE: usize = 128;

impl<'a> LineTable<'a> {
    pub fn new(buf: &'a str) -> Self {
        let bytes = buf.as_bytes();
        let n = bytes.len().div_ceil(SCALE) + 1;
        let mut cache = Vec::with_capacity(n);
        let mut pos = LCB::origin();
        for (i, b) in bytes.iter().enumerate() {
            if i % SCALE == 0 {
                cache.push(pos);
            }
            pos.update(*b);
        }
        cache.push(pos);
        assert_eq!(cache.len(), n);
        Self { buf, cache }
    }

    pub fn line_at(&self, offset: usize) -> usize {
        self.lcb_at(offset).line()
    }

    pub fn line_col_at(&self, offset: usize) -> (usize, usize) {
        let lcb = self.lcb_at(offset);
        (lcb.line(), lcb.col())
    }

    pub fn line_col_byte_at(&self, offset: usize) -> (usize, usize, usize) {
        let lcb = self.lcb_at(offset);
        (lcb.line(), lcb.col(), lcb.byte())
    }

    fn lcb_at(&self, offset: usize) -> LCB {
        let bytes = self.buf.as_bytes();
        if offset >= bytes.len() {
            return *self.cache.last().unwrap();
        }
        let slot = offset / SCALE;
        let mut pos = self.cache[slot];
        for b in &bytes[slot * SCALE..offset] {
            pos.update(*b);
        }
        pos
    }

    // TODO: add forward mapping -- can bsearch the cache to find approximate offset
}

#[derive(Copy, Clone)]
struct LCB(u64);

impl LCB {
    const fn new(line: usize, col: usize, byte: usize) -> LCB {
        assert!(line >> 31 == 0);
        assert!(col >> 31 == 0);
        assert!(byte >> 2 == 0);
        LCB((line as u64) << 33 | (col as u64) << 2 | (byte as u64))
    }
    const fn origin() -> LCB {
        LCB::new(1, 1, 0)
    }
    const fn line(&self) -> usize {
        (self.0 >> 33) as usize
    }
    const fn col(&self) -> usize {
        (self.0 << 31 >> 33) as usize
    }
    const fn byte(&self) -> usize {
        (self.0 & 3) as usize
    }
    const fn update(&mut self, b: u8) {
        match b {
            b'\n' => self.next_line(),
            0x80..0xc0 => self.next_byte(),
            _ => self.next_col(),
        }
    }
    const fn next_byte(&mut self) {
        assert!(self.byte() < 3);
        self.0 += 1;
    }
    const fn next_col(&mut self) {
        self.0 |= 3;
        self.0 += 1;
    }
    const fn next_line(&mut self) {
        self.0 = (self.0 >> 33 << 33) + (1 << 33) + (1 << 2);
    }
}

use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
pub struct LineTableCache<'a>(Mutex<HashMap<(usize, usize), Box<LineTable<'a>>>>);

impl<'a> LineTableCache<'a> {
    pub fn get(&self, buf: &'a str) -> &'a LineTable<'a> {
        let key = buf.as_bytes().as_ptr_range();
        let key = (key.start as usize, key.end as usize);
        let mut guard = self.0.lock().unwrap();
        let value: &LineTable<'a> = guard
            .entry(key)
            .or_insert_with(|| Box::new(LineTable::new(buf)));
        // SAFETY:  We never erase items from the map.
        // Hashtable resizes may move the Box, but not its contents.
        let value: &'a LineTable<'a> = unsafe { core::mem::transmute(value) };
        drop(guard);
        value
    }

    /// equal to `span.start_pos().line_col()` but faster
    pub fn start_line_col(&self, span: &pest::Span<'a>) -> (usize, usize) {
        self.get(span.get_input()).line_col_at(span.start())
    }

    pub fn end_line_col(&self, span: &pest::Span<'a>) -> (usize, usize) {
        self.get(span.get_input()).line_col_at(span.end())
    }
}
