//! Secret-only redaction for durable previews; preserves paths and hostnames.
pub const GITHUB_TOKEN_PATTERN: &str = r"\b(?:ghp|gho|ghs|ghu|ghr)_[A-Za-z0-9_]{36,}\b";
pub const AWS_KEY_PATTERN: &str = r"\b(?:AKIA|ASIA)[A-Z0-9]{16}\b";
use regex::Regex;
use std::sync::LazyLock;

// These are the secret-only recognition rules used by the issue-report redactor.
// Deliberately exclude its hostname, path, email, long-hex and generic base64 rules.
static SECRETS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?s)-----BEGIN (?:[A-Z]+ )?PRIVATE KEY-----.*?(?:-----END (?:[A-Z]+ )?PRIVATE KEY-----|$)",
        AWS_KEY_PATTERN,
        GITHUB_TOKEN_PATTERN,
        r"(?i-u)\b(?:sk|pk|rk|ak|key|token|secret)[-_][a-zA-Z0-9_\-]{20,}\b",
        r"(?i-u)\bBearer[ \t]+[A-Za-z0-9_\-.~+/]+=*",
        r#"(?i-u)\b[A-Za-z_]*(?:PASSWORD|SECRET|TOKEN|API_KEY|APIKEY|PRIVATE_KEY|ACCESS_KEY|AUTH|COOKIE)[A-Za-z_]*(?u-i)["']?\s*[=:]\s*(?:"[^"\n]*"|'[^'\n]*'|`[^`\n]*`|[^\s"'`]+)"#,
        r"[a-zA-Z][a-zA-Z0-9+.-]*://[^\s/@]+:[^\s/@]+@",
    ].into_iter().map(|pattern| Regex::new(pattern).expect("constant credential pattern")).collect()
});

pub fn redact_secrets(text: &str) -> String {
    SECRETS.iter().fold(text.to_owned(), |text, pattern| {
        pattern.replace_all(&text, "[REDACTED]").into_owned()
    })
}
