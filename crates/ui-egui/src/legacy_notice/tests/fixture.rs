//! Synthetic Word 97 input built from the public CFB and MS-DOC structures.

fn put16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
}

fn put32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn directory(bytes: &mut [u8], name: &str, kind: u8, color: u8, left: u32, child: u32, stream: (u32, u32)) {
    let units: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    for (i, unit) in units.iter().enumerate() {
        put16(bytes, i * 2, *unit);
    }
    put16(bytes, 64, (units.len() * 2) as u16);
    bytes[66] = kind;
    bytes[67] = color;
    put32(bytes, 68, left);
    put32(bytes, 72, u32::MAX);
    put32(bytes, 76, child);
    put32(bytes, 116, stream.0);
    put32(bytes, 120, stream.1);
}

pub(super) fn readable_doc() -> Vec<u8> {
    const END: u32 = 0xFFFF_FFFE;
    const FAT: u32 = 0xFFFF_FFFD;
    let mut bytes = vec![0u8; 19 * 512];
    let header = &mut bytes[..512];
    header[..8].copy_from_slice(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    put16(header, 24, 0x003E);
    put16(header, 26, 3);
    put16(header, 28, 0xFFFE);
    put16(header, 30, 9);
    put16(header, 32, 6);
    put32(header, 44, 1);
    put32(header, 48, 0);
    put32(header, 56, 4096);
    put32(header, 60, END);
    put32(header, 68, END);
    for i in 0..109 {
        put32(header, 76 + i * 4, if i == 0 { 17 } else { u32::MAX });
    }
    let dir = &mut bytes[512..1024];
    directory(&mut dir[..128], "Root Entry", 5, 1, u32::MAX, 1, (END, 0));
    directory(&mut dir[128..256], "WordDocument", 2, 1, 2, u32::MAX, (1, 4096));
    directory(&mut dir[256..384], "0Table", 2, 0, u32::MAX, u32::MAX, (9, 4096));
    // Streams at the mini-stream cutoff avoid needing a second allocation table.
    let word = &mut bytes[1024..5120];
    put16(word, 0, 0xA5EC);
    put16(word, 2, 0x00C1);
    put16(word, 0x0A, 0x1000);
    put16(word, 0x0C, 0x00BF);
    put16(word, 0x20, 0x000E);
    put16(word, 0x3E, 0x0016);
    put32(word, 0x40, 4096);
    put32(word, 0x4C, 6);
    put16(word, 0x98, 0x005D);
    put32(word, 0x9A + 33 * 8, 0x40);
    put32(word, 0x9A + 33 * 8 + 4, 21);
    word[0x600..0x606].copy_from_slice(b"Hello\r");
    let table = &mut bytes[5120..9216];
    table[0x40] = 2;
    put32(table, 0x41, 16);
    put32(table, 0x45, 0);
    put32(table, 0x49, 6);
    put32(table, 0x4F, 0x4000_0C00);
    let fat = &mut bytes[9216..];
    for i in 0..128 {
        let next = match i {
            0 | 8 | 16 => END,
            1..=7 | 9..=15 => i as u32 + 1,
            17 => FAT,
            _ => u32::MAX,
        };
        put32(fat, i * 4, next);
    }
    bytes
}
