//! Which URLs a request may go to (D4).

use std::fmt;
use std::net::IpAddr;

use url::{Host, Url};

use super::error::HostError;
use crate::{DomainRule, Manifest};

/// How a URL was allowed. It decides how strict the rest of the checks are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Matched `network.domains` of the manifest: `https` only, port 443 only, public addresses only.
    Manifest,
    /// Matched an address the user typed into a plugin setting: any port, `http` allowed,
    /// private addresses allowed (a home server is the point).
    Entered,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Entered {
    /// The typed host without a leading `www.`; matches itself and all its subdomains.
    Name(String),
    /// A typed IP address; matches only itself.
    Address(IpAddr),
}

/// The set of hosts one caller (a plugin call, or the app itself) may reach.
///
/// Built from the manifest's `network.domains` plus the hosts of `url` settings with
/// `network = true` (the address typed by the user and, per D4, all its subdomains).
#[derive(Clone, Debug, Default)]
pub struct AllowList {
    rules: Vec<DomainRule>,
    entered: Vec<Entered>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AllowListError {
    /// An entry of `network.domains` that does not parse as a [`DomainRule`].
    Domain(String),
    InvalidUrl(String),
    UnsupportedScheme(String),
    HasCredentials,
    NoHost,
}

impl fmt::Display for AllowListError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Domain(reason) => write!(f, "invalid domain rule: {reason}"),
            Self::InvalidUrl(url) => write!(f, "not a valid URL: {url}"),
            Self::UnsupportedScheme(scheme) => {
                write!(
                    f,
                    "only http and https addresses are supported, not {scheme}"
                )
            }
            Self::HasCredentials => {
                f.write_str("an address must not contain a user name or password")
            }
            Self::NoHost => f.write_str("the address has no host"),
        }
    }
}

impl std::error::Error for AllowListError {}

impl AllowList {
    pub fn new() -> Self {
        Self::default()
    }

    /// A plugin's list: its manifest domains plus the addresses entered in its `url` settings
    /// with `network = true`. Empty entries (setting not filled in yet) are skipped.
    pub fn for_manifest(
        manifest: &Manifest,
        entered_urls: &[&str],
    ) -> Result<Self, AllowListError> {
        let mut list = Self::new();
        for pattern in &manifest.network.domains {
            list.allow_domain(pattern)?;
        }
        for url in entered_urls {
            list.allow_entered_url(url)?;
        }
        Ok(list)
    }

    /// Allows `example.org` or `*.example.org` (manifest syntax) with [`Access::Manifest`] strictness.
    pub fn allow_domain(&mut self, pattern: &str) -> Result<(), AllowListError> {
        let rule = DomainRule::parse(pattern).map_err(AllowListError::Domain)?;
        self.rules.push(rule);
        Ok(())
    }

    /// Allows the host of an address the user typed, and its subdomains (D4). A leading `www.`
    /// is dropped, so `www.example.org` allows `example.org` and `static.example.org` too.
    /// An IP address allows only itself. An empty string is ignored.
    pub fn allow_entered_url(&mut self, input: &str) -> Result<(), AllowListError> {
        let input = input.trim();
        if input.is_empty() {
            return Ok(());
        }
        let url = Url::parse(input).map_err(|_| AllowListError::InvalidUrl(input.to_owned()))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(AllowListError::UnsupportedScheme(url.scheme().to_owned()));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(AllowListError::HasCredentials);
        }
        let entry = match url.host() {
            Some(Host::Domain(name)) => {
                let name = name.trim_end_matches('.');
                let base = match name.strip_prefix("www.") {
                    Some(rest) if rest.contains('.') => rest,
                    _ => name,
                };
                Entered::Name(base.to_owned())
            }
            Some(Host::Ipv4(ip)) => Entered::Address(ip.into()),
            Some(Host::Ipv6(ip)) => Entered::Address(ip.into()),
            None => return Err(AllowListError::NoHost),
        };
        if !self.entered.contains(&entry) {
            self.entered.push(entry);
        }
        Ok(())
    }

    /// Whether `host` may be contacted at all. The scheme is not known here, so the host is
    /// checked as for an `https` address; it is what `parse_search_page` and `merge_details`
    /// ask about a `cover_url`. The full address is checked again when it is fetched.
    pub fn allows_host(&self, host: &str) -> bool {
        Url::parse(&format!("https://{host}/"))
            .ok()
            .is_some_and(|url| self.check(&url).is_ok())
    }

    /// Checks scheme, credentials and host of `url`; called for the first URL and for every
    /// redirect target. Failures are `BlockedDomain`.
    pub fn check(&self, url: &Url) -> Result<Access, HostError> {
        if !matches!(url.scheme(), "http" | "https") {
            return Err(HostError::blocked(format!(
                "scheme {} is not allowed",
                url.scheme()
            )));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(HostError::blocked("credentials in a URL are not allowed"));
        }
        let host = url
            .host()
            .ok_or_else(|| HostError::blocked("the URL has no host"))?;
        if self.entered.iter().any(|entry| entry.matches(&host)) {
            return Ok(Access::Entered);
        }
        let Host::Domain(name) = host else {
            return Err(HostError::blocked(format!(
                "address {} is not allowed",
                url.host_str().unwrap_or_default()
            )));
        };
        if !self.rules.iter().any(|rule| rule.matches(name)) {
            return Err(HostError::blocked(format!(
                "host {name} is not in the list of allowed domains"
            )));
        }
        if url.scheme() != "https" {
            return Err(HostError::blocked(format!(
                "host {name} may be reached over https only"
            )));
        }
        if url.port_or_known_default() != Some(443) {
            return Err(HostError::blocked(format!(
                "host {name} may be reached on port 443 only"
            )));
        }
        Ok(Access::Manifest)
    }
}

impl Entered {
    fn matches(&self, host: &Host<&str>) -> bool {
        match (self, host) {
            (Self::Name(base), Host::Domain(name)) => {
                let name = name.trim_end_matches('.');
                name == base
                    || name
                        .strip_suffix(base.as_str())
                        .is_some_and(|prefix| prefix.ends_with('.'))
            }
            (Self::Address(wanted), Host::Ipv4(ip)) => *wanted == IpAddr::V4(*ip),
            (Self::Address(wanted), Host::Ipv6(ip)) => *wanted == IpAddr::V6(*ip),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HostErrorKind;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    fn manifest_list(domains: &[&str]) -> AllowList {
        let mut list = AllowList::new();
        for domain in domains {
            list.allow_domain(domain).unwrap();
        }
        list
    }

    fn entered_list(address: &str) -> AllowList {
        let mut list = AllowList::new();
        list.allow_entered_url(address).unwrap();
        list
    }

    fn blocked(list: &AllowList, target: &str) -> String {
        let error = list.check(&url(target)).unwrap_err();
        assert_eq!(error.kind, HostErrorKind::BlockedDomain, "{target}");
        error.message
    }

    #[test]
    fn manifest_domains_are_https_on_port_443_only() {
        let list = manifest_list(&["example.org", "*.cdn.example.net"]);
        assert_eq!(
            list.check(&url("https://example.org/a?b=1")),
            Ok(Access::Manifest)
        );
        assert_eq!(
            list.check(&url("https://EXAMPLE.org:443/")),
            Ok(Access::Manifest)
        );
        assert_eq!(
            list.check(&url("https://a.b.cdn.example.net/x")),
            Ok(Access::Manifest)
        );
        assert!(blocked(&list, "http://example.org/").contains("https"));
        assert!(blocked(&list, "https://example.org:8443/").contains("443"));
        assert!(blocked(&list, "https://www.example.org/").contains("not in the list"));
        assert!(blocked(&list, "https://cdn.example.net/").contains("not in the list"));
        assert!(blocked(&list, "https://evilexample.org/").contains("not in the list"));
        assert!(blocked(&list, "https://example.org.evil.com/").contains("not in the list"));
    }

    #[test]
    fn a_host_is_asked_about_without_its_scheme() {
        let list = manifest_list(&["example.org", "*.cdn.example.net"]);
        assert!(list.allows_host("example.org"));
        assert!(list.allows_host("a.cdn.example.net"));
        assert!(!list.allows_host("cdn.example.net"));
        assert!(!list.allows_host("evil.test"));
        assert!(!list.allows_host("not a host"));
        // An address typed by the user opens its host and subdomains, whatever the port.
        let entered = entered_list("http://nas.local:8080/opds");
        assert!(entered.allows_host("nas.local"));
        assert!(entered.allows_host("img.nas.local"));
        assert!(!entered.allows_host("example.org"));
    }

    #[test]
    fn schemes_other_than_http_and_https_are_blocked() {
        let list = manifest_list(&["example.org"]);
        for target in [
            "ftp://example.org/x",
            "file:///etc/passwd",
            "data:text/plain,hi",
            "javascript:alert(1)",
        ] {
            let message = blocked(&list, target);
            assert!(
                message.contains("scheme") || message.contains("host"),
                "{message}"
            );
        }
    }

    #[test]
    fn credentials_in_a_url_are_blocked() {
        let list = manifest_list(&["example.org"]);
        assert!(blocked(&list, "https://user:pw@example.org/").contains("credentials"));
        assert!(blocked(&list, "https://user@example.org/").contains("credentials"));
    }

    #[test]
    fn ip_addresses_never_match_manifest_domains() {
        let list = manifest_list(&["example.org"]);
        assert!(blocked(&list, "https://93.184.216.34/").contains("not allowed"));
        assert!(blocked(&list, "https://[::1]/").contains("not allowed"));
        assert!(blocked(&list, "http://127.0.0.1:8080/").contains("not allowed"));
    }

    #[test]
    fn an_entered_host_allows_itself_and_its_subdomains_over_any_scheme_and_port() {
        let list = entered_list("http://books.example.org:8080/opds");
        for target in [
            "http://books.example.org:8080/opds",
            "https://books.example.org/",
            "http://BOOKS.example.org/x",
            "http://static.books.example.org:9/y",
            "http://a.b.books.example.org/",
        ] {
            assert_eq!(list.check(&url(target)), Ok(Access::Entered), "{target}");
        }
        assert!(blocked(&list, "http://example.org/").contains("not in the list"));
        assert!(blocked(&list, "http://other.example.org/").contains("not in the list"));
        assert!(blocked(&list, "http://evilbooks.example.org/").contains("not in the list"));
    }

    #[test]
    fn a_leading_www_is_dropped_from_an_entered_host() {
        let list = entered_list("https://www.example.org/opds");
        for target in [
            "https://www.example.org/",
            "https://example.org/",
            "https://static.example.org/",
            "https://a.static.example.org/",
        ] {
            assert_eq!(list.check(&url(target)), Ok(Access::Entered), "{target}");
        }
        assert!(blocked(&list, "https://example.com/").contains("not in the list"));
        assert!(blocked(&list, "https://example.org.evil.com/").contains("not in the list"));
    }

    #[test]
    fn a_sibling_second_level_domain_needs_the_manifest() {
        let mut list = entered_list("http://mirror.example.org/");
        assert!(blocked(&list, "http://mirror.example.com/").contains("not in the list"));
        assert!(blocked(&list, "https://cdn.example.net/f").contains("not in the list"));
        list.allow_domain("*.example.net").unwrap();
        assert_eq!(
            list.check(&url("https://cdn.example.net/f")),
            Ok(Access::Manifest)
        );
        assert!(blocked(&list, "http://cdn.example.net/f").contains("https"));
    }

    #[test]
    fn a_www_prefix_is_kept_when_nothing_would_be_left_of_the_name() {
        let list = entered_list("http://www.local/");
        assert_eq!(list.check(&url("http://www.local/")), Ok(Access::Entered));
        assert!(blocked(&list, "http://local/").contains("not in the list"));
    }

    #[test]
    fn an_entered_ip_address_allows_only_itself() {
        let list = entered_list("http://192.168.1.5:8080/opds");
        assert_eq!(
            list.check(&url("http://192.168.1.5:8080/x")),
            Ok(Access::Entered)
        );
        assert_eq!(list.check(&url("http://192.168.1.5/")), Ok(Access::Entered));
        assert!(blocked(&list, "http://192.168.1.6/").contains("not allowed"));
        assert!(blocked(&list, "http://127.0.0.1/").contains("not allowed"));
        let v6 = entered_list("http://[fd00::5]:8080/");
        assert_eq!(v6.check(&url("http://[fd00::5]/")), Ok(Access::Entered));
        assert!(blocked(&v6, "http://[fd00::6]/").contains("not allowed"));
    }

    #[test]
    fn an_entered_single_label_host_is_accepted() {
        let list = entered_list("http://nas:8080/");
        assert_eq!(list.check(&url("http://nas:8080/")), Ok(Access::Entered));
        assert_eq!(list.check(&url("http://files.nas/")), Ok(Access::Entered));
    }

    #[test]
    fn invalid_entered_addresses_are_rejected_and_empty_ones_ignored() {
        let mut list = AllowList::new();
        assert_eq!(list.allow_entered_url(""), Ok(()));
        assert_eq!(list.allow_entered_url("   "), Ok(()));
        assert!(matches!(
            list.allow_entered_url("not a url"),
            Err(AllowListError::InvalidUrl(_))
        ));
        assert!(matches!(
            list.allow_entered_url("ftp://example.org/"),
            Err(AllowListError::UnsupportedScheme(_))
        ));
        assert_eq!(
            list.allow_entered_url("https://u:p@example.org/"),
            Err(AllowListError::HasCredentials)
        );
        assert!(matches!(
            list.allow_domain("*.org"),
            Err(AllowListError::Domain(_))
        ));
        assert!(matches!(
            list.allow_domain("127.0.0.1"),
            Err(AllowListError::Domain(_))
        ));
    }

    #[test]
    fn for_manifest_combines_domains_and_entered_addresses() {
        let manifest = Manifest::parse(
            "id = \"demo\"\nname = \"Demo\"\nversion = \"1.0.0\"\napi_version = 1\n\
             provides = [\"search\"]\n[network]\ndomains = [\"api.example.org\"]\n",
        )
        .unwrap();
        let list = AllowList::for_manifest(&manifest, &["http://192.168.0.2:8080", ""]).unwrap();
        assert_eq!(
            list.check(&url("https://api.example.org/")),
            Ok(Access::Manifest)
        );
        assert_eq!(
            list.check(&url("http://192.168.0.2:8080/opds")),
            Ok(Access::Entered)
        );
    }
}
