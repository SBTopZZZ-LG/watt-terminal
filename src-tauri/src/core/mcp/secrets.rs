//! The secret values of MCP servers: env variable values and header values, kept out of settings.json.
//!
//! They live in the same `Secrets` map as the provider keys, under `mcp.<server>.env.<NAME>` and
//! `mcp.<server>.header.<Name>`. The store already splits a large blob across Keychain items (see `secrets.rs`), so no
//! extra keyring entries are needed. A server name holds no '.', so one server's prefix never matches another's.

use std::collections::BTreeMap;
use std::fmt;

use crate::core::settings::secrets::{SecretError, Secrets};

const PREFIX: &str = "mcp.";
const ENV_PART: &str = "env.";
const HEADER_PART: &str = "header.";

/// The values of one server, env and headers kept apart. Keys are the variable or header names as configured.
/// `Debug` shows the names only.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SecretValues {
    pub env_values: BTreeMap<String, String>,
    pub header_values: BTreeMap<String, String>,
}

impl fmt::Debug for SecretValues {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretValues")
            .field("env_values", &redacted_names(&self.env_values))
            .field("header_values", &redacted_names(&self.header_values))
            .finish()
    }
}

/// The names of `values` with a placeholder for each value.
fn redacted_names(values: &BTreeMap<String, String>) -> BTreeMap<&str, &str> {
    values.keys().map(|name| (name.as_str(), "<redacted>")).collect()
}

/// MCP secrets, read and written through the app's `Secrets`.
pub struct McpSecrets<'a> {
    secrets: &'a Secrets,
}

impl<'a> McpSecrets<'a> {
    pub fn new(secrets: &'a Secrets) -> Self {
        Self { secrets }
    }

    /// The stored values of `server`. Empty when none are stored.
    pub fn get(&self, server: &str) -> Result<SecretValues, SecretError> {
        let map = self.secrets.snapshot()?;
        let prefix = server_prefix(server);
        let mut values = SecretValues::default();
        for (key, value) in &map {
            let Some(rest) = key.strip_prefix(&prefix) else {
                continue;
            };
            if let Some(name) = rest.strip_prefix(ENV_PART) {
                values.env_values.insert(name.to_string(), value.clone());
            } else if let Some(name) = rest.strip_prefix(HEADER_PART) {
                values.header_values.insert(name.to_string(), value.clone());
            }
        }
        Ok(values)
    }

    /// Replaces every stored value of `server` with `values`, in one save. Empty values are not stored.
    pub fn set(&self, server: &str, values: SecretValues) -> Result<(), SecretError> {
        let prefix = server_prefix(server);
        self.secrets.edit(|map| {
            map.retain(|key, _| !key.starts_with(&prefix));
            for (name, value) in values.env_values.iter().filter(|(_, v)| !v.is_empty()) {
                map.insert(format!("{prefix}{ENV_PART}{name}"), value.clone());
            }
            for (name, value) in values.header_values.iter().filter(|(_, v)| !v.is_empty()) {
                map.insert(format!("{prefix}{HEADER_PART}{name}"), value.clone());
            }
        })
    }

    /// Removes every stored value of `server`.
    pub fn delete(&self, server: &str) -> Result<(), SecretError> {
        self.secrets.remove_prefix(&server_prefix(server))
    }

    /// Every non-empty MCP secret value, of every server, for the Redactor. Returned as an error, not an empty list,
    /// when the store cannot be read, so a caller never masks less than it should without knowing.
    pub fn all_values(&self) -> Result<Vec<String>, SecretError> {
        let map = self.secrets.snapshot()?;
        Ok(map
            .iter()
            .filter(|(key, value)| key.starts_with(PREFIX) && !value.is_empty())
            .map(|(_, value)| value.clone())
            .collect())
    }
}

fn server_prefix(server: &str) -> String {
    format!("{PREFIX}{server}.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::settings::secrets::MemoryStore;

    fn values(env: &[(&str, &str)], headers: &[(&str, &str)]) -> SecretValues {
        SecretValues {
            env_values: env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            header_values: headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        }
    }

    fn fake() -> (Secrets, MemoryStore) {
        let store = MemoryStore::default();
        (Secrets::new(Box::new(store.clone())), store)
    }

    #[test]
    fn set_then_get_keeps_env_and_headers_apart() {
        let (secrets, _) = fake();
        let mcp = McpSecrets::new(&secrets);
        let input = values(&[("FETCH_TOKEN", "env-value-1")], &[("Authorization", "Bearer hdr-value-2")]);
        mcp.set("you-com", input.clone()).unwrap();
        assert_eq!(mcp.get("you-com").unwrap(), input);
        assert_eq!(mcp.get("other").unwrap(), SecretValues::default());
    }

    #[test]
    fn values_are_stored_under_the_server_keys_in_the_shared_map() {
        let (secrets, store) = fake();
        McpSecrets::new(&secrets)
            .set("ctx", values(&[("K_ENV", "e1")], &[("X.Custom", "h1")]))
            .unwrap();
        let stored = store.stored().unwrap();
        assert!(stored.contains(r#""mcp.ctx.env.K_ENV":"e1""#), "{stored}");
        assert!(stored.contains(r#""mcp.ctx.header.X.Custom":"h1""#), "{stored}");
    }

    #[test]
    fn servers_do_not_see_each_others_values() {
        let (secrets, _) = fake();
        let mcp = McpSecrets::new(&secrets);
        mcp.set("a", values(&[("K", "from-a")], &[])).unwrap();
        mcp.set("a-b", values(&[("K", "from-a-b")], &[])).unwrap();
        assert_eq!(mcp.get("a").unwrap().env_values.get("K").map(String::as_str), Some("from-a"));
        assert_eq!(mcp.get("a-b").unwrap().env_values.get("K").map(String::as_str), Some("from-a-b"));
        mcp.delete("a").unwrap();
        assert_eq!(mcp.get("a-b").unwrap().env_values.get("K").map(String::as_str), Some("from-a-b"));
    }

    #[test]
    fn set_replaces_the_old_values_of_the_server() {
        let (secrets, _) = fake();
        let mcp = McpSecrets::new(&secrets);
        mcp.set("fetch", values(&[("OLD", "old-value")], &[("X-Old", "old-h")])).unwrap();
        mcp.set("fetch", values(&[("NEW", "new-value")], &[])).unwrap();
        assert_eq!(mcp.get("fetch").unwrap(), values(&[("NEW", "new-value")], &[]));
    }

    #[test]
    fn empty_values_are_not_stored() {
        let (secrets, _) = fake();
        let mcp = McpSecrets::new(&secrets);
        mcp.set("fetch", values(&[("EMPTY", "")], &[("X-Empty", "")])).unwrap();
        assert_eq!(mcp.get("fetch").unwrap(), SecretValues::default());
        assert!(mcp.all_values().unwrap().is_empty());
    }

    #[test]
    fn delete_removes_only_that_server() {
        let (secrets, _) = fake();
        let mcp = McpSecrets::new(&secrets);
        mcp.set("fetch", values(&[("A", "1")], &[])).unwrap();
        mcp.set("ctx", values(&[("B", "2")], &[])).unwrap();
        mcp.delete("fetch").unwrap();
        assert_eq!(mcp.get("fetch").unwrap(), SecretValues::default());
        assert_eq!(mcp.get("ctx").unwrap(), values(&[("B", "2")], &[]));
        mcp.delete("never-stored").unwrap();
    }

    #[test]
    fn provider_secrets_are_not_mcp_values() {
        let (secrets, _) = fake();
        secrets.set("provider.example.api_key", "provider-key-value").unwrap();
        let mcp = McpSecrets::new(&secrets);
        assert!(mcp.all_values().unwrap().is_empty());
        mcp.delete("example").unwrap();
        assert!(secrets.has("provider.example.api_key").unwrap(), "delete must not touch other secrets");
    }

    #[test]
    fn all_values_lists_every_non_empty_mcp_value() {
        let (secrets, _) = fake();
        let mcp = McpSecrets::new(&secrets);
        mcp.set("fetch", values(&[("A", "env-secret-a")], &[("Authorization", "Bearer hdr-secret-b")])).unwrap();
        mcp.set("ctx", values(&[], &[("CONTEXT7_API_KEY", "ctx7-secret-c")])).unwrap();
        let mut all = mcp.all_values().unwrap();
        all.sort();
        assert_eq!(all, vec!["Bearer hdr-secret-b", "ctx7-secret-c", "env-secret-a"]);
    }

    #[test]
    fn all_values_reports_a_failed_read_instead_of_an_empty_list() {
        let (secrets, store) = fake();
        store.set_fail_loads(true);
        assert!(McpSecrets::new(&secrets).all_values().is_err());
    }

    #[test]
    fn a_failed_save_is_an_error_and_keeps_the_old_values() {
        let (secrets, store) = fake();
        let mcp = McpSecrets::new(&secrets);
        mcp.set("fetch", values(&[("A", "old-value")], &[])).unwrap();
        store.set_fail_saves(true);
        assert!(mcp.set("fetch", values(&[("A", "new-value")], &[])).is_err());
        assert_eq!(mcp.get("fetch").unwrap(), values(&[("A", "old-value")], &[]));
    }

    #[test]
    fn debug_shows_names_but_no_values() {
        let input = values(&[("TOKEN", "very-secret-env")], &[("Authorization", "Bearer very-secret-hdr")]);
        let debug = format!("{input:?}");
        assert!(debug.contains("TOKEN") && debug.contains("Authorization"), "{debug}");
        assert!(!debug.contains("very-secret"), "{debug}");
    }
}
