//! One test per case in swamp's `content_redactor_test.ts`, same inputs and
//! expectations, plus checks on what the port adds.

use super::*;

// --- redact ---

#[test]
fn returns_text_unchanged_when_nothing_sensitive() {
    let input = "The model failed to run because the schema was invalid.";
    let result = redact(input);
    assert_eq!(result.text, input);
    assert_eq!(result.summary.total, 0);
}

#[test]
fn redacts_email_addresses() {
    let result = redact("Contact jane.doe@acme-corp.com for details");
    assert_eq!(result.text, "Contact [REDACTED-EMAIL] for details");
    assert_eq!(result.summary.count(Category::Email), 1);
}

#[test]
fn redacts_aws_access_key_ids() {
    let result = redact("The key AKIAIOSFODNN7EXAMPLE was exposed");
    assert_eq!(result.text, "The key [REDACTED-SECRET-1] was exposed");
    assert_eq!(result.summary.count(Category::Secret), 1);
}

#[test]
fn redacts_github_tokens() {
    let result = redact("Token ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmn was in the log");
    assert_eq!(result.text, "Token [REDACTED-SECRET-1] was in the log");
}

#[test]
fn redacts_bearer_tokens() {
    let result = redact("Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.test");
    assert!(result.text.contains("Bearer [REDACTED-SECRET-1]"));
}

#[test]
fn redacts_padded_bearer_tokens() {
    let result = redact("Bearer dG9rZW5WYWx1ZTEyMzQ1Njc4OTAxMjM0NTY3ODkw== next");
    assert_eq!(result.text, "Bearer [REDACTED-SECRET-1] next");
}

#[test]
fn bearer_at_end_of_line_leaves_the_next_line_alone() {
    let result = redact("The header was Bearer\nRestart the daemon");
    assert_eq!(result.text, "The header was Bearer\nRestart the daemon");
}

#[test]
fn redacts_prefixed_api_keys() {
    let result = redact("API key sk_test_00000000000000000000");
    assert_eq!(result.text, "API key [REDACTED-SECRET-1]");
}

#[test]
fn redacts_env_var_style_secrets() {
    let result = redact("DATABASE_PASSWORD=hunter2 was in .env");
    assert_eq!(
        result.text,
        "DATABASE_PASSWORD=[REDACTED-SECRET-1] was in .env"
    );
}

#[test]
fn env_var_values_keep_their_closing_quote() {
    let result = redact(r#"run: echo "MY_TOKEN=hunter2" in the shell"#);
    assert_eq!(
        result.text,
        r#"run: echo "MY_TOKEN=[REDACTED-SECRET-1]" in the shell"#
    );
}

#[test]
fn quote_initial_env_var_values_are_redacted_inside_their_delimiters() {
    let result = redact(r#"MY_TOKEN="real secret value""#);
    assert_eq!(result.text, r#"MY_TOKEN="[REDACTED-SECRET-1]""#);
}

#[test]
fn masked_env_var_values_pass_through_untouched() {
    let result = redact("| case A | MY_TOKEN=*** |");
    assert_eq!(result.text, "| case A | MY_TOKEN=*** |");
    assert_eq!(result.summary.total, 0);
}

#[test]
fn variable_reference_env_var_values_pass_through_untouched() {
    let input = r#"run: echo "MY_TOKEN=${FROM_VAULT}""#;
    let result = redact(input);
    assert_eq!(result.text, input);
    assert_eq!(result.summary.total, 0);
}

#[test]
fn masked_vs_unmasked_evidence_table_keeps_its_contrast() {
    let input = [
        "| case A | MY_TOKEN=*** |",
        "| case B | MY_TOKEN=the-actual-value |",
    ]
    .join("\n");
    let result = redact(&input);
    let rows: Vec<&str> = result.text.split('\n').collect();
    assert_eq!(rows[0], "| case A | MY_TOKEN=*** |");
    assert_eq!(rows[1], "| case B | MY_TOKEN=[REDACTED-SECRET-1] |");
}

#[test]
fn distinct_env_var_values_get_distinct_placeholders_same_value_the_same_one() {
    let result = redact("MY_TOKEN=first-value OTHER_SECRET=second-value AGAIN_TOKEN=first-value");
    assert_eq!(
        result.text,
        "MY_TOKEN=[REDACTED-SECRET-1] OTHER_SECRET=[REDACTED-SECRET-2] AGAIN_TOKEN=[REDACTED-SECRET-1]"
    );
}

#[test]
fn camel_case_uri_params_with_auth_keyword_pass_through_unredacted() {
    let r1 = redact("mongodb://HOST:27017/?replicaSet=rs0&authSource=admin");
    assert_eq!(
        r1.text,
        "mongodb://HOST:27017/?replicaSet=rs0&authSource=admin"
    );
    assert_eq!(r1.summary.total, 0);

    let r2 = redact("mongodb://HOST:27017/?authMechanism=SCRAM-SHA-256");
    assert_eq!(r2.text, "mongodb://HOST:27017/?authMechanism=SCRAM-SHA-256");
    assert_eq!(r2.summary.total, 0);
}

#[test]
fn upper_snake_case_env_vars_with_auth_keyword_still_redact() {
    let r1 = redact("AUTH_TOKEN=my_secret_token");
    assert_eq!(r1.text, "AUTH_TOKEN=[REDACTED-SECRET-1]");

    let r2 = redact("MY_AUTH=some_value");
    assert_eq!(r2.text, "MY_AUTH=[REDACTED-SECRET-1]");
}

#[test]
fn camel_case_names_with_high_confidence_secret_keywords_still_redact() {
    let r1 = redact("myPassword=hunter2");
    assert_eq!(r1.text, "myPassword=[REDACTED-SECRET-1]");

    let r2 = redact("accessToken=tok_abc");
    assert_eq!(r2.text, "accessToken=[REDACTED-SECRET-1]");

    let r3 = redact("apiSecret=xyz123");
    assert_eq!(r3.text, "apiSecret=[REDACTED-SECRET-1]");

    let r4 = redact("apiKey=sk-123");
    assert_eq!(r4.text, "apiKey=[REDACTED-SECRET-1]");

    let r5 = redact("authToken=eyJhbGciOiJIUzI1NiJ9");
    assert_eq!(r5.text, "authToken=[REDACTED-SECRET-1]");
}

#[test]
fn redacts_ssn_patterns() {
    let result = redact("SSN was 123-45-6789 in the form");
    assert_eq!(result.text, "SSN was [REDACTED-ID] in the form");
}

#[test]
fn redacts_credit_cards_with_luhn_validation() {
    // 4111111111111111 is a valid Luhn test number.
    let result = redact("Card number 4111111111111111 was logged");
    assert_eq!(result.text, "Card number [REDACTED-CC] was logged");
}

#[test]
fn leaves_invalid_credit_card_numbers_alone() {
    let result = redact("ID 1234567890123456 in database");
    assert!(!result.text.contains("[REDACTED-CC]"));
}

#[test]
fn redacts_phone_numbers_with_separators() {
    let result = redact("Call +1 555-867-5309 for support");
    assert!(result.text.contains("[REDACTED-PHONE]"));
}

#[test]
fn does_not_match_bare_digit_runs_as_phone_numbers() {
    let result = redact("batch 20260708 at offset 12345678");
    assert!(!result.text.contains("[REDACTED-PHONE]"));
}

#[test]
fn does_not_redact_space_separated_http_status_codes() {
    let result = redact("404 404 404 404 404 404 404 404");
    assert_eq!(result.text, "404 404 404 404 404 404 404 404");
    assert_eq!(result.summary.total, 0);
}

#[test]
fn does_not_redact_mixed_status_code_runs() {
    let input =
        "HOPS=0:  404 404 404 404 404 404 404 404\nHOPS=1:  404 404 404 404 404 429 429 429";
    let result = redact(input);
    assert_eq!(result.text, input);
}

#[test]
fn does_not_redact_status_codes_in_fenced_code_blocks() {
    let input = "```\n404 404 404 404 404 404 404 404\n```";
    let result = redact(input);
    assert_eq!(result.text, input);
}

#[test]
fn redacts_phone_numbers_with_dot_separators() {
    let result = redact("Call 555.867.5309 for support");
    assert!(result.text.contains("[REDACTED-PHONE]"));
}

#[test]
fn redacts_phone_numbers_with_hyphen_separators() {
    let result = redact("Call 555-867-5309 for support");
    assert!(result.text.contains("[REDACTED-PHONE]"));
}

#[test]
fn redacts_phone_numbers_with_parentheses() {
    let result = redact("Call (555) 867-5309 for support");
    assert!(result.text.contains("[REDACTED-PHONE]"));
}

#[test]
fn redacts_connection_string_credentials() {
    let result = redact("postgres://admin:s3cret@db-prod.internal:5432/mydb");
    assert!(result.text.contains("postgres://[REDACTED-USER]:***@"));
    assert!(result.text.contains(":5432/mydb"));
    assert!(!result.text.contains("s3cret"));
    assert!(!result.text.contains("admin"));
}

#[test]
fn redacts_home_directory_usernames_on_macos() {
    let result = redact("Error at /Users/janedoe/code/swamp/src/main.ts");
    assert_eq!(
        result.text,
        "Error at /Users/[REDACTED]/code/swamp/src/main.ts"
    );
}

#[test]
fn redacts_home_directory_usernames_on_linux() {
    let result = redact("Config in /home/jdoe/.config/swamp/config.yaml");
    assert_eq!(
        result.text,
        "Config in /home/[REDACTED]/.config/swamp/config.yaml"
    );
}

#[test]
fn redacts_home_directory_usernames_on_windows() {
    let result = redact(r"Path C:\Users\JaneDoe\AppData\Local\swamp");
    assert_eq!(result.text, r"Path C:\Users\[REDACTED]\AppData\Local\swamp");
}

#[test]
fn handles_home_home_without_leaking_username() {
    let result = redact("Path /home/home/.config/swamp");
    assert_eq!(result.text, "Path /home/[REDACTED]/.config/swamp");
}

#[test]
fn replaces_ipv4_with_stable_placeholders() {
    let result = redact("Server at 10.0.3.47 could not reach 10.0.3.48, tried 10.0.3.47 again");
    assert!(result.text.contains("[IP-1]"));
    assert!(result.text.contains("[IP-2]"));
    // The same IP gets the same placeholder.
    assert_ne!(result.text.find("[IP-1]"), result.text.rfind("[IP-1]"));
    assert!(!result.text.contains("10.0.3.47"));
    assert!(!result.text.contains("10.0.3.48"));
}

#[test]
fn replaces_public_ipv4_addresses_too() {
    let result = redact("Resolved to 52.14.88.201");
    assert!(result.text.contains("[IP-"));
    assert!(!result.text.contains("52.14.88.201"));
}

#[test]
fn does_not_treat_version_numbers_as_ips() {
    let result = redact("Running swamp version 1.23.4");
    assert_eq!(result.text, "Running swamp version 1.23.4");
}

#[test]
fn replaces_internal_hostnames_with_stable_placeholders() {
    let result = redact("Could not connect to db-prod.internal then tried cache.internal");
    assert!(result.text.contains("[HOST-1]"));
    assert!(result.text.contains("[HOST-2]"));
    assert!(!result.text.contains("db-prod.internal"));
}

#[test]
fn replaces_fqdns_with_stable_placeholders() {
    let result = redact("DNS lookup for api.acme-corp.prod.net failed");
    assert!(result.text.contains("[HOST-"));
    assert!(!result.text.contains("api.acme-corp.prod.net"));
}

#[test]
fn same_hostname_gets_same_placeholder() {
    let result = redact("Tried db.corp twice: first db.corp then db.corp");
    assert_eq!(result.text.matches("[HOST-1]").count(), 3);
}

#[test]
fn preserves_allowlisted_public_hosts() {
    let result = redact("Push to github.com failed, checked api.swamp-club.com");
    assert!(result.text.contains("github.com"));
    assert!(result.text.contains("api.swamp-club.com"));
}

#[test]
fn preserves_subdomains_of_allowlisted_hosts() {
    let result = redact("Fetching from registry.npmjs.org works");
    assert!(result.text.contains("registry.npmjs.org"));
}

#[test]
fn preserves_error_messages_and_stack_traces() {
    let input = "Error: ECONNREFUSED
    at TCPConnectWrap.afterConnect [as oncomplete]
    at Module._compile (node:internal/modules/cjs/loader:1234:14)
  Code: ECONNREFUSED";
    let result = redact(input);
    assert!(result.text.contains("ECONNREFUSED"));
    assert!(result.text.contains("TCPConnectWrap"));
}

#[test]
fn preserves_swamp_diagnostic_info() {
    let input = "swamp version: 1.2.3
OS: darwin (arm64)
Deno: 2.1.0
Shell: /bin/zsh
Model: @adam/cfgmgmt
Method: run";
    let result = redact(input);
    assert!(result.text.contains("1.2.3"));
    assert!(result.text.contains("darwin"));
    assert!(result.text.contains("2.1.0"));
    assert!(result.text.contains("@adam/cfgmgmt"));
}

#[test]
fn handles_multiple_categories_in_one_text() {
    let result = redact("User admin@corp.com hit error on 10.0.1.5 with API_TOKEN=abc123xyz");
    assert!(!result.text.contains("admin@corp.com"));
    assert!(!result.text.contains("10.0.1.5"));
    assert!(!result.text.contains("abc123xyz"));
    assert!(result.summary.total >= 3);
}

#[test]
fn long_hex_strings_are_redacted() {
    let hex = "a".repeat(40);
    let result = redact(&format!("Token was {hex} in header"));
    assert_eq!(result.text, "Token was [REDACTED-SECRET-1] in header");
}

#[test]
fn hex_digest_in_json_checksum_field_is_not_redacted() {
    let sha256 = "868f2f74abc123def456789012345678901234567890abcdef1234567817030a";
    let input = format!(r#"{{ "checksum": "{sha256}", "isLatest": true }}"#);
    let result = redact(&input);
    assert_eq!(result.text, input);
    assert_eq!(result.summary.total, 0);
}

#[test]
fn hex_digest_with_various_safe_key_names_is_not_redacted() {
    let hex = "a1b2c3d4e5f6".repeat(6); // 72 chars
    for key in ["sha256", "sha1", "digest", "hash", "etag", "md5"] {
        let input = format!(r#""{key}": "{hex}""#);
        let result = redact(&input);
        assert_eq!(result.text, input, "expected {key} to be safe");
    }
}

#[test]
fn hex_digest_with_escaped_json_quotes_is_not_redacted() {
    let sha256 = "868f2f74abc123def456789012345678901234567890abcdef1234567817030a";
    let input = format!(r#"\"checksum\": \"{sha256}\""#);
    let result = redact(&input);
    assert_eq!(result.text, input);
    assert_eq!(result.summary.total, 0);
}

#[test]
fn standalone_hex_string_without_safe_key_is_still_redacted() {
    let hex = "a".repeat(64);
    let result = redact(&format!("The value {hex} was found"));
    assert_eq!(result.text, "The value [REDACTED-SECRET-1] was found");
}

#[test]
fn scheme_url_without_credentials_does_not_swallow_following_lines() {
    let input = [
        "Steps:",
        "1. Point the client at ws://gateway:9000/socket",
        "2. Observe the reconnect loop",
        "3. Expected: single connection",
        "",
        "Uses the @swamp/issue-lifecycle package.",
    ]
    .join("\n");
    let result = redact(&input);
    assert_eq!(result.text, input);
    assert_eq!(result.summary.total, 0);
}

#[test]
fn ipv6_addresses_are_redacted() {
    let result = redact("Listening on 2001:0db8:85a3:0000:0000:8a2e:0370:7334");
    assert!(result.text.contains("[IP-"));
    assert!(!result
        .text
        .contains("2001:0db8:85a3:0000:0000:8a2e:0370:7334"));
}

// --- FQDN false-positive guards ---

#[test]
fn dotted_code_identifiers_are_not_treated_as_hostnames() {
    let r1 = redact("s.authScopes.includes(scope)");
    assert_eq!(r1.text, "s.authScopes.includes(scope)");
    assert_eq!(r1.summary.total, 0);

    let r2 = redact(r#"Deno.env.get("SWAMP_API_KEY")"#);
    assert_eq!(r2.text, r#"Deno.env.get("SWAMP_API_KEY")"#);
    assert_eq!(r2.summary.total, 0);

    let r3 = redact("array.forEach.call(ctx)");
    assert_eq!(r3.text, "array.forEach.call(ctx)");
    assert_eq!(r3.summary.total, 0);
}

#[test]
fn dotted_identifiers_with_non_tld_segments_are_not_treated_as_hostnames() {
    let r1 = redact("config.database.timeout was 30s");
    assert_eq!(r1.text, "config.database.timeout was 30s");
    assert_eq!(r1.summary.total, 0);

    let r2 = redact("obj.prototype.toString");
    assert_eq!(r2.text, "obj.prototype.toString");
    assert_eq!(r2.summary.total, 0);
}

#[test]
fn real_fqdns_are_still_redacted() {
    let result = redact("Connected to api.acme-corp.prod.net");
    assert!(result.text.contains("[HOST-"));
    assert!(!result.text.contains("api.acme-corp.prod.net"));
}

// --- Safe IP addresses ---

#[test]
fn loopback_ipv4_is_not_redacted() {
    let r1 = redact("curl http://127.0.0.1:8080/api/health");
    assert_eq!(r1.text, "curl http://127.0.0.1:8080/api/health");
    assert_eq!(r1.summary.total, 0);

    let r2 = redact("listening on 127.0.0.2");
    assert_eq!(r2.text, "listening on 127.0.0.2");
    assert_eq!(r2.summary.total, 0);
}

#[test]
fn rfc_5737_documentation_ips_are_not_redacted() {
    let r1 = redact("example: curl http://192.0.2.1/test");
    assert_eq!(r1.text, "example: curl http://192.0.2.1/test");

    let r2 = redact("see 198.51.100.42 in the docs");
    assert_eq!(r2.text, "see 198.51.100.42 in the docs");

    let r3 = redact("203.0.113.5 is reserved for documentation");
    assert_eq!(r3.text, "203.0.113.5 is reserved for documentation");
}

#[test]
fn unspecified_ipv4_is_not_redacted() {
    let result = redact("bind to 0.0.0.0:3000");
    assert_eq!(result.text, "bind to 0.0.0.0:3000");
}

#[test]
fn link_local_ipv4_is_not_redacted() {
    let result = redact("APIPA assigned 169.254.1.5");
    assert_eq!(result.text, "APIPA assigned 169.254.1.5");
}

#[test]
fn real_ipv4_addresses_are_still_redacted() {
    let result = redact("Server at 52.14.88.201");
    assert!(result.text.contains("[IP-"));
    assert!(!result.text.contains("52.14.88.201"));
}

#[test]
fn ipv6_localhost_and_link_local_are_not_redacted() {
    let r1 = redact("listening on ::1 port 8080");
    assert!(r1.text.contains("::1"));

    let r2 = redact("found fe80:0000:0000:0000:0000:0000:0000:0001 on interface");
    assert!(r2.text.contains("fe80:"));
}

#[test]
fn hh_mm_ss_timestamps_are_not_redacted_as_ips() {
    let input = "system │ Starting workflow remote-demo · 20:18:09 UTC\n build │ done echo-on-worker in 729ms · 14:30:45 UTC";
    let result = redact(input);
    assert!(result.text.contains("20:18:09"));
    assert!(result.text.contains("14:30:45"));
    assert_eq!(result.summary.total, 0);
}

#[test]
fn boundary_timestamps_are_not_redacted() {
    let r1 = redact("started at 00:00:00 UTC");
    assert!(r1.text.contains("00:00:00"));

    let r2 = redact("ended at 23:59:59 UTC");
    assert!(r2.text.contains("23:59:59"));
}

#[test]
fn real_ipv6_with_3_plus_groups_is_still_redacted() {
    let result = redact("Listening on 2001:0db8:85a3:0000:0000:8a2e:0370:7334");
    assert!(result.text.contains("[IP-"));
    assert!(!result
        .text
        .contains("2001:0db8:85a3:0000:0000:8a2e:0370:7334"));
}

// --- Angle-bracket placeholders ---

#[test]
fn angle_bracket_placeholders_in_env_vars_are_not_redacted() {
    let r1 = redact("SWAMP_API_KEY=<collective token>");
    assert_eq!(r1.text, "SWAMP_API_KEY=<collective token>");
    assert_eq!(r1.summary.total, 0);

    let r2 = redact("SWAMP_API_KEY=<token with the matching 12-char head>");
    assert_eq!(
        r2.text,
        "SWAMP_API_KEY=<token with the matching 12-char head>"
    );
    assert_eq!(r2.summary.total, 0);
}

#[test]
fn real_env_var_secrets_are_still_redacted_alongside_safe_ips() {
    let result = redact("API_TOKEN=real_secret_value on 127.0.0.1");
    assert!(result.text.contains("[REDACTED-SECRET-1]"));
    assert!(result.text.contains("127.0.0.1"));
}

// --- redact_title_and_body ---

#[test]
fn title_and_body_share_placeholders() {
    let result = redact_title_and_body(
        "Error on 10.0.3.47",
        "The server 10.0.3.47 returned 500, also tried 10.0.3.48",
    );
    // The same IP in title and body gets the same placeholder.
    assert!(result.title.text.contains("[IP-1]"));
    assert!(result.body.text.contains("[IP-1]"));
    assert!(result.body.text.contains("[IP-2]"));
    assert!(!result.title.text.contains("10.0.3.47"));
    assert!(!result.body.text.contains("10.0.3.47"));
}

#[test]
fn title_and_body_return_a_combined_summary() {
    let result = redact_title_and_body("Bug on 10.0.1.1", "Contact admin@example.org for 10.0.1.2");
    assert!(result.summary.total >= 3);
    assert!(result.summary.count(Category::IpAddress) >= 2);
}

// --- Line-level changes ---

#[test]
fn reports_per_line_changes_with_line_numbers() {
    let input = [
        "First line is fine",
        "DATABASE_PASSWORD=hunter2 leaked here",
        "Third line is fine",
        "Contact admin@corp.com about it",
    ]
    .join("\n");
    let result = redact(&input);
    assert_eq!(result.changes.len(), 2);
    assert_eq!(result.changes[0].line_number, 2);
    assert_eq!(
        result.changes[0].original,
        "DATABASE_PASSWORD=hunter2 leaked here"
    );
    assert_eq!(
        result.changes[0].redacted,
        "DATABASE_PASSWORD=[REDACTED-SECRET-1] leaked here"
    );
    assert_eq!(result.changes[1].line_number, 4);
}

#[test]
fn reports_no_changes_when_nothing_was_redacted() {
    let result = redact("Nothing sensitive here");
    assert!(result.changes.is_empty());
}

#[test]
fn title_and_body_report_changes_per_field() {
    let result = redact_title_and_body("Error contacting 10.0.3.47", "All good in the body");
    assert_eq!(result.title.changes.len(), 1);
    assert_eq!(result.title.changes[0].line_number, 1);
    assert!(result.body.changes.is_empty());
}

// --- details ---

fn change(line_number: usize, original: &str, redacted: &str) -> LineChange {
    LineChange {
        line_number,
        original: original.to_string(),
        redacted: redacted.to_string(),
    }
}

#[test]
fn details_format_line_number_and_before_after() {
    let lines = details(&[change(3, "TOKEN=abc", "TOKEN=[X-1]")], None);
    assert_eq!(lines, ["line 3: TOKEN=abc -> TOKEN=[X-1]"]);
}

#[test]
fn details_prefix_the_label_when_given() {
    let lines = details(&[change(1, "a", "b")], Some("title"));
    assert_eq!(lines, ["title line 1: a -> b"]);
}

#[test]
fn details_escape_newlines_and_truncate_long_content() {
    let long = "x".repeat(500);
    let lines = details(&[change(2, &format!("one\ntwo\n{long}"), "gone")], None);
    assert!(lines[0].contains("one\\ntwo\\n"));
    assert!(!lines[0].contains('\n'));
    assert!(lines[0].contains('…'));
    assert!(lines[0].contains("-> gone"));
}

// --- Summary::describe ---

#[test]
fn describe_is_empty_for_zero_redactions() {
    assert_eq!(Summary::default().describe(), "");
}

#[test]
fn describe_uses_human_readable_names_with_pluralization() {
    let summary = Summary {
        total: 5,
        categories: vec![(Category::Secret, 3), (Category::IpAddress, 2)],
    };
    let msg = summary.describe();
    assert!(msg.contains("3 secrets"));
    assert!(msg.contains("2 IP addresses"));
}

#[test]
fn describe_uses_the_singular_for_a_count_of_one() {
    let summary = Summary {
        total: 1,
        categories: vec![(Category::Email, 1)],
    };
    let msg = summary.describe();
    assert!(msg.contains("1 email"));
    assert!(!msg.contains("emails"));
}

// --- Beyond the TypeScript suite ---

#[test]
fn describe_matches_the_typescript_sentence() {
    let summary = Summary {
        total: 3,
        categories: vec![(Category::Email, 2), (Category::Secret, 1)],
    };
    assert_eq!(
        summary.describe(),
        "Redacted 2 emails, 1 secret from issue content."
    );
}

#[test]
fn body_summary_counts_only_what_the_body_added() {
    let result = redact_title_and_body("Bug on 10.0.1.1", "Contact admin@corp.com");
    assert_eq!(result.title.summary.categories, [(Category::IpAddress, 1)]);
    assert_eq!(result.body.summary.categories, [(Category::Email, 1)]);
    assert_eq!(result.body.summary.total, 1);
    assert_eq!(result.summary.total, 2);
}

#[test]
fn a_redaction_that_consumes_newlines_is_one_block_change() {
    // A phone number's separator may be a newline.
    let result = redact("first\nCall 555\n867-5309 now\nlast");
    assert_eq!(result.text, "first\nCall [REDACTED-PHONE] now\nlast");
    assert_eq!(
        result.changes,
        [change(
            2,
            "Call 555\n867-5309 now",
            "Call [REDACTED-PHONE] now"
        )]
    );
}

#[test]
fn case_insensitive_patterns_match_either_case() {
    assert_eq!(
        redact("Token-ABCDEFGHIJKLMNOPQRSTUVWX here").text,
        "[REDACTED-SECRET-1] here"
    );
    assert_eq!(
        redact("db_password=hunter2").text,
        "db_password=[REDACTED-SECRET-1]"
    );
    let hex = "ab".repeat(20);
    let input = format!(r#""SHA256": "{hex}""#);
    assert_eq!(redact(&input).text, input);
}

#[test]
fn bearer_and_base64_need_whitespace_or_the_end_after_them() {
    let token = "dG9rZW5WYWx1ZTEyMzQ1Njc4OTAxMjM0NTY3ODkw";
    assert_eq!(
        redact(&format!("Bearer {token},")).text,
        format!("Bearer {token},")
    );
    assert_eq!(redact(token).text, "[REDACTED-SECRET-1]");
    assert_eq!(redact(&format!("{token}!")).text, format!("{token}!"));
}

#[test]
fn common_tlds_are_sorted_for_binary_search() {
    assert!(COMMON_TLDS.windows(2).all(|w| w[0] < w[1]));
}
