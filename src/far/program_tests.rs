use super::*;
use std::io::Read;

/// Program for seed 81 and the server's `lod_column` answers, written by the
/// server test `lod_program_exports_without_the_seed` with
/// `SOLARIS_LOD_PROGRAM_FIXTURE` set.
pub(crate) fn fixture() -> (Vec<u8>, Vec<(i32, i32, i32, String)>) {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/far-program-seed81.zlib"
    );
    let compressed = std::fs::read(path).expect("far terrain program fixture");
    let mut raw = Vec::new();
    flate2::read::ZlibDecoder::new(&compressed[..])
        .read_to_end(&mut raw)
        .expect("zlib fixture");
    let word = |at: usize| u32::from_le_bytes(raw[at..at + 4].try_into().unwrap());
    let length = word(0) as usize;
    let program = raw[4..4 + length].to_vec();
    let mut at = 4 + length;
    let count = word(at) as usize;
    at += 4;
    let mut columns = Vec::with_capacity(count);
    for _ in 0..count {
        let [x, z, height] = std::array::from_fn(|i| word(at + i * 4) as i32);
        at += 12;
        let name_length = usize::from(u16::from_le_bytes([raw[at], raw[at + 1]]));
        at += 2;
        let name = String::from_utf8(raw[at..at + name_length].to_vec()).unwrap();
        at += name_length;
        columns.push((x, z, height, name));
    }
    assert_eq!(at, raw.len());
    (program, columns)
}

#[test]
fn cpu_program_matches_the_server_lod_column_exactly() {
    let (bytes, columns) = fixture();
    let program = Program::decode(&bytes).expect("decode");
    assert!(program.registers() <= MAX_REGISTERS);
    let mut regs = Vec::new();
    for (x, z, height, biome) in &columns {
        let column = program.column(*x, *z, &mut regs);
        assert_eq!(
            (
                column.height,
                program.biomes[usize::from(column.biome)].as_str()
            ),
            (*height, biome.as_str()),
            "column ({x}, {z})"
        );
    }
}

#[test]
fn decode_rejects_truncated_and_forward_reading_programs() {
    let (bytes, _) = fixture();
    assert!(Program::decode(&bytes[..bytes.len() - 1]).is_err());
    let mut longer = bytes.clone();
    longer.push(0);
    assert!(Program::decode(&longer).is_err());
    let mut version = bytes.clone();
    version[4] = 9;
    assert!(Program::decode(&version).is_err());
    // The first surface instruction: make it an `add` of registers 0 and 1,
    // which it cannot read before writing them.
    let program = Program::decode(&bytes).unwrap();
    let octave_bytes = program.octaves.len() * (256 + 24);
    let mut at = 4 + 2 + 20 + 4 + octave_bytes + 4;
    for normal in &program.normals {
        at += 8;
        for perlin in [&normal.first, &normal.second] {
            at += 16 + 4 + perlin.levels.len() * 12;
        }
    }
    at += 4 + program.blended.len() * (40 * 4 + 5 * 8);
    at += 8 + 4; // flat_len, root, instruction count
    let mut forward = bytes.clone();
    forward[at] = 1;
    forward[at + 1..at + 5].copy_from_slice(&0u32.to_le_bytes());
    forward[at + 5..at + 9].copy_from_slice(&1u32.to_le_bytes());
    let error = Program::decode(&forward)
        .err()
        .expect("forward register read");
    assert!(error.contains("register"), "{error}");
}
