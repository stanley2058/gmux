//! Build identity helpers.

pub const BASE_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn version() -> String {
    BASE_VERSION.to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn stable_version_defaults_to_cargo_version() {
        assert!(!super::version().is_empty());
    }
}
