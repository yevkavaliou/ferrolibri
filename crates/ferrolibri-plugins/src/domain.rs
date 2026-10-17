use crate::limits::MAX_DOMAIN_CHARS;

/// An entry of `network.domains`: an exact host (`example.org`) or all subdomains of a domain
/// (`*.example.org`, which does not match `example.org` itself). Matching ignores ASCII case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DomainRule {
    Exact(String),
    /// Stores the domain without the `*.` prefix.
    Subdomains(String),
}

impl DomainRule {
    pub fn parse(pattern: &str) -> Result<Self, String> {
        if pattern.chars().count() > MAX_DOMAIN_CHARS {
            return Err(format!("must be at most {MAX_DOMAIN_CHARS} characters"));
        }
        let (wildcard, host) = match pattern.strip_prefix("*.") {
            Some(rest) => (true, rest),
            None => (false, pattern),
        };
        if host.contains('*') {
            return Err("may use '*' only as the leading '*.' prefix".into());
        }
        let host = host.to_ascii_lowercase();
        let labels: Vec<&str> = host.split('.').collect();
        if labels.len() < 2 {
            return Err("must contain at least two labels, e.g. example.org".into());
        }
        for label in &labels {
            let valid = !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
            if !valid {
                return Err("must be a plain ASCII host name (letters, digits, '-')".into());
            }
        }
        // `127.0.0.1` and friends are addresses, not names.
        if labels
            .last()
            .is_some_and(|l| l.bytes().all(|b| b.is_ascii_digit()))
        {
            return Err("must be a host name, not an IP address".into());
        }
        Ok(if wildcard {
            Self::Subdomains(host)
        } else {
            Self::Exact(host)
        })
    }

    pub fn matches(&self, host: &str) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        match self {
            Self::Exact(exact) => host == *exact,
            Self::Subdomains(domain) => host
                .strip_suffix(domain.as_str())
                .is_some_and(|prefix| prefix.ends_with('.') && prefix.len() > 1),
        }
    }
}
