//! Auto-start-at-login integration via the `auto-launch` crate.
//!
//! Registers the current executable to launch at login with a `--minimized`
//! flag so only the tray icon appears (see `main.rs` for the flag handling).
//! Best-effort: registry/LaunchAgent/XDG autostart behavior cannot be fully
//! verified in this Linux devcontainer sandbox (AGENTS.md reserves that kind
//! of manual verification for the user).

use auto_launch::AutoLaunchBuilder;

const APP_NAME: &str = "vrc-immich-syncer";

/// Enables or disables auto-start at login for the current executable,
/// launching with `--minimized` when enabled.
pub fn set_auto_start(enabled: bool) -> anyhow::Result<()> {
    let exe_path = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("failed to determine current executable path: {e}"))?;
    let exe_path_str = exe_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("executable path is not valid UTF-8: {exe_path:?}"))?;

    let auto_launch = AutoLaunchBuilder::new()
        .set_app_name(APP_NAME)
        .set_app_path(exe_path_str)
        .set_args(&["--minimized"])
        .build()
        .map_err(|e| anyhow::anyhow!("failed to configure auto-launch: {e}"))?;

    if enabled {
        auto_launch
            .enable()
            .map_err(|e| anyhow::anyhow!("failed to enable auto-start: {e}"))?;
    } else {
        // `disable` errors if the entry doesn't exist on some platforms;
        // treat "already disabled" as success by checking first.
        let is_enabled = auto_launch.is_enabled().unwrap_or(false);
        if is_enabled {
            auto_launch
                .disable()
                .map_err(|e| anyhow::anyhow!("failed to disable auto-start: {e}"))?;
        }
    }

    Ok(())
}

/// Best-effort query of the current auto-start registration state. Returns
/// `Ok(false)` (rather than erroring) if the platform backend can't
/// determine the state, since this is only used for UI display.
pub fn is_auto_start_enabled() -> anyhow::Result<bool> {
    let exe_path = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("failed to determine current executable path: {e}"))?;
    let exe_path_str = exe_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("executable path is not valid UTF-8: {exe_path:?}"))?;

    let auto_launch = AutoLaunchBuilder::new()
        .set_app_name(APP_NAME)
        .set_app_path(exe_path_str)
        .set_args(&["--minimized"])
        .build()
        .map_err(|e| anyhow::anyhow!("failed to configure auto-launch: {e}"))?;

    Ok(auto_launch.is_enabled().unwrap_or(false))
}
