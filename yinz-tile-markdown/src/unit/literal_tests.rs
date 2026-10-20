#[test]
fn literal_display_round_trips_markup_and_block_openers() {
    for text in [
        "_name_",
        "**name**",
        "- session",
        "1. session",
        "&amp;",
        "[name](path)",
        "a\\b",
        "# title",
        "a|b",
        "`code`",
        "    /tmp/my_file.txt",
        "\t/tmp/my_file.txt",
        "  \t **日本語**",
    ] {
        let lines = super::render(&super::escape_literal(text), 80, super::Options::compact());
        assert_eq!(lines[0].text, text);
        assert!(lines[0]
            .styles
            .iter()
            .all(|style| *style == super::MdStyle::default()));
    }
}

#[test]
fn literal_documents_keep_explicit_lines() {
    for text in [
        "",
        "one\ntwo",
        "one\n\ntwo",
        "one\n\n\ntwo",
        "\n\none\n\n",
        "\n",
        "\n\n\n",
        "paths:\n    /tmp/my_file.txt\n\t**code**\nend",
        "\n  _topic_\n\n    [draft](path)\n\t# title\n\n",
    ] {
        for options in [super::Options::default(), super::Options::compact()] {
            let lines = super::render(&super::escape_literal(text), 80, options);
            assert_eq!(
                lines
                    .iter()
                    .map(|line| line.text.as_str())
                    .collect::<Vec<_>>(),
                text.split('\n').collect::<Vec<_>>(),
                "{text:?} {options:?}"
            );
            assert!(lines
                .iter()
                .flat_map(|line| &line.styles)
                .all(|style| *style == super::MdStyle::default()));
        }
    }
}
