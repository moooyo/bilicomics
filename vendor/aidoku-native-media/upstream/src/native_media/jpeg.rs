//! Conservative JPEG admission before an untrusted bitstream reaches JDEC.

use std::ops::Range;

pub const MAX_ENCODED_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_PIXELS: usize = 6 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sampling {
    Gray,
    Yuv422,
    Yuv444,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Information {
    pub width: u32,
    pub height: u32,
    pub sampling: Sampling,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejection {
    NotJpeg,
    Limits,
    Coding,
    Sampling,
    ColorSpace,
    OddColorWidth,
    Malformed,
}

#[derive(Default)]
struct Huffman {
    codes: [u32; 17],
    counts: [usize; 17],
    indices: [usize; 17],
    symbols: Vec<u8>,
    fast: Vec<u16>,
}

impl Huffman {
    fn new(counts: &[u8], symbols: &[u8], ac: bool) -> Option<Self> {
        let mut table = Self {
            symbols: symbols.to_vec(),
            fast: vec![0; 256],
            ..Self::default()
        };
        let (mut code, mut index, mut space) = (0_u32, 0, 1_i32);
        let mut seen = [false; 256];
        for (i, &count) in counts.iter().enumerate() {
            let length = i + 1;
            space = space * 2 - i32::from(count);
            if space < 0 {
                return None;
            }
            table.codes[length] = code;
            table.indices[length] = index;
            table.counts[length] = usize::from(count);
            for _ in 0..count {
                let symbol = *symbols.get(index)?;
                let valid = if ac {
                    symbol == 0 || symbol == 0xf0 || (1..=10).contains(&(symbol & 15))
                } else {
                    symbol <= 11
                };
                if !valid || seen[usize::from(symbol)] {
                    return None;
                }
                seen[usize::from(symbol)] = true;
                if length <= 8 {
                    let start = (code << (8 - length)) as usize;
                    let value = (u16::from(symbol) << 5) | length as u16;
                    table
                        .fast
                        .get_mut(start..start + (1 << (8 - length)))?
                        .fill(value);
                }
                code += 1;
                index += 1;
            }
            code <<= 1;
        }
        (index == symbols.len() && index > 0 && space > 0).then_some(table)
    }

    fn read(&self, bits: &mut Bits<'_>) -> Option<u8> {
        if let Some(prefix) = bits.peek(8) {
            let entry = self.fast[prefix as usize];
            if entry != 0 {
                bits.take((entry & 31) as u8)?;
                return Some((entry >> 5) as u8);
            }
        }
        let mut code = 0;
        for length in 1..=16 {
            code = (code << 1) | bits.take(1)?;
            let first = self.codes[length];
            if code >= first && (code - first) < self.counts[length] as u32 {
                return self
                    .symbols
                    .get(self.indices[length] + (code - first) as usize)
                    .copied();
            }
        }
        None
    }
}

struct Bits<'a> {
    data: &'a [u8],
    position: usize,
    value: u32,
    count: u8,
}

impl Bits<'_> {
    fn peek(&mut self, count: u8) -> Option<u32> {
        while self.count < count {
            let value = *self.data.get(self.position)?;
            self.position += 1;
            if value == 255 {
                if self.data.get(self.position) != Some(&0) {
                    return None;
                }
                self.position += 1;
            }
            self.value = (self.value << 8) | u32::from(value);
            self.count += 8;
        }
        Some((self.value >> (self.count - count)) & ((1_u32 << count) - 1))
    }

    fn take(&mut self, count: u8) -> Option<u32> {
        if count == 0 {
            return Some(0);
        }
        let result = self.peek(count)?;
        self.count -= count;
        Some(result)
    }
}

pub struct Parsed {
    pub information: Information,
    components: usize,
    horizontal: [usize; 3],
    selectors: [(usize, usize); 3],
    dc: [Option<Huffman>; 2],
    ac: [Option<Huffman>; 2],
    entropy: Range<usize>,
}

fn word(bytes: &[u8]) -> u32 {
    u32::from(u16::from_be_bytes([bytes[0], bytes[1]]))
}

/// Read structural bounds and the qualified coding/color layout. The parent
/// uses this cheap pass for geometry; the isolated worker also validates every
/// entropy block before calling the driver.
pub fn parse(bytes: &[u8]) -> Result<Parsed, Rejection> {
    if !bytes.starts_with(&[255, 216]) {
        return Err(Rejection::NotJpeg);
    }
    if bytes.len() > MAX_ENCODED_BYTES {
        return Err(Rejection::Limits);
    }
    let mut position = 2;
    let mut information = None;
    let mut components = 0;
    let (mut ids, mut horizontal, mut quantizers) = ([0_u8; 3], [0_usize; 3], [0_usize; 3]);
    let mut quantization = [false; 4];
    let mut dc = [None, None];
    let mut ac = [None, None];
    let mut adobe = None;
    while position < bytes.len() {
        if bytes.get(position) != Some(&255) {
            return Err(Rejection::Malformed);
        }
        while bytes.get(position) == Some(&255) {
            position += 1;
        }
        let marker = *bytes.get(position).ok_or(Rejection::Malformed)?;
        position += 1;
        if marker == 1 {
            continue;
        }
        if marker == 0 || marker == 216 || marker == 217 || (208..=215).contains(&marker) {
            return Err(Rejection::Malformed);
        }
        let length = word(
            bytes
                .get(position..position + 2)
                .ok_or(Rejection::Malformed)?,
        ) as usize;
        if length < 2 {
            return Err(Rejection::Malformed);
        }
        let body = bytes
            .get(position + 2..position.checked_add(length).ok_or(Rejection::Malformed)?)
            .ok_or(Rejection::Malformed)?;
        position += length;
        match marker {
            0xc0 => {
                if information.is_some() || body.len() < 6 || body[0] != 8 {
                    return Err(Rejection::Coding);
                }
                components = usize::from(body[5]);
                if !matches!(components, 1 | 3) || body.len() != 6 + 3 * components {
                    return Err(Rejection::Coding);
                }
                let (height, width) = (word(&body[1..3]), word(&body[3..5]));
                if !(16..=2048).contains(&width)
                    || !(16..=4096).contains(&height)
                    || u64::from(width) * u64::from(height) > MAX_PIXELS as u64
                {
                    return Err(Rejection::Limits);
                }
                for i in 0..components {
                    ids[i] = body[6 + i * 3];
                    horizontal[i] = usize::from(body[7 + i * 3] >> 4);
                    if body[7 + i * 3] & 15 != 1 {
                        return Err(Rejection::Sampling);
                    }
                    quantizers[i] = usize::from(body[8 + i * 3]);
                    if quantizers[i] >= 4 {
                        return Err(Rejection::Malformed);
                    }
                }
                let sampling = if components == 1 && horizontal[0] == 1 {
                    Sampling::Gray
                } else if components == 3 && ids == [1, 2, 3] && horizontal == [2, 1, 1] {
                    Sampling::Yuv422
                } else if components == 3 && ids == [1, 2, 3] && horizontal == [1, 1, 1] {
                    Sampling::Yuv444
                } else {
                    return Err(Rejection::Sampling);
                };
                if components == 3 && width % 2 != 0 {
                    return Err(Rejection::OddColorWidth);
                }
                information = Some(Information {
                    width,
                    height,
                    sampling,
                });
            }
            0xc1..=0xcf if !matches!(marker, 0xc4 | 0xc8) => return Err(Rejection::Coding),
            0xdb => {
                let mut index = 0;
                while index < body.len() {
                    let descriptor = body[index];
                    index += 1;
                    if descriptor >> 4 != 0 || descriptor & 15 >= 4 {
                        return Err(Rejection::Coding);
                    }
                    let values = body.get(index..index + 64).ok_or(Rejection::Malformed)?;
                    if values.contains(&0) {
                        return Err(Rejection::Malformed);
                    }
                    quantization[usize::from(descriptor & 15)] = true;
                    index += 64;
                }
            }
            0xc4 => {
                let mut index = 0;
                while index < body.len() {
                    let descriptor = body[index];
                    index += 1;
                    let (class, id) = (descriptor >> 4, usize::from(descriptor & 15));
                    if class > 1 || id >= 2 {
                        return Err(Rejection::Coding);
                    }
                    let counts = body.get(index..index + 16).ok_or(Rejection::Malformed)?;
                    index += 16;
                    let count: usize = counts.iter().map(|&n| usize::from(n)).sum();
                    if count > 256 {
                        return Err(Rejection::Malformed);
                    }
                    let symbols = body.get(index..index + count).ok_or(Rejection::Malformed)?;
                    let table =
                        Huffman::new(counts, symbols, class == 1).ok_or(Rejection::Malformed)?;
                    if class == 0 {
                        dc[id] = Some(table);
                    } else {
                        ac[id] = Some(table);
                    }
                    index += count;
                }
            }
            0xdd => {
                if body != [0, 0] {
                    return Err(Rejection::Coding);
                }
            }
            0xee if body.starts_with(b"Adobe") => {
                if body.len() < 12 {
                    return Err(Rejection::Malformed);
                }
                adobe = Some(body[11]);
            }
            0xda => {
                let information = information.ok_or(Rejection::Malformed)?;
                if components == 3 && adobe.is_some_and(|value| value != 1) {
                    return Err(Rejection::ColorSpace);
                }
                if body.len() != 4 + components * 2
                    || usize::from(body[0]) != components
                    || body[1 + components * 2..] != [0, 63, 0]
                {
                    return Err(Rejection::Coding);
                }
                let mut selectors = [(0, 0); 3];
                for i in 0..components {
                    if body[1 + 2 * i] != ids[i] || !quantization[quantizers[i]] {
                        return Err(Rejection::Malformed);
                    }
                    let selector = body[2 + 2 * i];
                    let (d, a) = (usize::from(selector >> 4), usize::from(selector & 15));
                    if d >= 2 || a >= 2 || dc[d].is_none() || ac[a].is_none() {
                        return Err(Rejection::Malformed);
                    }
                    selectors[i] = (d, a);
                }
                let start = position;
                while position < bytes.len() {
                    if bytes[position] != 255 {
                        position += 1;
                        continue;
                    }
                    let next = *bytes.get(position + 1).ok_or(Rejection::Malformed)?;
                    if next == 0 {
                        position += 2;
                        continue;
                    }
                    if next == 217
                        && bytes[position + 2..].iter().all(|&value| value == 0)
                        && position > start
                    {
                        return Ok(Parsed {
                            information,
                            components,
                            horizontal,
                            selectors,
                            dc,
                            ac,
                            entropy: start..position,
                        });
                    }
                    return Err(Rejection::Coding);
                }
                return Err(Rejection::Malformed);
            }
            _ => {}
        }
    }
    Err(Rejection::Malformed)
}

impl Parsed {
    /// Validate every Huffman block without IDCT, RGB allocation, or resizing.
    /// This keeps malformed entropy away from the kernel while retaining the
    /// useful decode/resize acceleration for already bounded baseline images.
    pub fn validate_entropy(&self, bytes: &[u8]) -> bool {
        let Some(data) = bytes.get(self.entropy.clone()) else {
            return false;
        };
        let mut bits = Bits {
            data,
            position: 0,
            value: 0,
            count: 0,
        };
        let mcus = self
            .information
            .width
            .div_ceil(8 * self.horizontal[0] as u32)
            * self.information.height.div_ceil(8);
        let mut predictors = [0_i32; 3];
        for _ in 0..mcus {
            for (component, predictor) in predictors.iter_mut().enumerate().take(self.components) {
                let (d, a) = self.selectors[component];
                let (dc, ac) = (self.dc[d].as_ref().unwrap(), self.ac[a].as_ref().unwrap());
                for _ in 0..self.horizontal[component] {
                    let Some(size) = dc.read(&mut bits) else {
                        return false;
                    };
                    let Some(value) = bits.take(size) else {
                        return false;
                    };
                    let delta = if size > 0 && value < (1 << (size - 1)) {
                        value as i32 + 1 - (1 << size)
                    } else {
                        value as i32
                    };
                    *predictor += delta;
                    if !(-1024..=1023).contains(predictor) {
                        return false;
                    }
                    let mut coefficient = 1;
                    while coefficient < 64 {
                        let Some(symbol) = ac.read(&mut bits) else {
                            return false;
                        };
                        if symbol == 0 {
                            break;
                        }
                        if symbol == 0xf0 {
                            coefficient += 16;
                            if coefficient > 64 {
                                return false;
                            }
                            continue;
                        }
                        coefficient += usize::from(symbol >> 4);
                        if coefficient >= 64 || bits.take(symbol & 15).is_none() {
                            return false;
                        }
                        coefficient += 1;
                    }
                }
            }
        }
        bits.position == data.len() && bits.count < 8
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GrayImage, Luma, Rgb, RgbImage, codecs::jpeg::JpegEncoder};

    fn fixture(color: bool) -> Vec<u8> {
        let mut bytes = Vec::new();
        if color {
            let image = RgbImage::from_fn(130, 97, |x, y| {
                Rgb([(x * 7) as u8, (y * 11) as u8, (x + y) as u8])
            });
            JpegEncoder::new_with_quality(&mut bytes, 92)
                .encode_image(&image)
                .unwrap();
        } else {
            let image = GrayImage::from_fn(129, 97, |x, y| Luma([(x * 7 + y * 11) as u8]));
            JpegEncoder::new_with_quality(&mut bytes, 92)
                .encode_image(&image)
                .unwrap();
        }
        bytes
    }

    #[test]
    fn real_baseline_entropy_is_complete_including_edge_blocks() {
        for color in [false, true] {
            let bytes = fixture(color);
            let parsed = parse(&bytes).unwrap();
            assert!(parsed.validate_entropy(&bytes));
        }
    }

    #[test]
    fn entropy_damage_is_rejected_even_when_headers_and_eoi_remain() {
        let mut bytes = fixture(false);
        let parsed = parse(&bytes).unwrap();
        bytes[parsed.entropy.clone()].fill(0);
        assert!(!parsed.validate_entropy(&bytes));
    }

    #[test]
    fn truncated_and_non_jpeg_inputs_never_reach_the_worker() {
        let bytes = fixture(false);
        assert!(parse(b"not a jpeg").is_err());
        for length in [0, 1, 2, 8, bytes.len() - 1] {
            assert!(parse(&bytes[..length]).is_err());
        }
    }
}
