use super::*;

#[test]
fn a_small_png_is_one_chunk_marked_final() {
    let sequence = png_transmit_sequence(7, b"not really a png, but short");
    assert_eq!(sequence.matches("\u{1b}_G").count(), 1);
    assert!(sequence.contains("a=t,f=100,t=d,q=2,i=7,m=0;"));
    assert!(sequence.ends_with("\u{1b}\\"));
}

#[test]
fn a_large_png_is_split_at_the_protocol_limit_with_only_the_last_chunk_final() {
    // 4096 base64 bytes carry 3072 source bytes, so this is a little over three chunks.
    let png = vec![0xab; 10_000];
    let sequence = png_transmit_sequence(1, &png);
    let commands: Vec<&str> = sequence
        .split("\u{1b}\\")
        .filter(|command| !command.is_empty())
        .collect();
    assert_eq!(commands.len(), 4);
    // Only the first carries control data; every one but the last says more is coming.
    assert!(commands[0].starts_with("\u{1b}_Ga=t,f=100,t=d,q=2,i=1,m=1;"));
    for command in &commands[1..3] {
        assert!(command.starts_with("\u{1b}_Gm=1;"), "{command:.20}");
    }
    assert!(commands[3].starts_with("\u{1b}_Gm=0;"));
    for command in &commands {
        let payload = command
            .split_once(';')
            .expect("every chunk has a payload")
            .1;
        assert!(payload.len() <= 4096, "{} bytes", payload.len());
    }
}

#[test]
fn a_row_of_placeholders_names_its_row_and_every_column() {
    // The first three diacritics in kitty's table, which encode 0, 1 and 2.
    let row = image_row(0x0a0b0c, 1, 3);
    assert_eq!(
        row,
        "\u{1b}[38;2;10;11;12m\
         \u{10EEEE}\u{030D}\u{0305}\
         \u{10EEEE}\u{030D}\u{030D}\
         \u{10EEEE}\u{030D}\u{030E}\
         \u{1b}[39m"
    );
}

#[test]
fn a_row_beyond_the_diacritic_table_draws_nothing_rather_than_the_wrong_row() {
    assert_eq!(image_row(1, ROWCOLUMN_DIACRITICS.len(), 4), String::new());
}

#[test]
fn columns_beyond_the_diacritic_table_are_dropped_rather_than_mis_encoded() {
    let row = image_row(1, 0, ROWCOLUMN_DIACRITICS.len() + 10);
    assert_eq!(row.matches(PLACEHOLDER).count(), ROWCOLUMN_DIACRITICS.len());
}
