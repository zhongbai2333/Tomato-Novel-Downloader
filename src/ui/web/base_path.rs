use anyhow::{Result, bail};

/// A validated URL prefix, empty for the root deployment and otherwise without a trailing slash.
#[derive(Clone, Debug, Default)]
pub(crate) struct WebBasePath(String);

impl WebBasePath {
    pub(crate) fn parse(raw: &str) -> Result<Self> {
        let path = raw.trim();
        if path.is_empty() || path == "/" {
            return Ok(Self::default());
        }

        let path = path.strip_suffix('/').unwrap_or(path);
        let valid = path.starts_with('/')
            && path[1..].split('/').all(|segment| {
                !segment.is_empty()
                    && segment != "."
                    && segment != ".."
                    && segment.bytes().all(|b| {
                        b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~')
                    })
            });
        if !valid {
            bail!(
                "invalid TOMATO_WEB_BASE_PATH: use '/' or a path such as '/tomato' or '/apps/tomato'; segments may contain only ASCII letters, digits and -_.~, and must not be empty, '.' or '..'"
            );
        }
        Ok(Self(path.to_string()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// `path` is an application route beginning with `/`.
    pub(crate) fn url(&self, path: &str) -> String {
        format!("{}{path}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_root_and_nested_paths() {
        for input in ["", " ", "/"] {
            let path = WebBasePath::parse(input).unwrap();
            assert_eq!(path.as_str(), "");
            assert_eq!(path.url("/"), "/");
            assert_eq!(path.url("/api/status"), "/api/status");
        }
        for input in ["/apps/tomato", "/apps/tomato/", " /apps/tomato/ "] {
            let path = WebBasePath::parse(input).unwrap();
            assert_eq!(path.as_str(), "/apps/tomato");
            assert_eq!(path.url("/api/status"), "/apps/tomato/api/status");
        }
        assert!(WebBasePath::parse("/tomato-v2_4.16~test").is_ok());
    }

    #[test]
    fn rejects_ambiguous_or_unsafe_prefixes() {
        for input in [
            "tomato",
            "//host",
            "/tomato//",
            "/a//b",
            "/a/../b",
            "/./a",
            "/a%2fb",
            "/a?b",
            "/a#b",
            "/a\\b",
            "/:id",
            "/*path",
            "/a\"b",
            "/a;b",
            "/a\nb",
            "/番茄",
        ] {
            assert!(WebBasePath::parse(input).is_err(), "accepted {input:?}");
        }
    }
}
