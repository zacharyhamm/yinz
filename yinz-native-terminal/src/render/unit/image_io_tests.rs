use super::test_helpers::*;
use super::*;

fn image() -> Image {
    Image {
        width: 2,
        height: 2,
        pixels: vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ],
    }
}

#[test]
fn an_image_survives_a_round_trip() {
    assert_eq!(decode(&encode(&image()).unwrap()).unwrap(), image());
}

#[test]
fn encoding_is_byte_stable_for_one_image() {
    assert_eq!(encode(&image()).unwrap(), encode(&image()).unwrap());
}

#[test]
fn a_file_round_trip_preserves_the_pixels() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("nested").join("frame.png");
    write(&path, &image()).unwrap();
    assert_eq!(read(&path).unwrap(), image());
}

#[test]
fn a_non_png_file_is_reported_rather_than_panicking() {
    assert!(decode(b"not a png").is_err());
}
