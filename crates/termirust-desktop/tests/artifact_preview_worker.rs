use std::io::{Cursor, Read, Write as _};
use std::process::{Command, Stdio};

use image::{ExtendedColorType, ImageEncoder as _, codecs::png::PngEncoder};

const MAGIC: &[u8; 8] = b"TRAPRVW1";

#[test]
fn artifact_preview_worker_decodes_png_through_the_packaged_process_boundary() {
    let mut png = Vec::new();
    PngEncoder::new(&mut png)
        .write_image(&[12, 34, 56, 255], 1, 1, ExtendedColorType::Rgba8)
        .unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_termirust"))
        .arg("--artifact-preview-worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    input.write_all(MAGIC).unwrap();
    input.write_all(&[1]).unwrap();
    input.write_all(&20_000_000_u64.to_le_bytes()).unwrap();
    input
        .write_all(&(80_u64 * 1024 * 1024).to_le_bytes())
        .unwrap();
    input.write_all(&(png.len() as u64).to_le_bytes()).unwrap();
    input.write_all(&png).unwrap();
    drop(input);

    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let mut response = Cursor::new(output.stdout);
    let mut magic = [0_u8; 8];
    response.read_exact(&mut magic).unwrap();
    assert_eq!(&magic, MAGIC);
    assert_eq!(read_u8(&mut response), 0);
    assert_eq!(read_u32(&mut response), 1);
    assert_eq!(read_u32(&mut response), 1);
    assert_eq!(read_u64(&mut response), 4);
    let mut rgba = [0_u8; 4];
    response.read_exact(&mut rgba).unwrap();
    assert_eq!(rgba, [12, 34, 56, 255]);
    assert_eq!(response.position(), response.get_ref().len() as u64);
}

fn read_u8(input: &mut impl Read) -> u8 {
    let mut bytes = [0_u8; 1];
    input.read_exact(&mut bytes).unwrap();
    bytes[0]
}

fn read_u32(input: &mut impl Read) -> u32 {
    let mut bytes = [0_u8; 4];
    input.read_exact(&mut bytes).unwrap();
    u32::from_le_bytes(bytes)
}

fn read_u64(input: &mut impl Read) -> u64 {
    let mut bytes = [0_u8; 8];
    input.read_exact(&mut bytes).unwrap();
    u64::from_le_bytes(bytes)
}
