//! What an MCP tool call becomes for the model: the result text, the error text, the size cap, and the mapping of the
//! rmcp errors. Pure logic: no process, no connection.

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rmcp::model::{CallToolResult, ContentBlock, ResourceContents};
use rmcp::ServiceError;

use super::client::AUTH_MESSAGE;

/// Largest tool result or error text handed to the model, in bytes, the notice included.
pub const RESULT_LIMIT: usize = 32 * 1024;
/// Room kept free for the truncation notice (its text plus a byte count).
const NOTICE_RESERVE: usize = 64;

/// The text of a successful call, or the error text of a call the server reported as failed (`isError`).
///
/// Text blocks, images, audio, resource links and embedded resources are rendered in order, one per line. When the
/// result has no text at all, its structured content is shown as pretty JSON instead.
pub fn map_result(result: &CallToolResult) -> Result<String, String> {
    let has_text = result.content.iter().any(|block| text_of(block).is_some());
    let text = match &result.structured_content {
        Some(structured) if !has_text => serde_json::to_string_pretty(structured).unwrap_or_default(),
        _ => result.content.iter().map(render_block).collect::<Vec<_>>().join("\n"),
    };
    if result.is_error == Some(true) {
        return Err(if text.is_empty() { "tool reported an error".to_string() } else { text });
    }
    Ok(text)
}

/// The model-facing text of a failed call. `timeout` is the per-call limit that was in force.
pub fn service_error_text(error: &ServiceError, server: &str, timeout: Duration) -> String {
    match error {
        ServiceError::McpError(e) => e.message.to_string(),
        ServiceError::Timeout { .. } => format!("timed out after {} s (MCP server '{server}')", timeout.as_secs()),
        ServiceError::Cancelled { .. } => "cancelled".to_string(),
        ServiceError::TransportClosed | ServiceError::TransportSend(_) if error.is_authorization_required() => {
            AUTH_MESSAGE.to_string()
        }
        ServiceError::TransportClosed | ServiceError::TransportSend(_) => {
            format!("MCP server '{server}' connection lost: {error}")
        }
        other => format!("MCP server '{server}': {other}"),
    }
}

/// Whether a call error means the connection to the server is gone, so the runtime must rebuild it.
pub fn is_connection_lost(error: &ServiceError) -> bool {
    matches!(error, ServiceError::TransportClosed | ServiceError::TransportSend(_)) && !error.is_authorization_required()
}

/// Cuts `text` to `RESULT_LIMIT` bytes, on a character boundary, with a notice of the bytes left out.
pub fn cap_result(text: String) -> String {
    if text.len() <= RESULT_LIMIT {
        return text;
    }
    let mut keep = RESULT_LIMIT - NOTICE_RESERVE;
    while !text.is_char_boundary(keep) {
        keep -= 1;
    }
    let notice = format!("\n[truncated {} bytes]", text.len() - keep);
    format!("{}{notice}", &text[..keep])
}

fn text_of(block: &ContentBlock) -> Option<&str> {
    match block {
        ContentBlock::Text(text) => Some(&text.text),
        ContentBlock::Resource(embedded) => match &embedded.resource {
            ResourceContents::TextResourceContents { text, .. } => Some(text),
            _ => None,
        },
        _ => None,
    }
}

fn render_block(block: &ContentBlock) -> String {
    match block {
        ContentBlock::Text(text) => text.text.clone(),
        ContentBlock::Image(image) => {
            format!("[image: {}, {} bytes omitted]", image.mime_type, decoded_len(&image.data))
        }
        ContentBlock::Audio(audio) => {
            format!("[audio: {}, {} bytes omitted]", audio.mime_type, decoded_len(&audio.data))
        }
        ContentBlock::ResourceLink(link) => format!("[resource: {}]", link.uri),
        ContentBlock::Resource(embedded) => match &embedded.resource {
            ResourceContents::TextResourceContents { text, .. } => text.clone(),
            ResourceContents::BlobResourceContents { uri, blob, .. } => {
                format!("[embedded resource: {uri}, {} bytes omitted]", decoded_len(blob))
            }
            _ => "[embedded resource]".to_string(),
        },
        _ => "[unsupported content]".to_string(),
    }
}

/// The size of base64 `data` once decoded. Falls back to the size estimate when the data is not valid base64.
fn decoded_len(data: &str) -> usize {
    STANDARD.decode(data).map_or(data.len() / 4 * 3, |bytes| bytes.len())
}

#[cfg(test)]
mod tests {
    use rmcp::model::{AudioContent, EmbeddedResource, ImageContent, Resource, TextContent};
    use serde_json::json;

    use super::*;

    fn result(content: Vec<ContentBlock>) -> CallToolResult {
        CallToolResult::success(content)
    }

    fn text(s: &str) -> ContentBlock {
        ContentBlock::Text(TextContent::new(s))
    }

    #[test]
    fn text_blocks_are_joined_with_newlines_in_order() {
        let r = result(vec![text("first"), text("second")]);
        assert_eq!(map_result(&r), Ok("first\nsecond".to_string()));
    }

    #[test]
    fn an_empty_result_is_an_empty_string() {
        assert_eq!(map_result(&result(vec![])), Ok(String::new()));
    }

    #[test]
    fn images_and_audio_are_named_with_their_decoded_size() {
        // "AAAA" decodes to 3 zero bytes.
        let r = result(vec![
            ContentBlock::Image(ImageContent::new("AAAA", "image/png")),
            ContentBlock::Audio(AudioContent::new("AAAAAAAA", "audio/wav")),
        ]);
        assert_eq!(map_result(&r), Ok("[image: image/png, 3 bytes omitted]\n[audio: audio/wav, 6 bytes omitted]".into()));
    }

    #[test]
    fn a_resource_link_is_named_by_its_uri() {
        let link = Resource::new("file:///a.txt", "a");
        assert_eq!(map_result(&result(vec![ContentBlock::ResourceLink(link)])), Ok("[resource: file:///a.txt]".into()));
    }

    #[test]
    fn an_embedded_text_resource_shows_its_text_and_a_blob_only_its_size() {
        let text_res = EmbeddedResource::new(ResourceContents::TextResourceContents {
            uri: "file:///x".into(),
            mime_type: None,
            text: "inside".into(),
            meta: None,
        });
        let blob_res = EmbeddedResource::new(ResourceContents::BlobResourceContents {
            uri: "file:///y".into(),
            mime_type: None,
            blob: "AAAA".into(),
            meta: None,
        });
        let r = result(vec![ContentBlock::Resource(text_res), ContentBlock::Resource(blob_res)]);
        assert_eq!(map_result(&r), Ok("inside\n[embedded resource: file:///y, 3 bytes omitted]".into()));
    }

    #[test]
    fn structured_content_is_shown_only_when_there_is_no_text() {
        let mut no_text = result(vec![ContentBlock::Image(ImageContent::new("AAAA", "image/png"))]);
        no_text.structured_content = Some(json!({"b": 2, "a": 1}));
        let shown = map_result(&no_text).unwrap();
        assert!(shown.contains("\"a\": 1") && shown.contains('\n'), "{shown}");

        let mut with_text = result(vec![text("plain")]);
        with_text.structured_content = Some(json!({"a": 1}));
        assert_eq!(map_result(&with_text), Ok("plain".to_string()));
    }

    #[test]
    fn an_error_result_is_the_error_text_or_a_default() {
        let failed = CallToolResult::error(vec![text("boom")]);
        assert_eq!(map_result(&failed), Err("boom".to_string()));
        let silent = CallToolResult::error(vec![]);
        assert_eq!(map_result(&silent), Err("tool reported an error".to_string()));
    }

    #[test]
    fn a_result_within_the_limit_is_not_cut() {
        let text = "a".repeat(RESULT_LIMIT);
        assert_eq!(cap_result(text.clone()), text);
    }

    #[test]
    fn an_over_limit_result_is_cut_with_a_notice_and_stays_within_the_limit() {
        let text = "b".repeat(100 * 1024);
        let capped = cap_result(text);
        assert!(capped.len() <= RESULT_LIMIT, "{}", capped.len());
        assert!(capped.ends_with(" bytes]"), "{}", &capped[capped.len() - 40..]);
        assert!(capped.contains("[truncated "), "{}", &capped[capped.len() - 40..]);
    }

    #[test]
    fn the_cut_lands_on_a_character_boundary() {
        // Multi-byte characters straddling the limit: must not panic and must stay valid UTF-8.
        let text = "é".repeat(RESULT_LIMIT);
        let capped = cap_result(text);
        assert!(capped.len() <= RESULT_LIMIT);
        assert!(capped.is_char_boundary(capped.find("\n[truncated").unwrap()));
    }

    #[test]
    fn service_errors_map_to_clear_text() {
        let timeout = Duration::from_secs(7);
        assert_eq!(
            service_error_text(&ServiceError::Timeout { timeout }, "fetch", timeout),
            "timed out after 7 s (MCP server 'fetch')"
        );
        assert_eq!(service_error_text(&ServiceError::Cancelled { reason: None }, "fetch", timeout), "cancelled");
        assert!(service_error_text(&ServiceError::TransportClosed, "fetch", timeout).contains("connection lost"));
        assert!(is_connection_lost(&ServiceError::TransportClosed));
        assert!(!is_connection_lost(&ServiceError::Cancelled { reason: None }));
    }

    #[test]
    fn a_json_rpc_error_shows_the_server_message() {
        let error = ServiceError::McpError(rmcp::ErrorData::invalid_params("bad arguments", None));
        assert_eq!(service_error_text(&error, "s", Duration::from_secs(1)), "bad arguments");
        assert!(!is_connection_lost(&error));
    }
}
