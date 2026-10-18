//! Minimal ZIP reader/writer (stored + deflate, no ZIP64) for 3MF and project files.

use super::IoError;

const CRC_TABLE: [u32; 256] = {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
};

pub fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c = CRC_TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

pub struct ZipWriter {
    buf: Vec<u8>,
    central: Vec<u8>,
    count: u16,
}

impl Default for ZipWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl ZipWriter {
    pub fn new() -> Self {
        ZipWriter { buf: Vec::new(), central: Vec::new(), count: 0 }
    }

    /// Adds a file. `compress` selects deflate (level 6) over storing.
    pub fn add(&mut self, name: &str, data: &[u8], compress: bool) {
        self.add_level(name, data, if compress { 6 } else { 0 });
    }

    /// Adds a file with a deflate level (0 = store, 1 = fastest .. 9 = smallest).
    pub fn add_level(&mut self, name: &str, data: &[u8], level: u8) {
        let crc = crc32(data);
        let (method, payload) = if level > 0 {
            let c = miniz_oxide::deflate::compress_to_vec(data, level);
            if c.len() < data.len() { (8u16, c) } else { (0, data.to_vec()) }
        } else {
            (0, data.to_vec())
        };
        let offset = self.buf.len() as u32;
        let name_b = name.as_bytes();
        // Local file header.
        let mut h = Vec::with_capacity(30 + name_b.len());
        h.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        h.extend_from_slice(&20u16.to_le_bytes()); // version needed
        h.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 names
        h.extend_from_slice(&method.to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes()); // time
        h.extend_from_slice(&0x21u16.to_le_bytes()); // date 1980-01-01
        h.extend_from_slice(&crc.to_le_bytes());
        h.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        h.extend_from_slice(&(data.len() as u32).to_le_bytes());
        h.extend_from_slice(&(name_b.len() as u16).to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes());
        h.extend_from_slice(name_b);
        self.buf.extend_from_slice(&h);
        self.buf.extend_from_slice(&payload);
        // Central directory entry.
        let c = &mut self.central;
        c.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        c.extend_from_slice(&20u16.to_le_bytes()); // made by
        c.extend_from_slice(&h[4..30]);
        c.extend_from_slice(&0u16.to_le_bytes()); // comment len
        c.extend_from_slice(&0u16.to_le_bytes()); // disk
        c.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        c.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        c.extend_from_slice(&offset.to_le_bytes());
        c.extend_from_slice(name_b);
        self.count += 1;
    }

    pub fn finish(mut self) -> Vec<u8> {
        let cd_off = self.buf.len() as u32;
        let cd_len = self.central.len() as u32;
        self.buf.append(&mut self.central);
        let b = &mut self.buf;
        b.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        b.extend_from_slice(&[0u8; 4]);
        b.extend_from_slice(&self.count.to_le_bytes());
        b.extend_from_slice(&self.count.to_le_bytes());
        b.extend_from_slice(&cd_len.to_le_bytes());
        b.extend_from_slice(&cd_off.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        self.buf
    }
}

pub struct ZipEntry {
    pub name: String,
    method: u16,
    comp_size: usize,
    size: usize,
    local_offset: usize,
}

pub struct ZipReader<'a> {
    data: &'a [u8],
    pub entries: Vec<ZipEntry>,
}

fn u16_at(d: &[u8], o: usize) -> Result<u16, IoError> {
    d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).ok_or_else(|| IoError::Format("zip truncated".into()))
}
fn u32_at(d: &[u8], o: usize) -> Result<u32, IoError> {
    d.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or_else(|| IoError::Format("zip truncated".into()))
}

impl<'a> ZipReader<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self, IoError> {
        let start = data.len().saturating_sub(22 + 65535);
        let eocd = (start..data.len().saturating_sub(21))
            .rev()
            .find(|&i| data[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])
            .ok_or_else(|| IoError::Format("not a zip archive".into()))?;
        let count = u16_at(data, eocd + 10)? as usize;
        let mut p = u32_at(data, eocd + 16)? as usize;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            if u32_at(data, p)? != 0x0201_4b50 {
                return Err(IoError::Format("bad zip central directory".into()));
            }
            let method = u16_at(data, p + 10)?;
            let comp_size = u32_at(data, p + 20)? as usize;
            let size = u32_at(data, p + 24)? as usize;
            let nlen = u16_at(data, p + 28)? as usize;
            let elen = u16_at(data, p + 30)? as usize;
            let clen = u16_at(data, p + 32)? as usize;
            let local_offset = u32_at(data, p + 42)? as usize;
            let name = String::from_utf8_lossy(data.get(p + 46..p + 46 + nlen).unwrap_or_default()).into_owned();
            entries.push(ZipEntry { name, method, comp_size, size, local_offset });
            p += 46 + nlen + elen + clen;
        }
        Ok(ZipReader { data, entries })
    }

    pub fn find(&self, name: &str) -> Option<&ZipEntry> {
        let name = name.trim_start_matches('/');
        self.entries.iter().find(|e| e.name.eq_ignore_ascii_case(name))
    }

    pub fn read(&self, e: &ZipEntry) -> Result<Vec<u8>, IoError> {
        let d = self.data;
        let o = e.local_offset;
        if u32_at(d, o)? != 0x0403_4b50 {
            return Err(IoError::Format("bad zip local header".into()));
        }
        let start = o + 30 + u16_at(d, o + 26)? as usize + u16_at(d, o + 28)? as usize;
        let raw = d.get(start..start + e.comp_size).ok_or_else(|| IoError::Format("zip entry truncated".into()))?;
        match e.method {
            0 => Ok(raw.to_vec()),
            8 => miniz_oxide::inflate::decompress_to_vec_with_limit(raw, e.size.max(1) * 2 + 1024)
                .map_err(|_| IoError::Format(format!("cannot inflate {}", e.name))),
            m => Err(IoError::Format(format!("unsupported zip compression method {m}"))),
        }
    }

    pub fn read_name(&self, name: &str) -> Result<Vec<u8>, IoError> {
        let e = self.find(name).ok_or_else(|| IoError::Format(format!("missing {name} in archive")))?;
        self.read(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_known_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn roundtrip() {
        let mut w = ZipWriter::new();
        let big = vec![7u8; 10000];
        w.add("a.txt", b"hello", false);
        w.add("dir/b.bin", &big, true);
        let bytes = w.finish();
        let r = ZipReader::new(&bytes).unwrap();
        assert_eq!(r.entries.len(), 2);
        assert_eq!(r.read_name("a.txt").unwrap(), b"hello");
        assert_eq!(r.read_name("/dir/b.bin").unwrap(), big);
    }
}
