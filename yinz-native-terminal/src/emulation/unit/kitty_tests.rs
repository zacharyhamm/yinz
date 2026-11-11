use super::*;

fn media_dir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("yinz-terminal-media-test")
        .tempdir_in(std::env::temp_dir())
        .unwrap()
}

#[test]
fn shared_media_paths_that_climb_out_of_the_media_directory_are_refused() {
    let climbing = format!("{}/../etc/passwd", SHM_DIR);
    assert_eq!(read_shared_media(&climbing, 4), None);
    let climbing = std::env::temp_dir().join("..").join("etc").join("passwd");
    assert_eq!(read_shared_media(&climbing.to_string_lossy(), 4), None);
}

#[test]
fn shared_media_reads_no_more_than_the_record_names() {
    let directory = media_dir();
    let path = directory.path().join("media");
    std::fs::write(&path, vec![7u8; 64]).unwrap();
    assert_eq!(
        read_shared_media(&path.to_string_lossy(), 16),
        Some(vec![7u8; 16])
    );
    assert_eq!(read_shared_media(&path.to_string_lossy(), 65), None);
}

#[cfg(unix)]
#[test]
fn shared_media_that_is_not_a_plain_file_is_refused_without_blocking() {
    let directory = media_dir();
    let fifo = directory.path().join("fifo");
    let name = std::ffi::CString::new(fifo.to_string_lossy().into_owned()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert_eq!(read_shared_media(&fifo.to_string_lossy(), 4), None);
    assert_eq!(
        read_shared_media(&directory.path().to_string_lossy(), 4),
        None
    );
}

fn observed(stream: &[u8]) -> (Vec<u8>, usize) {
    let mut filter = ApcFilter::new();
    let mut text = Vec::new();
    let mut clears = 0usize;
    for byte in stream {
        let step = filter.advance(*byte);
        text.extend_from_slice(step.bytes.as_slice());
        if step.kind == Kind::Cleared {
            clears += 1;
        }
    }
    (text, clears)
}

#[test]
fn an_erase_in_display_is_reported_without_being_consumed() {
    let (text, clears) = observed(b"a\x1b[m\x1b[2Jb");
    assert_eq!(text, b"a\x1b[m\x1b[2Jb");
    assert_eq!(clears, 1);
}

#[test]
fn the_scrollback_erase_is_reported_too() {
    assert_eq!(observed(b"\x1b[3J").1, 1);
}

#[test]
fn a_partial_erase_or_another_final_byte_is_not_a_display_clear() {
    for stream in [
        &b"\x1b[J"[..],
        &b"\x1b[0J"[..],
        &b"\x1b[1J"[..],
        &b"\x1b[2K"[..],
        &b"\x1b[23J"[..],
        &b"\x1b[?2J"[..],
        &b"\x1b[2;3H"[..],
    ] {
        let (text, clears) = observed(stream);
        assert_eq!(text, stream, "{:?}", stream);
        assert_eq!(clears, 0, "{:?}", stream);
    }
}

#[test]
fn an_aborted_control_sequence_does_not_carry_its_parameters_forward() {
    let (text, clears) = observed(b"\x1b[2\x1b[J");
    assert_eq!(text, b"\x1b[2\x1b[J");
    assert_eq!(clears, 0);
}

#[test]
fn a_control_sequence_longer_than_the_parameter_budget_is_still_forwarded() {
    let mut stream = b"\x1b[".to_vec();
    stream.extend(std::iter::repeat_n(b'1', MAX_CSI_PARAMS * 2));
    stream.extend_from_slice(b"J");
    let (text, clears) = observed(&stream);
    assert_eq!(text, stream);
    assert_eq!(clears, 0);
}

#[test]
fn plain_text_passes_through_untouched() {
    let (text, _) = observed(b"hello\x1b[31mworld");
    assert_eq!(text, b"hello\x1b[31mworld");
}

#[test]
fn an_escape_that_is_not_an_apc_is_forwarded_whole() {
    let (text, _) = observed(b"\x1b[2J\x1b]52;c;QQ\x1b\\");
    assert_eq!(text, b"\x1b[2J\x1b]52;c;QQ\x1b\\");
}

#[test]
fn an_apc_or_a_device_control_string_is_forwarded_byte_for_byte() {
    for stream in [
        &b"a\x1b_Ga=t,i=1;QUJD\x1b\\b"[..],
        &b"\x1b_Xpayload\x1b\\"[..],
        &b"a\x1bP0;1;0q#0;2;100;0;0@\x1b\\b"[..],
        &b"\x1bPq~~~\x9cz"[..],
        &b"\x1bP+q544e\x1b\\"[..],
    ] {
        let (text, clears) = observed(stream);
        assert_eq!(
            text, stream,
            "nothing captures graphics any more; {:?} must reach the parser whole",
            stream
        );
        assert_eq!(clears, 0, "{:?}", stream);
    }
}

#[test]
fn a_repeated_escape_is_not_lost() {
    let (text, _) = observed(b"\x1b\x1bA");
    assert_eq!(text, b"\x1b\x1bA");
}

#[test]
fn a_control_byte_inside_a_control_sequence_is_forwarded_and_does_not_break_recognition() {
    let (text, clears) = observed(b"\x1b[2\x07J");
    assert_eq!(text, b"\x1b[2\x07J");
    assert_eq!(clears, 1);
}

fn steps(stream: &[u8]) -> Vec<&'static str> {
    let mut filter = ApcFilter::new();
    stream
        .iter()
        .map(|byte| match filter.advance(*byte).kind {
            Kind::Forward => "forward",
            Kind::Cleared => "cleared",
            Kind::Boundary => "boundary",
            Kind::Signal(_) => "signal",
        })
        .collect()
}

#[test]
fn every_control_sequence_closes_a_run_and_printable_text_does_not() {
    assert_eq!(steps(b"ab"), vec!["forward", "forward"]);
    assert_eq!(*steps(b"\x1b[31m").last().unwrap(), "boundary");
    assert_eq!(*steps(b"\x1b[s").last().unwrap(), "boundary");
    assert_eq!(*steps(b"\x1b[u").last().unwrap(), "boundary");
    assert_eq!(*steps(b"\x1b[?25l").last().unwrap(), "boundary");
    assert_eq!(*steps(b"\x1b[2;3H").last().unwrap(), "boundary");
    assert_eq!(*steps(b"\x1b7").last().unwrap(), "boundary");
    assert_eq!(*steps(b"\x1b8").last().unwrap(), "boundary");
    assert_eq!(*steps(b"\x1bM").last().unwrap(), "boundary");
    assert_eq!(*steps(b"\r").last().unwrap(), "boundary");
    assert_eq!(*steps(b"\n").last().unwrap(), "boundary");
    assert_eq!(*steps(b"\t").last().unwrap(), "boundary");
    assert_eq!(*steps(b"\x7f").last().unwrap(), "boundary");
    assert_eq!(*steps(b"\x1b[2J").last().unwrap(), "cleared");
}

#[test]
fn a_boundary_carries_its_bytes_so_the_parser_still_sees_them() {
    let (text, _) = observed(b"a\x1b[31mb\rc\x1b7d");
    assert_eq!(text, b"a\x1b[31mb\rc\x1b7d");
}

#[test]
fn the_digest_separates_images_that_differ_by_one_byte() {
    let one = Image {
        width: 1,
        height: 1,
        pixels: vec![1, 2, 3, 4],
    };
    let other = Image {
        width: 1,
        height: 1,
        pixels: vec![1, 2, 3, 5],
    };
    assert_eq!(one.digest(), one.clone().digest());
    assert_ne!(one.digest(), other.digest());
}

#[test]
fn clipboard_updates_decode_only_complete_osc52_writes() {
    use base64::Engine as _;
    for destination in ["c", "p"] {
        for terminator in ["\x07", "\x1b\\"] {
            for text in ["", "copied", "界\nselection"] {
                let encoded = BASE64.encode(text);
                let content = format!("\x1b]52;{destination};{encoded}{terminator}");
                assert_eq!(clipboard_update(&content).as_deref(), Some(text));
            }
        }
    }
    for content in [
        "plain text",
        "\x1b]52;c;?\x07",
        "\x1b]52;c;%%%\x07",
        "\x1b]52;c;/w==\x07",
        "\x1b]52;c;YQ==",
        "\x1b]2;title\x07",
    ] {
        assert_eq!(clipboard_update(content), None, "{content:?}");
    }
    assert_eq!(
        clipboard_update("\x1b]52;c;YQ==\x07\x1b]52;p;Yg==\x1b\\"),
        Some("b".into())
    );
}
