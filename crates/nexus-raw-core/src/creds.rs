//! Credentials resolver: `-u` flag first, then env; values never printed (spec §4).

use base64::Engine as _;

use crate::error::Error;

/// The value of the `Authorization` header (e.g. `Basic <b64>`).
#[derive(Debug, Clone)]
pub struct Creds {
    pub header: String,
}

/// Order: `-u user:pass`, then `NXR_AUTH` (base64 `user:pass`),
/// then `NXR_USERNAME` + `NXR_PASSWORD`.
pub fn resolve(explicit: Option<(&str, &str)>) -> Result<Option<Creds>, Error> {
    if let Some((user, pass)) = explicit {
        return Ok(Some(Creds {
            header: format!("Basic {}", basic(user, pass)),
        }));
    }
    if let Ok(v) = std::env::var("NXR_AUTH") {
        if !v.is_empty() {
            return Ok(Some(Creds {
                header: format!("Basic {v}"),
            }));
        }
    }
    let user = std::env::var("NXR_USERNAME").ok().filter(|s| !s.is_empty());
    let pass = std::env::var("NXR_PASSWORD").ok().filter(|s| !s.is_empty());
    match (user, pass) {
        (Some(u), Some(p)) => Ok(Some(Creds {
            header: format!("Basic {}", basic(&u, &p)),
        })),
        (Some(_), None) | (None, Some(_)) => Err(Error::misuse(
            "NXR_USERNAME and NXR_PASSWORD must be set together",
        )),
        (None, None) => Ok(None),
    }
}

/// `user:pass` → standard base64.
pub fn basic(user: &str, pass: &str) -> String {
    use base64::engine::general_purpose::STANDARD;
    STANDARD.encode(format!("{user}:{pass}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    // One env at a time: these tests mutate NXR_AUTH and run in parallel.
    static ENV_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    #[test]
    fn basic_encoding() {
        assert_eq!(basic("user", "pass"), "dXNlcjpwYXNz");
    }

    #[test]
    fn explicit_wins() {
        let _env = ENV_LOCK.lock();
        std::env::set_var("NXR_AUTH", "envB64");
        let creds = resolve(Some(("user", "pass"))).unwrap().unwrap();
        assert_eq!(creds.header, "Basic dXNlcjpwYXNz");
        std::env::remove_var("NXR_AUTH");
    }
}
