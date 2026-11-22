//! The program to start for a stdio server. On Windows, the Node tools are batch files (`npx.cmd`), which `CreateProcess`
//! does not find from a bare `npx`. Everything else is started as written.

/// The Node launchers that are `.cmd` files on Windows.
const WINDOWS_CMD_LAUNCHERS: &[&str] = &["npx", "npm", "pnpm", "yarn"];

/// The command to start for a configured `command`. On Windows only, a bare `npx`, `npm`, `pnpm` or `yarn` (any case,
/// no path, no extension) gets `.cmd` added. Anything else, including an explicit `npx.cmd` or `cmd`, is returned as
/// it is.
pub fn resolve_launch(command: &str, windows: bool) -> String {
    let bare = !command.contains(['/', '\\', '.']);
    let is_launcher = WINDOWS_CMD_LAUNCHERS.iter().any(|name| command.eq_ignore_ascii_case(name));
    if windows && bare && is_launcher { format!("{command}.cmd") } else { command.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_node_launchers_get_cmd_on_windows() {
        assert_eq!(resolve_launch("npx", true), "npx.cmd");
        assert_eq!(resolve_launch("npm", true), "npm.cmd");
        assert_eq!(resolve_launch("pnpm", true), "pnpm.cmd");
        assert_eq!(resolve_launch("yarn", true), "yarn.cmd");
        assert_eq!(resolve_launch("NPX", true), "NPX.cmd");
    }

    #[test]
    fn nothing_changes_off_windows() {
        for name in ["npx", "npm", "pnpm", "yarn"] {
            assert_eq!(resolve_launch(name, false), name);
        }
    }

    #[test]
    fn explicit_extensions_paths_and_other_commands_are_unchanged_on_windows() {
        for command in [
            "npx.cmd",
            "npx.exe",
            "cmd",
            "uvx",
            "node",
            r"C:\Program Files\nodejs\npx",
            "C:/nodejs/npx",
            r".\npx",
            "./npx",
            "npx-helper",
            "my npx",
            "",
        ] {
            assert_eq!(resolve_launch(command, true), command, "{command:?}");
        }
    }
}
