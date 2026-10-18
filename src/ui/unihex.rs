use crate::protocol::Result;
pub(super) struct Decoded {
    pub scalar: Option<char>,
    pub rows: [u32; 16],
    pub width: usize,
    pub left: usize,
    pub right: usize,
}
pub(super) fn decode(line: &str) -> Result<Decoded> {
    // Check byte shapes before slicing: malformed Unicode is never a bitmap offset.
    let (code, bits) = line.split_once(':').ok_or("invalid unihex line")?;
    if !(4..=6).contains(&code.len()) || !code.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("unihex codepoint width/digits".into());
    }
    if !matches!(bits.len(), 32 | 64 | 96 | 128) || !bits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("unihex glyph width/digits".into());
    }
    let scalar = u32::from_str_radix(code, 16).map_err(|_| "unihex codepoint")?;
    let width = bits.len() / 4;
    let digits = width / 4;
    let mut rows = [0u32; 16];
    let mut left = width;
    let mut right = 0;
    for (y, row) in rows.iter_mut().enumerate() {
        *row = u32::from_str_radix(&bits[y * digits..(y + 1) * digits], 16)
            .map_err(|_| "unihex bitmap digits")?;
        if *row != 0 {
            let leading = row.leading_zeros() as usize - (32 - width);
            let trailing = row.trailing_zeros() as usize;
            left = left.min(leading);
            right = right.max(width - 1 - trailing);
        }
    }
    Ok(Decoded {
        scalar: char::from_u32(scalar),
        rows,
        width,
        left,
        right,
    })
}
