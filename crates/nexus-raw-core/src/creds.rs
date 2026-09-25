//! Credentials resolver: env only, nothing ever printed.

use base64::Engine as _;

use crate::error::Error;

/// The value of the `Authorization` header (e.g. `Basic <b64>`).
#[derive(Debug, Clone)]
pub struct Creds {
    pub header: String,
}

/// Order: `NXR_<PROFILE>_AUTH`, `NXR_AUTH`, `NXR_USERNAME` + `NXR_PASSWORD`.
pub fn resolve(profile: Option<&str>) -> Result<Option<Creds>, Error> {
    if let Some(p) = profile {
        let key = format!("NXR_{}_AUTH", env_key(p));
        if let Ok(v) = std::env::var(&key) {
            if !v.is_empty() {
                return Ok(Some(Creds {
                    header: format!("Basic {v}"),
                }));
            }
        }
    }
    if let Ok(v) = std::env::var("NXR_AUTH") {
        if !v.is_empty() {
            return Ok(Some(Creds {
                header: format!("Basic {v}"),
            }));
        }
    }
    for (user_key, pass_key) in [("NXR_USERNAME", "NXR_PASSWORD")] {
        let user = std::env::var(user_key).ok().filter(|s| !s.is_empty());
        let pass = std::env::var(pass_key).ok().filter(|s| !s.is_empty());
        match (user, pass) {
            (Some(u), Some(p)) => {
                return Ok(Some(Creds {
                    header: format!("Basic {}", basic(&u, &p)),
                }));
            }
            (Some(_), None) | (None, Some(_)) => {
                return Err(Error::misuse(format!(
                    "{user_key} and {pass_key} must be set together"
                )));
            }
            (None, None) => {}
        }
    }
    Ok(None)
}

/// `user:pass` → standard base64.
pub fn basic(user: &str, pass: &str) -> String {
    use base64::engine::general_purpose::STANDARD;
    STANDARD.encode(format!("{user}:{pass}"))
}

/// Profile name → env segment: `my-lib` → `MY_LIB`.
pub fn env_key(profile: &str) -> String {
    profile
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_key_mapping() {
        assert_eq!(env_key("release"), "RELEASE");
        assert_eq!(env_key("my-lib"), "MY_LIB");
        assert_eq!(env_key("dev.2"), "DEV_2");
    }

    #[test]
    fn basic_encoding() {
        assert_eq!(basic("user", "pass"), "dXNlcjpwYXNz");
    }
}
