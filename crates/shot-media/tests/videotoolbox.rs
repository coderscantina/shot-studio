#![cfg(target_os = "macos")]

use std::path::PathBuf;

use shot_media::videotoolbox::{Decoder, Matrix, PIXEL_FORMAT};

fn fixture(name: &str) -> PathBuf {
    let dir =
        std::env::var_os("SHOT_FIXTURES").expect("SHOT_FIXTURES points at the generated fixtures");
    PathBuf::from(dir).join(name)
}

#[test]
#[ignore = "needs the generated fixtures and the hardware decoder"]
fn the_bt601_output_re_encodes_a_bt709_source_for_gpui() {
    let path = fixture("screen-30s.mp4");
    let matrix = |matrix| {
        let mut decoder = Decoder::open_with(&path, matrix).expect("opening the fixture");
        let frame = decoder
            .next_frame()
            .expect("decoding")
            .expect("a first frame");
        assert_eq!(frame.pixel_format(), PIXEL_FORMAT);
        assert!(frame.is_iosurface_backed());
        frame.ycbcr_matrix()
    };
    assert_eq!(matrix(Matrix::Source).as_deref(), Some("ITU_R_709_2"));
    assert_eq!(matrix(Matrix::Bt601).as_deref(), Some("ITU_R_601_4"));
}
