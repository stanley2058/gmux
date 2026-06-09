use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UpdateRelease {
    pub version: String,
    pub tag: String,
    pub asset_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UpdateCheckResult {
    pub release: Option<UpdateRelease>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UpdateInstallSuccess {
    pub version: String,
    pub binary_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UpdateInstallResult {
    Success(UpdateInstallSuccess),
    Failed { message: String },
}

#[cfg(feature = "self-update")]
pub(crate) fn self_update_enabled() -> bool {
    true
}

#[cfg(not(feature = "self-update"))]
pub(crate) fn self_update_enabled() -> bool {
    false
}

pub(crate) fn check_latest_release(current_version: &str) -> UpdateCheckResult {
    match check_latest_release_inner(current_version) {
        Ok(release) => UpdateCheckResult {
            release,
            error: None,
        },
        Err(err) => UpdateCheckResult {
            release: None,
            error: Some(err),
        },
    }
}

fn check_latest_release_inner(current_version: &str) -> Result<Option<UpdateRelease>, String> {
    let asset_name = match platform_asset_name() {
        Some(asset_name) => asset_name,
        None => return Ok(None),
    };
    let output = curl_text("https://api.github.com/repos/stanley2058/gmux/releases/latest")?;
    latest_release_from_json(&output, current_version, asset_name.as_str())
}

pub(crate) fn install_release(release: UpdateRelease, binary_path: PathBuf) -> UpdateInstallResult {
    match install_release_inner(&release, binary_path) {
        Ok(success) => UpdateInstallResult::Success(success),
        Err(message) => UpdateInstallResult::Failed { message },
    }
}

fn install_release_inner(
    release: &UpdateRelease,
    binary_path: PathBuf,
) -> Result<UpdateInstallSuccess, String> {
    let commit = crate::build_info::build_commit();
    if commit == "unknown" || commit.is_empty() {
        return Err("build commit is unknown; cannot fetch pinned installer".to_string());
    }

    let script_url =
        format!("https://raw.githubusercontent.com/stanley2058/gmux/{commit}/install.sh");
    let script_path = temp_script_path();
    download_to(&script_url, &script_path)?;

    let output = Command::new("bash")
        .arg(&script_path)
        .env("GMUX_VERSION", &release.tag)
        .env("GMUX_INSTALL_BINARY_PATH", &binary_path)
        .env("GMUX_INSTALL_COMPLETIONS", "1")
        .env("GMUX_INSTALL_QUIET", "1")
        .output()
        .map_err(|err| format!("failed to run pinned installer: {err}"));
    let _ = std::fs::remove_file(&script_path);
    let output = output?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let message = if stderr.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        return Err(if message.is_empty() {
            format!("installer exited with {}", output.status)
        } else {
            message
        });
    }

    Ok(UpdateInstallSuccess {
        version: release.version.clone(),
        binary_path,
    })
}

fn curl_text(url: &str) -> Result<String, String> {
    let output = Command::new("curl")
        .arg("-fsSL")
        .arg("--retry")
        .arg("2")
        .arg("--connect-timeout")
        .arg("5")
        .arg("--max-time")
        .arg("15")
        .arg(url)
        .output()
        .map_err(|err| format!("failed to run curl: {err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("curl failed: {}", stderr.trim()));
    }
    String::from_utf8(output.stdout).map_err(|err| format!("release response was not utf-8: {err}"))
}

fn download_to(url: &str, path: &std::path::Path) -> Result<(), String> {
    let output = Command::new("curl")
        .arg("-fsSL")
        .arg("--retry")
        .arg("2")
        .arg("--connect-timeout")
        .arg("5")
        .arg("--max-time")
        .arg("30")
        .arg("-o")
        .arg(path)
        .arg(url)
        .output()
        .map_err(|err| format!("failed to run curl: {err}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "failed to download pinned installer: {}",
            stderr.trim()
        ))
    }
}

fn temp_script_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("gmux-install-{}-{nanos}.sh", std::process::id()))
}

fn platform_asset_name() -> Option<String> {
    let platform = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "linux-x86_64",
        ("macos", "aarch64") => "darwin-aarch64",
        _ => return None,
    };
    Some(format!("gmux-{platform}.tar.gz"))
}

fn latest_release_from_json(
    json: &str,
    current_version: &str,
    asset_name: &str,
) -> Result<Option<UpdateRelease>, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|err| format!("failed to parse release json: {err}"))?;
    let tag = value
        .get("tag_name")
        .and_then(|tag| tag.as_str())
        .ok_or_else(|| "release json did not contain tag_name".to_string())?;
    let version = tag.strip_prefix('v').unwrap_or(tag).to_string();
    if !is_newer_version(&version, current_version) {
        return Ok(None);
    }
    let has_asset = value
        .get("assets")
        .and_then(|assets| assets.as_array())
        .is_some_and(|assets| {
            assets.iter().any(|asset| {
                asset
                    .get("name")
                    .and_then(|name| name.as_str())
                    .is_some_and(|name| name == asset_name)
            })
        });
    if !has_asset {
        return Ok(None);
    }
    Ok(Some(UpdateRelease {
        version,
        tag: tag.to_string(),
        asset_name: asset_name.to_string(),
    }))
}

fn is_newer_version(candidate: &str, current: &str) -> bool {
    let candidate_parts = version_parts(candidate);
    let current_parts = version_parts(current);
    for idx in 0..candidate_parts.len().max(current_parts.len()) {
        let candidate = candidate_parts.get(idx).copied().unwrap_or(0);
        let current = current_parts.get(idx).copied().unwrap_or(0);
        if candidate != current {
            return candidate > current;
        }
    }
    false
}

fn version_parts(version: &str) -> Vec<u64> {
    version
        .split(|ch: char| !ch.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse::<u64>().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_numerically() {
        assert!(is_newer_version("0.10.0", "0.9.9"));
        assert!(is_newer_version("1.0.0", "0.99.0"));
        assert!(!is_newer_version("0.1.0", "0.1.0"));
        assert!(!is_newer_version("0.1.0", "0.2.0"));
    }

    #[test]
    fn parses_latest_release_with_matching_asset() {
        let json = r#"{
            "tag_name": "v0.2.0",
            "assets": [
                { "name": "gmux-linux-x86_64.tar.gz" },
                { "name": "SHA256SUMS" }
            ]
        }"#;

        let release = latest_release_from_json(json, "0.1.0", "gmux-linux-x86_64.tar.gz")
            .expect("valid release json")
            .expect("new release");

        assert_eq!(release.version, "0.2.0");
        assert_eq!(release.tag, "v0.2.0");
    }

    #[test]
    fn ignores_release_without_matching_asset() {
        let json = r#"{
            "tag_name": "v0.2.0",
            "assets": [{ "name": "gmux-darwin-aarch64.tar.gz" }]
        }"#;

        let release = latest_release_from_json(json, "0.1.0", "gmux-linux-x86_64.tar.gz")
            .expect("valid release json");

        assert!(release.is_none());
    }
}
