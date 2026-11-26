use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};

use anyhow::{Context, Result, ensure};
use axum::http::header::{HOST, ORIGIN};
use axum::http::uri::Authority;
use axum::http::{HeaderMap, Method, Uri};
use reqwest::Url;

#[derive(Debug)]
pub(crate) struct RequestPolicy {
    allowed_hosts: HashSet<String>,
    trusted_proxies: HashSet<IpAddr>,
    accepts_literal_addresses: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct ClientAddress {
    pub(crate) ip: Option<IpAddr>,
    pub(crate) direct_loopback: bool,
    pub(crate) scheme: &'static str,
}

impl RequestPolicy {
    pub(crate) fn new(
        addresses: &[SocketAddr],
        extra_hosts: &str,
        trusted_proxies: &str,
    ) -> Result<Self> {
        let mut allowed_hosts = HashSet::from(["localhost".to_string()]);
        allowed_hosts.extend(addresses.iter().map(|address| address.ip().to_string()));
        if addresses.iter().any(|address| address.ip().is_loopback()) {
            allowed_hosts.extend(["127.0.0.1".to_string(), "::1".to_string()]);
        }
        for configured in comma_list(extra_hosts) {
            let authority: Authority = configured
                .parse()
                .with_context(|| format!("invalid TOMATO_WEB_ALLOWED_HOSTS entry: {configured}"))?;
            ensure!(
                !configured.contains('@') && authority.port().is_none(),
                "TOMATO_WEB_ALLOWED_HOSTS accepts host names without ports"
            );
            allowed_hosts.insert(normalized_host(&authority));
        }
        let trusted_proxies = comma_list(trusted_proxies)
            .map(|value| {
                value
                    .parse()
                    .with_context(|| format!("invalid TOMATO_WEB_TRUSTED_PROXIES entry: {value}"))
            })
            .collect::<Result<HashSet<_>>>()?;
        Ok(Self {
            allowed_hosts,
            trusted_proxies,
            accepts_literal_addresses: addresses
                .iter()
                .any(|address| address.ip().is_unspecified()),
        })
    }

    pub(crate) fn authority(&self, headers: &HeaderMap, uri: &Uri) -> Option<Authority> {
        let mut hosts = headers.get_all(HOST).iter();
        let authority: Authority = match hosts.next() {
            Some(value) => value.to_str().ok()?.parse().ok()?,
            None => uri.authority()?.clone(),
        };
        if hosts.next().is_some()
            || authority.as_str().contains('@')
            || uri.authority().is_some_and(|other| other != &authority)
        {
            return None;
        }
        let host = normalized_host(&authority);
        (self.allowed_hosts.contains(&host)
            || (self.accepts_literal_addresses && host.parse::<IpAddr>().is_ok()))
        .then_some(authority)
    }

    pub(crate) fn client_address(
        &self,
        peer: Option<SocketAddr>,
        authority: &Authority,
        headers: &HeaderMap,
    ) -> ClientAddress {
        let peer_ip = peer.map(|address| address.ip());
        if peer_ip.is_some_and(|ip| self.trusted_proxies.contains(&ip)) {
            let mut forwarded = headers.get_all("x-real-ip").iter();
            let parsed = forwarded
                .next()
                .and_then(|raw| raw.to_str().ok())
                .and_then(|value| value.parse().ok());
            let scheme = if headers
                .get("x-forwarded-proto")
                .is_some_and(|value| value == "https")
            {
                "https"
            } else {
                "http"
            };
            return ClientAddress {
                ip: if forwarded.next().is_none() {
                    parsed
                } else {
                    None
                },
                direct_loopback: false,
                scheme,
            };
        }
        let host = normalized_host(authority);
        let local_host =
            host == "localhost" || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback());
        ClientAddress {
            ip: peer_ip,
            direct_loopback: local_host && peer_ip.is_some_and(|ip| ip.is_loopback()),
            scheme: "http",
        }
    }
}

pub(crate) fn accepts_request_origin(
    method: &Method,
    authority: &Authority,
    headers: &HeaderMap,
    scheme: &str,
) -> bool {
    if method.is_safe() {
        return true;
    }
    if headers
        .get("sec-fetch-site")
        .is_some_and(|value| value == "cross-site")
    {
        return false;
    }
    let mut origins = headers.get_all(ORIGIN).iter();
    let Some(value) = origins.next() else {
        return true;
    };
    let Some(origin) = value.to_str().ok().and_then(|text| Url::parse(text).ok()) else {
        return false;
    };
    if origins.next().is_some()
        || origin.scheme() != scheme
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
    {
        return false;
    }
    Url::parse(&format!("{scheme}://{authority}"))
        .is_ok_and(|expected| origin.origin() == expected.origin())
}

fn comma_list(input: &str) -> impl Iterator<Item = &str> {
    input
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn normalized_host(authority: &Authority) -> String {
    let host = authority
        .host()
        .trim_matches(['[', ']'])
        .trim_end_matches('.');
    host.parse::<IpAddr>()
        .map_or_else(|_| host.to_ascii_lowercase(), |ip| ip.to_string())
}

#[cfg(test)]
mod tests {
    use super::{RequestPolicy, accepts_request_origin};
    use axum::http::{HeaderMap, Method, Uri};

    #[test]
    fn only_configured_hosts_and_matching_origins_are_accepted() {
        let listener = "127.0.0.1:18423".parse().unwrap();
        let policy = RequestPolicy::new(&[listener], "books.example.com", "127.0.0.1").unwrap();
        let mut headers = HeaderMap::new();
        let uri = Uri::from_static("/tomato/");
        headers.insert("host", "books.example.com".parse().unwrap());
        headers.insert("x-forwarded-proto", "https".parse().unwrap());
        headers.insert("x-real-ip", "192.0.2.10".parse().unwrap());
        headers.insert("origin", "https://books.example.com".parse().unwrap());
        let host = policy.authority(&headers, &uri).unwrap();
        let client = policy.client_address(Some(listener), &host, &headers);
        assert_eq!(client.ip, Some("192.0.2.10".parse().unwrap()));
        assert!(accepts_request_origin(
            &Method::POST,
            &host,
            &headers,
            client.scheme
        ));
        headers.insert("host", "unlisted.example.com".parse().unwrap());
        assert!(policy.authority(&headers, &uri).is_none());
    }
}
