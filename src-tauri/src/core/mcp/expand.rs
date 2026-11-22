//! `${NAME}` and `${NAME:-default}` in the configured values of an MCP server, and the resolution of a server's transport
//! into the values the process is started or connected with.
//!
//! Errors name the variable, never a value and never the text around the reference (it may hold a secret).

use std::collections::BTreeMap;
use std::fmt;

use super::config::{McpTransport, valid_env_name};
use super::secrets::SecretValues;

/// Replaces every `${NAME}` and `${NAME:-default}` in `input`. `lookup` gives a variable's value, `None` when unset.
/// `${NAME:-default}` uses `default` when the variable is unset or empty; `default` ends at the first `}` and is not
/// expanded itself. `${NAME}` with no value is an error. A `$` not followed by `{` stays as it is.
pub fn expand(input: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Result<String, String> {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(pos) = rest.find('$') {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 1..];
        let Some(body) = after.strip_prefix('{') else {
            out.push('$');
            rest = after;
            continue;
        };
        let Some(close) = body.find('}') else {
            return Err("unterminated ${ reference: add the closing }".into());
        };
        let inner = &body[..close];
        rest = &body[close + 1..];
        let (name, default) = match inner.split_once(":-") {
            Some((name, default)) => (name, Some(default)),
            None => (inner, None),
        };
        if !valid_env_name(name) {
            return Err("invalid variable reference: names are letters, digits and _, not starting with a digit".into());
        }
        let value = match (lookup(name).filter(|v| !v.is_empty() || default.is_none()), default) {
            (Some(value), _) => value,
            (None, Some(default)) => default.to_string(),
            (None, None) => return Err(format!("variable {name} is not set")),
        };
        out.push_str(&value);
    }
    out.push_str(rest);
    Ok(out)
}

/// The transport of one server with every reference expanded and every secret value filled in. Holds secret values,
/// so `Debug` shows names only.
#[derive(Clone, PartialEq, Eq)]
pub enum ResolvedTransport {
    Stdio {
        command: String,
        args: Vec<String>,
        cwd: Option<String>,
        /// Variable name to value, for the names in the config.
        env: BTreeMap<String, String>,
    },
    Http {
        url: String,
        /// Header name to value.
        headers: Vec<(String, String)>,
    },
}

impl fmt::Debug for ResolvedTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolvedTransport::Stdio { command, args, cwd, env } => f
                .debug_struct("Stdio")
                .field("command", command)
                .field("args", args)
                .field("cwd", cwd)
                .field("env", &env.keys().map(|k| (k.as_str(), "<redacted>")).collect::<BTreeMap<_, _>>())
                .finish(),
            ResolvedTransport::Http { url, headers } => f
                .debug_struct("Http")
                .field("url", url)
                .field("headers", &headers.iter().map(|(k, _)| (k.as_str(), "<redacted>")).collect::<Vec<_>>())
                .finish(),
        }
    }
}

/// Resolves a server's transport. A `${NAME}` is looked up in the server's own values first (env, then headers),
/// then in the process environment (`process`). Every config env name needs a value from the server's own env or
/// from the process environment. Every config header needs a stored value.
pub fn resolve_transport(
    transport: &McpTransport,
    values: &SecretValues,
    process: &dyn Fn(&str) -> Option<String>,
) -> Result<ResolvedTransport, String> {
    let lookup = |name: &str| {
        values.env_values.get(name).or_else(|| values.header_values.get(name)).cloned().or_else(|| process(name))
    };
    match transport {
        McpTransport::Stdio { command, args, cwd, env } => {
            let command = expand(command, &lookup).map_err(|e| format!("command: {e}"))?;
            let args = args
                .iter()
                .enumerate()
                .map(|(index, arg)| expand(arg, &lookup).map_err(|e| format!("args[{index}]: {e}")))
                .collect::<Result<Vec<_>, _>>()?;
            let cwd = cwd.as_deref().map(|cwd| expand(cwd, &lookup).map_err(|e| format!("cwd: {e}"))).transpose()?;
            let mut resolved = BTreeMap::new();
            for name in env {
                let raw = lookup(name).ok_or_else(|| {
                    format!("env variable {name} has no value: store it for this server or set it in the environment")
                })?;
                let value = expand(&raw, &lookup).map_err(|e| format!("env variable {name}: {e}"))?;
                resolved.insert(name.clone(), value);
            }
            Ok(ResolvedTransport::Stdio { command, args, cwd, env: resolved })
        }
        McpTransport::Http { url, headers } => {
            let url = expand(url, &lookup).map_err(|e| format!("url: {e}"))?;
            let mut resolved = Vec::new();
            for name in headers {
                let raw = values
                    .header_values
                    .get(name)
                    .ok_or_else(|| format!("header {name} has no stored value"))?;
                let value = expand(raw, &lookup).map_err(|e| format!("header {name}: {e}"))?;
                resolved.push((name.clone(), value));
            }
            Ok(ResolvedTransport::Http { url, headers: resolved })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let owned: BTreeMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |name: &str| owned.get(name).cloned()
    }

    fn none(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn plain_text_is_unchanged() {
        assert_eq!(expand("hello world", &none).unwrap(), "hello world");
        assert_eq!(expand("", &none).unwrap(), "");
    }

    #[test]
    fn a_set_variable_is_replaced() {
        let lookup = map(&[("HOME_DIR", "/srv/data"), ("EMPTY", "")]);
        assert_eq!(expand("${HOME_DIR}/x", &lookup).unwrap(), "/srv/data/x");
        assert_eq!(expand("a${HOME_DIR}b${HOME_DIR}c", &lookup).unwrap(), "a/srv/datab/srv/datac");
        // A set but empty variable with no default is an empty string.
        assert_eq!(expand("[${EMPTY}]", &lookup).unwrap(), "[]");
    }

    #[test]
    fn an_unset_variable_without_default_is_an_error_that_names_it() {
        let err = expand("token=${API_KEY_NAME}", &none).unwrap_err();
        assert_eq!(err, "variable API_KEY_NAME is not set");
    }

    #[test]
    fn a_default_is_used_when_unset_or_empty() {
        let lookup = map(&[("SET", "real"), ("EMPTY", "")]);
        assert_eq!(expand("${MISSING:-fallback}", &lookup).unwrap(), "fallback");
        assert_eq!(expand("${EMPTY:-fallback}", &lookup).unwrap(), "fallback");
        assert_eq!(expand("${SET:-fallback}", &lookup).unwrap(), "real");
        assert_eq!(expand("${MISSING:-}", &none).unwrap(), "");
    }

    #[test]
    fn a_default_is_not_expanded_and_ends_at_the_first_brace() {
        let lookup = map(&[("INNER", "inner")]);
        // The default is "${INNER" (ends at the first '}'), copied as is; the last '}' is literal text.
        assert_eq!(expand("${MISSING:-${INNER}}", &lookup).unwrap(), "${INNER}");
    }

    #[test]
    fn a_dollar_not_followed_by_brace_stays_literal() {
        assert_eq!(expand("price $5 and $ alone", &none).unwrap(), "price $5 and $ alone");
        assert_eq!(expand("end with $", &none).unwrap(), "end with $");
        assert_eq!(expand("$$", &none).unwrap(), "$$");
        assert_eq!(expand("$NAME without braces", &map(&[("NAME", "x")])).unwrap(), "$NAME without braces");
    }

    #[test]
    fn an_unterminated_reference_is_an_error() {
        assert!(expand("${OPEN", &none).unwrap_err().contains("unterminated"));
        assert!(expand("${OPEN:-x", &none).unwrap_err().contains("unterminated"));
        assert!(expand("${A} then ${B", &map(&[("A", "1")])).unwrap_err().contains("unterminated"));
    }

    #[test]
    fn an_invalid_name_is_an_error_that_does_not_echo_the_text() {
        for bad in ["${}", "${1ABC}", "${A-B}", "${has space}", "${:-x}"] {
            let err = expand(bad, &none).unwrap_err();
            assert!(err.contains("invalid variable reference"), "{bad}: {err}");
            assert!(!err.contains("has space") && !err.contains("A-B"), "{err}");
        }
    }

    #[test]
    fn names_follow_the_identifier_rule() {
        let lookup = map(&[("_under", "u"), ("a1B2", "v")]);
        assert_eq!(expand("${_under}${a1B2}", &lookup).unwrap(), "uv");
    }

    fn server_values() -> SecretValues {
        SecretValues {
            env_values: BTreeMap::from([("API_TOKEN".to_string(), "tok-from-store".to_string())]),
            header_values: BTreeMap::from([("Authorization".to_string(), "Bearer ${API_TOKEN}".to_string())]),
        }
    }

    #[test]
    fn stdio_fields_are_expanded_and_env_values_come_from_the_store_first() {
        let transport = McpTransport::Stdio {
            command: "${TOOL_BIN:-uvx}".into(),
            args: vec!["--dir".into(), "${WORK_DIR}".into()],
            cwd: Some("${WORK_DIR}/sub".into()),
            env: vec!["API_TOKEN".into(), "HOME_HINT".into()],
        };
        let process = map(&[("WORK_DIR", "/work"), ("HOME_HINT", "from-process")]);
        let resolved = resolve_transport(&transport, &server_values(), &process).unwrap();
        assert_eq!(
            resolved,
            ResolvedTransport::Stdio {
                command: "uvx".into(),
                args: vec!["--dir".into(), "/work".into()],
                cwd: Some("/work/sub".into()),
                env: BTreeMap::from([
                    ("API_TOKEN".to_string(), "tok-from-store".to_string()),
                    ("HOME_HINT".to_string(), "from-process".to_string()),
                ]),
            }
        );
    }

    #[test]
    fn http_headers_are_read_from_the_store_and_expanded() {
        let transport = McpTransport::Http { url: "https://${HOST}/mcp".into(), headers: vec!["Authorization".into()] };
        let process = map(&[("HOST", "api.example.test")]);
        let resolved = resolve_transport(&transport, &server_values(), &process).unwrap();
        assert_eq!(
            resolved,
            ResolvedTransport::Http {
                url: "https://api.example.test/mcp".into(),
                headers: vec![("Authorization".into(), "Bearer tok-from-store".into())],
            }
        );
    }

    #[test]
    fn a_missing_env_value_or_header_value_is_an_error_naming_it() {
        let stdio = McpTransport::Stdio { command: "x".into(), args: vec![], cwd: None, env: vec!["NOPE".into()] };
        let err = resolve_transport(&stdio, &SecretValues::default(), &none).unwrap_err();
        assert!(err.contains("env variable NOPE has no value"), "{err}");

        let http = McpTransport::Http { url: "https://x.example".into(), headers: vec!["X-Key".into()] };
        let err = resolve_transport(&http, &SecretValues::default(), &none).unwrap_err();
        assert_eq!(err, "header X-Key has no stored value");
    }

    #[test]
    fn a_bad_reference_in_an_argument_names_the_argument() {
        let transport = McpTransport::Stdio {
            command: "x".into(),
            args: vec!["ok".into(), "${MISSING_THING}".into()],
            cwd: None,
            env: vec![],
        };
        let err = resolve_transport(&transport, &SecretValues::default(), &none).unwrap_err();
        assert_eq!(err, "args[1]: variable MISSING_THING is not set");
    }

    #[test]
    fn debug_of_a_resolved_transport_hides_values() {
        let transport = McpTransport::Http { url: "https://x.example".into(), headers: vec!["Authorization".into()] };
        let resolved = resolve_transport(&transport, &server_values(), &none).unwrap();
        let debug = format!("{resolved:?}");
        assert!(debug.contains("Authorization"), "{debug}");
        assert!(!debug.contains("tok-from-store"), "{debug}");
    }
}
