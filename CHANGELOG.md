# Changelog

## Unreleased

### Changed

- Converted this repository into a source-built personal fork.
- Removed public website, hosted installer, package-manager, release workflow, release manifest, preview channel, and in-app update infrastructure.
- Updated documentation and metadata to point at `https://github.com/stanley2058/gmux`.

### Removed

- Removed the self-update and update-channel commands.
- Removed background update checks, release-note announcements, product announcements, and update notifications from the TUI/server runtime.
- Removed remote bootstrap downloads from hosted manifests; remote use now requires an existing compatible remote binary, a same-platform local binary, or `GMUX_REMOTE_BINARY`.
