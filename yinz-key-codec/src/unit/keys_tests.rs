use super::*;
#[test]
fn modified_backtab_keeps_implicit_shift() {
    assert_eq!(
        key_from_bytes_with_no_ending_byte(b"Z", b"5"),
        Some(
            KeyWithModifier::new(BareKey::Tab)
                .with_ctrl_modifier()
                .with_shift_modifier()
        )
    );
}

#[test]
pub fn can_parse_unicode_bare_keys() {
    let key = "1087"; // п
    assert_eq!(
        bare_key_from_bytes_with_u(key.as_bytes()),
        Some(BareKey::Char('п')),
        "Can parse a bare 'п' keypress"
    );
    let key = "1255"; // ӧ
    assert_eq!(
        bare_key_from_bytes_with_u(key.as_bytes()),
        Some(BareKey::Char('ӧ')),
        "Can parse a bare 'ӧ' keypress"
    );
    let key = "1098"; // ъ
    assert_eq!(
        bare_key_from_bytes_with_u(key.as_bytes()),
        Some(BareKey::Char('ъ')),
        "Can parse a bare 'ъ' keypress"
    );
}
