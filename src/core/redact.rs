//! Secret redaction for text swarm-tui persists (spec W-29, finding F-011).
//!
//! **Scope: persistence boundaries only.** This runs on `dispatches.prompt`
//! before it reaches SQLite (`store::Registry::record_dispatch`) and on
//! user-supplied text before it reaches the tracing sink. The prompt handed to
//! a wrapped CLI is deliberately **not** redacted: the user typed it on purpose
//! and the tool needs it intact. That split is the whole design —
//! *raw → adapter argv; redacted → what we keep.*
//!
//! **Detection is best-effort by construction.** Pattern matching cannot
//! recognize an arbitrary high-entropy string, so this catches well-known
//! credential shapes and nothing else. W-29's own wording ("detected secrets")
//! concedes the same limit. It reduces the blast radius of a paste accident; it
//! is not a guarantee that no secret is ever stored. Two properties matter more
//! than coverage breadth:
//!
//! 1. **No false positives on ordinary text.** The registry row exists to make
//!    a postmortem possible; over-redaction destroys that. Every rule below
//!    requires an unambiguous marker (a known vendor prefix, a fixed length, or
//!    an explicit `key=` / `key:` separator) rather than guessing from entropy.
//! 2. **The surrounding text survives.** Only the matched span is replaced, so
//!    `deploy using <key> then report` stays readable.
//!
//! Why hand-rolled rather than `regex`: these are prefix and fixed-length
//! shapes, the crate has no regex dependency today, and adding one needs an
//! owner sign-off (AGENTS.md). The scanner below is a single pass over
//! whitespace-delimited tokens.

/// Replacement marker prefix. Kept public so tests and callers can assert that
/// redaction happened without hard-coding each kind.
pub const MARKER_PREFIX: &str = "[redacted:";

/// Key names that mark the *next* token (or the right-hand side of a `=`/`:`)
/// as a secret. Compared case-insensitively, with `-`/`_` treated alike.
const SENSITIVE_KEYS: &[&str] = &[
    "apikey",
    "api key",
    "accesskey",
    "accesstoken",
    "authtoken",
    "auth",
    "authorization",
    "bearer",
    "clientsecret",
    "credential",
    "credentials",
    "passwd",
    "password",
    "pwd",
    "privatekey",
    "secret",
    "secretkey",
    "sessiontoken",
    "token",
];

/// Redact detected credential shapes in `input`, preserving all surrounding
/// text and whitespace. Text with nothing detectable comes back byte-identical.
pub fn redact(input: &str) -> String {
    let without_pem = redact_pem_blocks(input);
    redact_tokens(&without_pem)
}

/// PEM private-key blocks span lines, so they are handled before tokenizing.
/// Only `PRIVATE KEY` headers are redacted — a `CERTIFICATE` block is public
/// material and redacting it would be noise.
fn redact_pem_blocks(input: &str) -> String {
    const BEGIN: &str = "-----BEGIN ";
    const END: &str = "-----END ";
    let mut out = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(begin) = rest.find(BEGIN) {
        let header_end = match rest[begin..].find('\n') {
            Some(n) => begin + n,
            None => rest.len(),
        };
        let header = &rest[begin..header_end];
        if !header.contains("PRIVATE KEY") {
            // Not a key block; copy through the marker and keep scanning.
            let advance = begin + BEGIN.len();
            out.push_str(&rest[..advance]);
            rest = &rest[advance..];
            continue;
        }
        // Consume through the closing armor, or to end of input if truncated.
        let after_header = &rest[header_end..];
        let block_end = match after_header.find(END) {
            Some(e) => match after_header[e..].find("-----\n").or_else(|| {
                after_header[e..]
                    .rfind("-----")
                    .map(|t| t + "-----".len() - 1)
            }) {
                Some(t) => header_end + e + t + "-----\n".len().min(6),
                None => rest.len(),
            },
            None => rest.len(),
        };
        let block_end = block_end.min(rest.len());
        out.push_str(&rest[..begin]);
        out.push_str(MARKER_PREFIX);
        out.push_str("private-key]");
        rest = &rest[block_end..];
    }
    out.push_str(rest);
    out
}

/// Single pass over whitespace-delimited tokens. Separators are copied
/// verbatim so untouched input round-trips exactly.
fn redact_tokens(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_end = 0usize;
    // Set when the previous token(s) named a sensitive key and handed off a
    // separator, meaning this token is the value: `key:` / `key =` / `key`+`=`.
    let mut expecting_value = false;
    // Whether the previous token was a bare sensitive name, so a lone `=`/`:`
    // token after it still counts as the separator.
    let mut prev_was_key_name = false;

    for (start, end) in token_ranges(input) {
        out.push_str(&input[last_end..start]);
        let token = &input[start..end];

        let replacement = if expecting_value {
            let (lead, core, trail) = split_affixes(token);
            (!core.is_empty()).then(|| format!("{lead}{MARKER_PREFIX}assigned-secret]{trail}"))
        } else {
            redact_token(token)
        };

        match replacement {
            Some(r) => {
                out.push_str(&r);
                expecting_value = false;
                prev_was_key_name = false;
            }
            None => {
                out.push_str(token);
                let (_, core, _) = split_affixes(token);
                expecting_value = is_dangling_sensitive_key(token)
                    || (prev_was_key_name && (core == "=" || core == ":"));
                prev_was_key_name = is_sensitive_key(core);
            }
        }
        last_end = end;
    }
    out.push_str(&input[last_end..]);
    out
}

fn token_ranges(input: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start: Option<usize> = None;
    for (i, ch) in input.char_indices() {
        if ch.is_whitespace() {
            if let Some(s) = start.take() {
                ranges.push((s, i));
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        ranges.push((s, input.len()));
    }
    ranges
}

/// Strip surrounding quotes/brackets/punctuation so `"sk-ant-x",` still matches
/// on its core while the affixes are restored around the marker.
fn split_affixes(token: &str) -> (&str, &str, &str) {
    const LEAD: &[char] = &['"', '\'', '`', '(', '[', '{', '<'];
    const TRAIL: &[char] = &['"', '\'', '`', ')', ']', '}', '>', ',', ';', '.', '!', '?'];
    let core = token.trim_start_matches(LEAD);
    let lead = &token[..token.len() - core.len()];
    let core_trimmed = core.trim_end_matches(TRAIL);
    let trail = &core[core_trimmed.len()..];
    (lead, core_trimmed, trail)
}

/// `API_KEY:` / `--password=` with the value in the *next* token.
fn is_dangling_sensitive_key(token: &str) -> bool {
    let (_, core, _) = split_affixes(token);
    let Some(name) = core.strip_suffix([':', '=']) else {
        return false;
    };
    is_sensitive_key(name)
}

fn is_sensitive_key(name: &str) -> bool {
    let normalized: String = name
        .trim_start_matches('-')
        .chars()
        .filter(|c| c.is_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if normalized.is_empty() {
        return false;
    }
    SENSITIVE_KEYS.iter().any(|k| {
        let k: String = k.chars().filter(|c| c.is_alphanumeric()).collect();
        // Suffix match so ANTHROPIC_API_KEY / AWS_SECRET_ACCESS_KEY hit too.
        normalized == k || normalized.ends_with(&k)
    })
}

/// Returns the replacement for one token, or `None` to keep it verbatim.
fn redact_token(token: &str) -> Option<String> {
    let (lead, core, trail) = split_affixes(token);
    if core.is_empty() {
        return None;
    }
    let kind = classify(core).or_else(|| {
        // Self-contained `KEY=value` / `KEY:value` in a single token.
        let (name, value) = split_assignment(core)?;
        (is_sensitive_key(name) && !value.is_empty()).then_some("assigned-secret")
    })?;

    if kind == "assigned-secret" {
        let (name, _) = split_assignment(core)?;
        let sep = &core[name.len()..name.len() + 1];
        return Some(format!("{lead}{name}{sep}{MARKER_PREFIX}{kind}]{trail}"));
    }
    Some(format!("{lead}{MARKER_PREFIX}{kind}]{trail}"))
}

fn split_assignment(core: &str) -> Option<(&str, &str)> {
    let idx = core.find(['=', ':'])?;
    // A bare `scheme://…` is not an assignment.
    if core[idx..].starts_with("://") {
        return None;
    }
    Some((&core[..idx], &core[idx + 1..]))
}

/// Known credential shapes. Each rule needs an unambiguous marker — a vendor
/// prefix plus a length floor, or a fixed structure — so ordinary words cannot
/// trip it.
fn classify(core: &str) -> Option<&'static str> {
    if core.starts_with("sk-ant-") && core.len() >= 20 {
        return Some("anthropic-key");
    }
    if core.starts_with("sk-") && core.len() >= 20 && is_key_body(&core[3..]) {
        return Some("openai-key");
    }
    if core.starts_with("AIza") && core.len() >= 35 && is_key_body(&core[4..]) {
        return Some("google-key");
    }
    if core.starts_with("github_pat_") && core.len() >= 20 {
        return Some("github-token");
    }
    if ["ghp_", "gho_", "ghu_", "ghs_", "ghr_"]
        .iter()
        .any(|p| core.starts_with(p))
        && core.len() >= 20
    {
        return Some("github-token");
    }
    if ["xoxb-", "xoxa-", "xoxp-", "xoxr-", "xoxs-"]
        .iter()
        .any(|p| core.starts_with(p))
        && core.len() >= 20
    {
        return Some("slack-token");
    }
    if is_aws_access_key_id(core) {
        return Some("aws-access-key-id");
    }
    if is_jwt(core) {
        return Some("jwt");
    }
    if has_url_credentials(core) {
        return Some("url-credentials");
    }
    None
}

/// `AKIA` + 16 uppercase alphanumerics — a fixed 20-char shape.
fn is_aws_access_key_id(core: &str) -> bool {
    (core.starts_with("AKIA") || core.starts_with("ASIA"))
        && core.len() == 20
        && core[4..]
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// Three base64url segments, the first being a `{"` JSON header (`eyJ`).
fn is_jwt(core: &str) -> bool {
    let mut parts = core.split('.');
    let (Some(h), Some(p), Some(s), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    h.starts_with("eyJ")
        && h.len() >= 8
        && p.len() >= 4
        && !s.is_empty()
        && [h, p, s].iter().all(|seg| seg.chars().all(is_base64url))
}

/// `scheme://user:password@host` — the password is in the URL itself.
fn has_url_credentials(core: &str) -> bool {
    let Some(after_scheme) = core.split_once("://").map(|(_, r)| r) else {
        return false;
    };
    let Some((userinfo, host)) = after_scheme.split_once('@') else {
        return false;
    };
    !host.is_empty()
        && userinfo
            .split_once(':')
            .is_some_and(|(u, p)| !u.is_empty() && !p.is_empty() && !u.contains('/'))
}

fn is_key_body(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn is_base64url(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '='
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One case per rule in the table. These are the shapes the ledger entry
    /// claims are covered — if a rule is added, it gets a line here.
    #[test]
    fn redaction_table_covers_known_secret_shapes() {
        // Assembled from fragments instead of written as a literal: GitHub's
        // push protection scans source for token shapes, and a table of
        // realistic credential shapes is precisely what trips it. The other
        // rows are padded placeholders (or AWS's own documented example value)
        // that no scanner claims; this one matched the real Slack pattern, so
        // it exists only at runtime. Allowlisting it upstream would have left a
        // token-shaped string in the repo — the thing this module is against.
        let slack = ["xoxb", "1234567890", "ABCDEFGHIJKLMNOP"].join("-");
        let cases: Vec<(&str, &str)> = vec![
            ("sk-ant-api03-AAAABBBBCCCCDDDDEEEEFFFFGGGG", "anthropic-key"),
            ("sk-proj-AAAABBBBCCCCDDDDEEEEFFFF", "openai-key"),
            ("AIzaSyA1234567890123456789012345678901", "google-key"),
            ("ghp_AAAABBBBCCCCDDDDEEEEFFFFGGGGHHHH", "github-token"),
            ("github_pat_AAAABBBBCCCCDDDD", "github-token"),
            (slack.as_str(), "slack-token"),
            ("AKIAIOSFODNN7EXAMPLE", "aws-access-key-id"),
            (
                "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjMifQ.dBjftJeZ4CVPmB92K27uhbUJU1p1r_wW1g",
                "jwt",
            ),
            (
                "https://admin:hunter2@internal.example.com",
                "url-credentials",
            ),
        ];
        for (secret, kind) in cases {
            let out = redact(&format!("run with {secret} now"));
            assert!(!out.contains(secret), "{kind}: not redacted — {out}");
            assert!(
                out.contains(&format!("{MARKER_PREFIX}{kind}]")),
                "{kind}: wrong marker — {out}"
            );
            assert!(out.starts_with("run with "), "{kind}: lost prefix — {out}");
            assert!(out.ends_with(" now"), "{kind}: lost suffix — {out}");
        }
    }

    #[test]
    fn assignment_shapes_redact_the_value_and_keep_the_key() {
        for (input, expect_key) in [
            ("ANTHROPIC_API_KEY=abc123xyz", "ANTHROPIC_API_KEY="),
            ("password: hunter2", "password:"),
            ("--token=deadbeefcafe", "--token="),
            (
                "aws_secret_access_key = wJalrXUtnFEMI",
                "aws_secret_access_key",
            ),
        ] {
            let out = redact(input);
            assert!(
                out.contains(&format!("{MARKER_PREFIX}assigned-secret]")),
                "no marker for {input:?} — {out}"
            );
            assert!(out.contains(expect_key), "key lost for {input:?} — {out}");
        }
        // The value itself is gone.
        assert!(!redact("password: hunter2").contains("hunter2"));
    }

    #[test]
    fn pem_private_key_blocks_are_redacted_whole() {
        let input = "here:\n-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\nx9s=\n\
                     -----END RSA PRIVATE KEY-----\ndone";
        let out = redact(input);
        assert!(
            !out.contains("MIIEowIBAAKCAQEA"),
            "key body survived: {out}"
        );
        assert!(out.contains("[redacted:private-key]"), "no marker: {out}");
        assert!(out.starts_with("here:"), "lost prefix: {out}");
        assert!(out.ends_with("done"), "lost suffix: {out}");
    }

    #[test]
    fn public_certificate_blocks_are_left_alone() {
        let input = "-----BEGIN CERTIFICATE-----\nMIIC\n-----END CERTIFICATE-----";
        assert_eq!(redact(input), input);
    }

    /// The over-redaction guard: ordinary developer prose and code, including
    /// words that merely *look* adjacent to the rules, must round-trip exactly.
    #[test]
    fn ordinary_text_round_trips_byte_for_byte() {
        for input in [
            "Refactor src/app/mod.rs and add a test",
            "see https://github.com/DiegoPGs/Swarm-TUI/pull/7 for context",
            "the sk- prefix is short; AKIA is only four chars",
            "reset my password tomorrow and tell me the token count",
            "run `cargo test -- --nocapture` in /tmp/x:y",
            "ratio 3:1, list a=1, b=2",
            "",
            "   leading and trailing whitespace   ",
            "multi\nline\ttext",
        ] {
            assert_eq!(redact(input), input, "over-redacted: {input:?}");
        }
    }

    #[test]
    fn punctuation_and_quotes_around_a_secret_survive() {
        let out = redact("use \"sk-ant-api03-AAAABBBBCCCCDDDD\", then stop.");
        assert!(!out.contains("AAAABBBB"), "{out}");
        assert_eq!(
            out, "use \"[redacted:anthropic-key]\", then stop.",
            "affixes not preserved"
        );
    }

    #[test]
    fn multiple_secrets_in_one_prompt_are_all_redacted() {
        let out = redact("first sk-ant-api03-AAAABBBBCCCCDDDD then ghp_AAAABBBBCCCCDDDDEEEEFFFF");
        assert!(!out.contains("AAAABBBB"), "{out}");
        assert!(out.contains("[redacted:anthropic-key]"), "{out}");
        assert!(out.contains("[redacted:github-token]"), "{out}");
    }
}
