//! The approval of one MCP tool, from the server's enabled switch, the server level and the tool's override. The global
//! mode (approve-all, bypass) is not decided here: the gate applies it on top of this result.

use crate::core::external_tools::ExternalApproval;

use super::config::McpLevel;

/// A disabled server offers nothing. Otherwise the tool's own override wins over the server level.
pub fn resolve(server_enabled: bool, server: McpLevel, tool_override: Option<McpLevel>) -> ExternalApproval {
    if !server_enabled {
        return ExternalApproval::Never;
    }
    ExternalApproval::from(tool_override.unwrap_or(server))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEVELS: [McpLevel; 3] = [McpLevel::Ask, McpLevel::Auto, McpLevel::Never];

    /// Every combination of the server level and the tool override, for an enabled server.
    #[test]
    fn enabled_server_with_each_server_level_and_each_override() {
        for server in LEVELS {
            // No override: the server level.
            assert_eq!(resolve(true, server, None), ExternalApproval::from(server), "server {server:?}, inherit");
            // Each override beats the server level.
            for tool in LEVELS {
                assert_eq!(
                    resolve(true, server, Some(tool)),
                    ExternalApproval::from(tool),
                    "server {server:?}, tool {tool:?}"
                );
            }
        }
    }

    /// The 3 x 4 table written out, so a change to either side shows up here.
    #[test]
    fn the_full_table_for_an_enabled_server() {
        use ExternalApproval::{Ask, Auto, Never};
        let cases = [
            (McpLevel::Ask, None, Ask),
            (McpLevel::Ask, Some(McpLevel::Ask), Ask),
            (McpLevel::Ask, Some(McpLevel::Auto), Auto),
            (McpLevel::Ask, Some(McpLevel::Never), Never),
            (McpLevel::Auto, None, Auto),
            (McpLevel::Auto, Some(McpLevel::Ask), Ask),
            (McpLevel::Auto, Some(McpLevel::Auto), Auto),
            (McpLevel::Auto, Some(McpLevel::Never), Never),
            (McpLevel::Never, None, Never),
            (McpLevel::Never, Some(McpLevel::Ask), Ask),
            (McpLevel::Never, Some(McpLevel::Auto), Auto),
            (McpLevel::Never, Some(McpLevel::Never), Never),
        ];
        assert_eq!(cases.len(), 12);
        for (server, tool, expected) in cases {
            assert_eq!(resolve(true, server, tool), expected, "server {server:?}, tool {tool:?}");
        }
    }

    #[test]
    fn a_disabled_server_is_never_whatever_the_levels_say() {
        for server in LEVELS {
            assert_eq!(resolve(false, server, None), ExternalApproval::Never);
            for tool in LEVELS {
                assert_eq!(resolve(false, server, Some(tool)), ExternalApproval::Never, "server {server:?}, tool {tool:?}");
            }
        }
    }
}
