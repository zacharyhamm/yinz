use super::*;
#[test]
fn skill_install_is_explicit_idempotent_and_preserves_other_files() {
    const SKILL: &str = include_str!("../../assets/skills/yinz-sessions/SKILL.md");
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("skills/yinz-sessions/SKILL.md");
    install_content(&path, SKILL, true, false).unwrap();
    assert!(!path.parent().unwrap().exists());
    install_content(&path, SKILL, false, false).unwrap();
    let unrelated = path.with_file_name("personal.md");
    fs::write(&unrelated, "mine").unwrap();
    install_content(&path, SKILL, false, false).unwrap();
    fs::write(&path, "custom instructions").unwrap();
    assert!(install_content(&path, SKILL, false, false).is_err());
    assert!(install_content(&path, SKILL, true, false).is_err());
    install_content(&path, SKILL, true, true).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "custom instructions");
    install_content(&path, SKILL, false, true).unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), SKILL);
    assert_eq!(fs::read_to_string(unrelated).unwrap(), "mine");
}
