//! Build identity helpers.

pub const BASE_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BUILD_COMMIT: &str = env!("GMUX_BUILD_COMMIT");

pub fn version() -> String {
    BASE_VERSION.to_string()
}

pub fn build_commit() -> &'static str {
    BUILD_COMMIT
}

#[cfg(test)]
mod tests {
    #[test]
    fn stable_version_defaults_to_cargo_version() {
        assert!(!super::version().is_empty());
    }

    #[test]
    fn build_commit_is_available() {
        assert!(!super::build_commit().is_empty());
    }
}
