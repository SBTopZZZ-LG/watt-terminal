//! The context budget, slice 1: what a model request costs in tokens, by category, and how that estimate is calibrated
//! against the provider's own count. Nothing here changes a request. The breakdown is made from the wire messages as
//! they are sent, so it counts what the model reads, and the usage is what the context meter shows. The category ids
//! are strings, so a new category needs no change to the frontend's type.

use serde::Serialize;
use serde_json::Value;

/// The category ids. `OTHER` also takes anything the classifier does not recognise.
pub const SYSTEM: &str = "system";
pub const TOOLS: &str = "tools";
pub const USER: &str = "user";
pub const ATTACHMENTS: &str = "attachments";
pub const ASSISTANT: &str = "assistant";
pub const TOOL_OUTPUT: &str = "tool_output";
pub const OTHER: &str = "other";

/// The categories in the order they are reported. Each one is always present, even at zero.
const ORDER: [&str; 7] = [SYSTEM, TOOLS, USER, ATTACHMENTS, ASSISTANT, TOOL_OUTPUT, OTHER];

/// The flat estimate for one image part. The base64 data URL is not counted: its length says nothing about the tokens.
pub const IMAGE_TOKENS: u32 = 1000;
/// The fixed cost of each message (its role and framing), counted in the message's own category.
const MESSAGE_OVERHEAD: u32 = 4;
/// The output reserve when the settings give no `max_tokens`.
pub const DEFAULT_OUTPUT_RESERVE: u32 = 4096;
/// The calibration ratio stays in this range, so one odd reply cannot run the estimate away.
const MIN_RATIO: f64 = 0.5;
const MAX_RATIO: f64 = 2.0;
/// The notes that stand where an image is not sent (see core/chat/attachments::wire_content).
const OMITTED_NOTE_END: &str = " omitted from context]";
const UNAVAILABLE_NOTE_END: &str = " is no longer available]";
const NOT_SENT_NOTE: &str = " not sent: the current model does not accept images]";
/// Reasoning fields on an assistant message. They are normally stripped before a request (see core/chat/history).
const REASONING_KEYS: [&str; 3] = ["reasoning_content", "reasoning", "reasoning_details"];

/// The estimated tokens of a text of `chars` characters: a quarter of a token each, rounded up.
pub fn estimate_tokens(chars: usize) -> u32 {
    u32::try_from(chars.div_ceil(4)).unwrap_or(u32::MAX)
}

/// The raw estimate of one request, by category, in `ORDER`. Nothing here is calibrated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Breakdown {
    pub categories: Vec<(String, u32)>,
}

impl Breakdown {
    /// The raw estimate of the whole request: the sum over the categories.
    pub fn total(&self) -> u32 {
        self.categories.iter().fold(0u32, |sum, (_, tokens)| sum.saturating_add(*tokens))
    }
}

/// Characters and fixed token costs per category, in `ORDER`, while one request is classified.
#[derive(Default)]
struct Tally {
    chars: [usize; 7],
    fixed: [u32; 7],
}

impl Tally {
    fn chars(&mut self, id: &str, count: usize) {
        self.chars[slot(id)] += count;
    }

    fn fixed(&mut self, id: &str, tokens: u32) {
        let slot = slot(id);
        self.fixed[slot] = self.fixed[slot].saturating_add(tokens);
    }

    fn finish(self) -> Breakdown {
        let categories = ORDER
            .iter()
            .enumerate()
            .map(|(i, id)| (id.to_string(), estimate_tokens(self.chars[i]).saturating_add(self.fixed[i])))
            .collect();
        Breakdown { categories }
    }
}

/// The index of a category in `ORDER`. An id that is not listed counts as `OTHER`.
fn slot(id: &str) -> usize {
    ORDER.iter().position(|known| *known == id).unwrap_or(ORDER.len() - 1)
}

/// The estimated breakdown of one request: `system` is the system text it was built from, `tools` are the tool
/// definitions sent with it, and `messages` are the wire messages (as `History::messages` returns them). A system
/// message in `messages` is not counted again: its text is the `system` argument.
pub fn breakdown(system: &str, tools: &[Value], messages: &[Value]) -> Breakdown {
    let mut tally = Tally::default();
    tally.chars(SYSTEM, system.chars().count());
    if !tools.is_empty() {
        tally.chars(TOOLS, json_chars(&tools));
    }
    for message in messages {
        classify_message(&mut tally, message);
    }
    tally.finish()
}

/// The characters of a JSON value as it is serialised. A value that cannot be serialised counts as nothing.
fn json_chars<T: Serialize + ?Sized>(value: &T) -> usize {
    serde_json::to_string(value).map_or(0, |text| text.chars().count())
}

fn classify_message(tally: &mut Tally, message: &Value) {
    match message["role"].as_str() {
        // The system message is the `system` argument of `breakdown`, already counted.
        Some("system") => {}
        Some("user") => {
            tally.fixed(USER, MESSAGE_OVERHEAD);
            classify_user_content(tally, &message["content"]);
        }
        Some("assistant") => {
            tally.fixed(ASSISTANT, MESSAGE_OVERHEAD);
            classify_assistant(tally, message);
        }
        Some("tool") => {
            tally.fixed(TOOL_OUTPUT, MESSAGE_OVERHEAD);
            match &message["content"] {
                Value::String(text) => tally.chars(TOOL_OUTPUT, text.chars().count()),
                Value::Null => {}
                other => tally.chars(OTHER, json_chars(other)),
            }
        }
        _ => tally.chars(OTHER, json_chars(message)),
    }
}

fn classify_user_content(tally: &mut Tally, content: &Value) {
    match content {
        Value::String(text) => classify_user_text(tally, text),
        Value::Array(parts) => {
            for part in parts {
                match part["type"].as_str() {
                    Some("text") if part["text"].is_string() => {
                        classify_user_text(tally, part["text"].as_str().unwrap_or_default())
                    }
                    Some("image_url") => tally.fixed(ATTACHMENTS, IMAGE_TOKENS),
                    _ => tally.chars(OTHER, json_chars(part)),
                }
            }
        }
        Value::Null => {}
        other => tally.chars(OTHER, json_chars(other)),
    }
}

/// Counts a user text: the document blocks and the notes about images are attachments, the rest is the user's own text.
fn classify_user_text(tally: &mut Tally, text: &str) {
    let (mut user, mut attached) = (0, 0);
    let mut rest = text;
    while let Some(at) = block_start(rest) {
        let (before, from) = rest.split_at(at);
        let (before_user, before_attached) = split_notes(before);
        user += before_user;
        attached += before_attached;
        let end = from.find(DOCUMENT_CLOSE).map_or(from.len(), |i| i + DOCUMENT_CLOSE.len());
        attached += from[..end].chars().count();
        rest = &from[end..];
    }
    let (rest_user, rest_attached) = split_notes(rest);
    tally.chars(USER, user + rest_user);
    tally.chars(ATTACHMENTS, attached + rest_attached);
}

/// The start and the closing line of a document block, as `attachments::document_block` writes it.
const DOCUMENT_OPEN: &str = "<attached_file name=\"";
const DOCUMENT_CLOSE: &str = "\n</attached_file>";

/// Where the next document block starts in `text`: at its beginning, or after the blank line that joins the parts of
/// a message. None when there is none.
fn block_start(text: &str) -> Option<usize> {
    text.match_indices(DOCUMENT_OPEN).map(|(at, _)| at).find(|&at| at == 0 || text[..at].ends_with("\n\n"))
}

/// Splits text that has no document block into (user characters, attachment characters). The parts are joined by blank
/// lines, and the separators are counted as the user's.
fn split_notes(text: &str) -> (usize, usize) {
    let (mut user, mut attached) = (0, 0);
    for (index, segment) in text.split("\n\n").enumerate() {
        if index > 0 {
            user += 2;
        }
        let count = segment.chars().count();
        if is_attachment_note(segment) {
            attached += count;
        } else {
            user += count;
        }
    }
    (user, attached)
}

/// The notes that stand where an image or a document is left out of a request (see `wire_content` and core/chat/trim).
fn is_attachment_note(segment: &str) -> bool {
    let image = segment.starts_with("[image ")
        && (segment.ends_with(OMITTED_NOTE_END)
            || segment.ends_with(UNAVAILABLE_NOTE_END)
            || segment.contains(NOT_SENT_NOTE));
    let document = segment.starts_with("[file ") && segment.ends_with(OMITTED_NOTE_END);
    image || document
}

fn classify_assistant(tally: &mut Tally, message: &Value) {
    match &message["content"] {
        Value::String(text) => tally.chars(ASSISTANT, text.chars().count()),
        Value::Null => {}
        Value::Array(parts) => {
            for part in parts {
                match part["text"].as_str() {
                    Some(text) if part["type"] == "text" => tally.chars(ASSISTANT, text.chars().count()),
                    _ => tally.chars(OTHER, json_chars(part)),
                }
            }
        }
        other => tally.chars(OTHER, json_chars(other)),
    }
    match &message["tool_calls"] {
        Value::Null => {}
        Value::Array(calls) => {
            for call in calls {
                match (call["function"]["name"].as_str(), call["function"]["arguments"].as_str()) {
                    (Some(name), Some(arguments)) => {
                        tally.chars(ASSISTANT, name.chars().count() + arguments.chars().count());
                    }
                    _ => tally.chars(OTHER, json_chars(call)),
                }
            }
        }
        other => tally.chars(OTHER, json_chars(other)),
    }
    for key in REASONING_KEYS {
        if let Some(value) = message.get(key) {
            tally.chars(OTHER, json_chars(value));
        }
    }
}

/// Learns how far the estimate is from the provider's count, for one conversation. `ratio` starts at 1 and each reply
/// that reports its prompt size moves it halfway to the new sample, kept within [0.5, 2]. `observed` is true once a
/// reply has reported a count.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Calibrator {
    ratio: f64,
    observed: bool,
}

impl Default for Calibrator {
    fn default() -> Self {
        Calibrator { ratio: 1.0, observed: false }
    }
}

impl Calibrator {
    pub fn ratio(&self) -> f64 {
        self.ratio
    }

    pub fn observed(&self) -> bool {
        self.observed
    }

    /// Learns from one reply: `estimated` is the raw estimate of the request that was sent, `actual` the provider's
    /// prompt count. A zero on either side says nothing and is ignored.
    pub fn observe(&mut self, estimated: u32, actual: u32) {
        if estimated == 0 || actual == 0 {
            return;
        }
        let sample = f64::from(actual) / f64::from(estimated);
        self.ratio = (self.ratio * 0.5 + sample * 0.5).clamp(MIN_RATIO, MAX_RATIO);
        self.observed = true;
    }

    /// A raw estimate, calibrated and rounded.
    pub fn apply(&self, estimate: u32) -> u32 {
        (self.ratio * f64::from(estimate)).round() as u32
    }
}

/// What the context meter shows for one request. `used` is the estimate of the prompt, or the provider's count once
/// a reply has reported it (see `with_actual`). `limit` is None when the model's context limit is not known.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContextUsage {
    pub limit: Option<u32>,
    pub used: u32,
    pub reserved_output: u32,
    /// True when older content was left out of the request. Always false until trimming is built.
    pub trimmed: bool,
    /// True once a reply has reported the provider's count in this conversation.
    pub calibrated: bool,
    pub categories: Vec<CategoryUsage>,
}

/// The tokens of one category. `used` is the sum of the categories.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CategoryUsage {
    pub id: String,
    pub tokens: u32,
}

/// The usage of a request as estimated: each category calibrated, `used` their sum, and the output reserve the request
/// keeps (`max_tokens`, or `DEFAULT_OUTPUT_RESERVE`).
pub fn usage(breakdown: &Breakdown, calibrator: &Calibrator, limit: Option<u32>, max_tokens: Option<u32>) -> ContextUsage {
    let categories: Vec<CategoryUsage> = breakdown
        .categories
        .iter()
        .map(|(id, tokens)| CategoryUsage { id: id.clone(), tokens: calibrator.apply(*tokens) })
        .collect();
    let used = categories.iter().fold(0u32, |sum, category| sum.saturating_add(category.tokens));
    ContextUsage {
        limit,
        used,
        reserved_output: max_tokens.unwrap_or(DEFAULT_OUTPUT_RESERVE),
        trimmed: false,
        calibrated: calibrator.observed(),
        categories,
    }
}

/// The usage of the same request once the provider has counted its prompt: the categories are scaled so they sum to
/// `actual`, and `used` is `actual`. The rounding difference goes to the largest category. When the estimate has no
/// tokens at all, `actual` goes to `OTHER`. The result is calibrated: a count from the provider is the calibration.
pub fn with_actual(estimate: &ContextUsage, actual: u32) -> ContextUsage {
    let mut categories = estimate.categories.clone();
    let sum: u64 = categories.iter().map(|c| u64::from(c.tokens)).sum();
    match sum {
        0 => match categories.iter_mut().find(|c| c.id == OTHER) {
            Some(other) => other.tokens = actual,
            None => categories.push(CategoryUsage { id: OTHER.to_string(), tokens: actual }),
        },
        sum => {
            let actual_wide = u64::from(actual);
            for category in categories.iter_mut() {
                // Each category is scaled and rounded to the nearest token.
                let scaled = (u64::from(category.tokens) * actual_wide + sum / 2) / sum;
                category.tokens = u32::try_from(scaled).unwrap_or(u32::MAX);
            }
            let scaled_sum: i64 = categories.iter().map(|c| i64::from(c.tokens)).sum();
            let difference = i64::from(actual) - scaled_sum;
            let largest_tokens = categories.iter().map(|c| c.tokens).max().unwrap_or(0);
            if let Some(largest) = categories.iter_mut().find(|c| c.tokens == largest_tokens) {
                // A negative result is only possible when the largest category is a few tokens, and it stops at zero.
                largest.tokens = u32::try_from(i64::from(largest.tokens) + difference).unwrap_or(0);
            }
        }
    }
    let mut out = estimate.clone();
    out.used = actual;
    out.calibrated = true;
    out.categories = categories;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::chat::attachments::document_block;
    use serde_json::json;

    fn tokens(b: &Breakdown, id: &str) -> u32 {
        b.categories.iter().find(|(c, _)| c == id).map(|(_, t)| *t).expect("every category is listed")
    }

    fn ids(b: &Breakdown) -> Vec<&str> {
        b.categories.iter().map(|(id, _)| id.as_str()).collect()
    }

    fn one_user(content: Value) -> Breakdown {
        breakdown("", &[], &[json!({"role": "user", "content": content})])
    }

    #[test]
    fn estimate_is_a_quarter_of_a_token_per_character_rounded_up() {
        assert_eq!(estimate_tokens(0), 0);
        assert_eq!(estimate_tokens(1), 1);
        assert_eq!(estimate_tokens(4), 1);
        assert_eq!(estimate_tokens(5), 2);
        assert_eq!(estimate_tokens(4000), 1000);
    }

    #[test]
    fn characters_are_counted_not_bytes() {
        // Three characters of three bytes each: nine bytes, three characters, one token.
        assert_eq!(tokens(&one_user(json!("日本語")), USER), 1 + MESSAGE_OVERHEAD);
    }

    #[test]
    fn the_categories_are_listed_in_order_and_zero_ones_are_kept() {
        let b = breakdown("", &[], &[]);
        assert_eq!(ids(&b), ["system", "tools", "user", "attachments", "assistant", "tool_output", "other"]);
        assert!(b.categories.iter().all(|(_, t)| *t == 0));
        assert_eq!(b.total(), 0);
    }

    #[test]
    fn the_system_text_is_counted_once_even_with_a_system_message_in_the_list() {
        let system = "You are the terminal agent.";
        let messages = [json!({"role": "system", "content": system}), json!({"role": "user", "content": "hi"})];
        let b = breakdown(system, &[], &messages);
        assert_eq!(tokens(&b, SYSTEM), estimate_tokens(system.chars().count()));
        assert_eq!(b.total(), tokens(&b, SYSTEM) + tokens(&b, USER));
    }

    #[test]
    fn tool_definitions_are_counted_as_their_serialised_text() {
        let tools = [json!({"type": "function", "function": {"name": "get_cwd", "parameters": {}}})];
        let b = breakdown("", &tools, &[]);
        let chars = serde_json::to_string(&tools).unwrap().chars().count();
        assert_eq!(tokens(&b, TOOLS), estimate_tokens(chars));
        assert_eq!(tokens(&breakdown("", &[], &[]), TOOLS), 0, "no tools, nothing counted");
    }

    #[test]
    fn a_user_text_is_user_with_its_message_overhead() {
        let b = one_user(json!("hello"));
        assert_eq!(tokens(&b, USER), estimate_tokens(5) + MESSAGE_OVERHEAD);
        assert_eq!(tokens(&b, ATTACHMENTS), 0);
    }

    #[test]
    fn an_image_part_is_a_flat_estimate_and_its_data_url_is_not_counted() {
        let huge = format!("data:image/png;base64,{}", "A".repeat(200_000));
        let b = one_user(json!([
            {"type": "text", "text": "look"},
            {"type": "image_url", "image_url": {"url": huge}},
        ]));
        assert_eq!(tokens(&b, ATTACHMENTS), IMAGE_TOKENS, "the image costs the flat estimate, whatever its length");
        assert_eq!(tokens(&b, USER), estimate_tokens(4) + MESSAGE_OVERHEAD);
    }

    #[test]
    fn a_document_block_is_an_attachment_not_user_text() {
        let text = "x".repeat(400);
        let block = document_block("notes.txt", &text);
        let b = one_user(json!(format!("look\n\n{block}")));
        assert_eq!(tokens(&b, ATTACHMENTS), estimate_tokens(block.chars().count()));
        // The user's own words and the blank line between the parts are the user's.
        assert_eq!(tokens(&b, USER), estimate_tokens("look\n\n".chars().count()) + MESSAGE_OVERHEAD);
    }

    #[test]
    fn a_document_as_its_own_part_is_an_attachment() {
        let block = document_block("a.txt", "some text");
        let b = one_user(json!([{"type": "text", "text": block.clone()}, {"type": "text", "text": "and this"}]));
        assert_eq!(tokens(&b, ATTACHMENTS), estimate_tokens(block.chars().count()));
        assert_eq!(tokens(&b, USER), estimate_tokens("and this".chars().count()) + MESSAGE_OVERHEAD);
    }

    #[test]
    fn omitted_image_notes_are_attachments() {
        let omitted = "[image pic.png omitted from context]";
        let gone = "[image b.png is no longer available]";
        let b = one_user(json!(format!("hi\n\n{omitted}\n\n{gone}")));
        // The two notes are attachments, counted together; the words and the blank lines are the user's.
        let notes = omitted.chars().count() + gone.chars().count();
        assert_eq!(tokens(&b, ATTACHMENTS), estimate_tokens(notes));
        assert_eq!(tokens(&b, USER), estimate_tokens(2 + 2 + 2) + MESSAGE_OVERHEAD);
        let refused = one_user(json!("[image a.png not sent: the current model does not accept images]"));
        assert_eq!(tokens(&refused, ATTACHMENTS), estimate_tokens(refused_len()));
        assert_eq!(tokens(&refused, USER), MESSAGE_OVERHEAD, "the note is not the user's text");
    }

    fn refused_len() -> usize {
        "[image a.png not sent: the current model does not accept images]".chars().count()
    }

    #[test]
    fn a_document_left_out_of_a_trimmed_request_is_an_attachment() {
        let note = "[file notes.txt omitted from context]";
        let b = one_user(json!(format!("hi\n\n{note}")));
        assert_eq!(tokens(&b, ATTACHMENTS), estimate_tokens(note.chars().count()));
        assert_eq!(tokens(&b, USER), estimate_tokens(2 + 2) + MESSAGE_OVERHEAD, "the words and the blank line are the user's");
    }

    #[test]
    fn assistant_text_and_tool_calls_are_assistant() {
        let message = json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "get_cwd", "arguments": "{}"}}],
        });
        let b = breakdown("", &[], &[message]);
        // Name (7) and arguments (2) are nine characters: three tokens, plus the message overhead.
        assert_eq!(tokens(&b, ASSISTANT), estimate_tokens(9) + MESSAGE_OVERHEAD);
        assert_eq!(tokens(&b, OTHER), 0);
    }

    #[test]
    fn tool_results_are_tool_output() {
        let b = breakdown("", &[], &[json!({"role": "tool", "tool_call_id": "c1", "content": "output text"})]);
        assert_eq!(tokens(&b, TOOL_OUTPUT), estimate_tokens(11) + MESSAGE_OVERHEAD);
        assert_eq!(tokens(&b, USER), 0);
    }

    #[test]
    fn odd_shapes_are_other_and_never_panic() {
        let messages = [
            json!({"role": "mystery", "content": "x"}),
            json!(3),
            json!(null),
            json!({"role": "user", "content": 5}),
            json!({"role": "user", "content": [{"type": "video", "url": "u"}]}),
            json!({"role": "assistant", "content": "ok", "reasoning_content": "thinking it through"}),
            json!({"role": "assistant", "content": null, "tool_calls": [{"id": "c9"}]}),
            json!({"role": "tool", "tool_call_id": "c1", "content": {"nested": true}}),
        ];
        let b = breakdown("", &[], &messages);
        assert!(tokens(&b, OTHER) > 0);
        assert_eq!(tokens(&b, SYSTEM), 0);
        assert_eq!(b.total(), b.categories.iter().map(|(_, t)| t).sum::<u32>());
    }

    #[test]
    fn a_message_with_no_role_is_other() {
        let b = breakdown("", &[], &[json!({})]);
        assert_eq!(tokens(&b, OTHER), estimate_tokens(2), "the serialised `{{}}`");
        assert_eq!(b.total(), tokens(&b, OTHER));
    }

    #[test]
    fn the_calibrator_starts_at_one_and_moves_halfway_to_each_sample() {
        let mut c = Calibrator::default();
        assert_eq!(c.ratio(), 1.0);
        assert!(!c.observed());
        c.observe(100, 200);
        assert!((c.ratio() - 1.5).abs() < 1e-9, "{}", c.ratio());
        assert!(c.observed());
        c.observe(100, 200);
        assert!((c.ratio() - 1.75).abs() < 1e-9, "{}", c.ratio());
        assert_eq!(c.apply(40), 70);
    }

    #[test]
    fn the_calibrator_is_kept_between_half_and_twice() {
        let mut high = Calibrator::default();
        high.observe(1, 1000);
        assert_eq!(high.ratio(), 2.0);
        let mut low = Calibrator::default();
        low.observe(1000, 1);
        assert!((low.ratio() - 0.5005).abs() < 1e-9, "{}", low.ratio());
        low.observe(1000, 1);
        assert_eq!(low.ratio(), 0.5);
    }

    #[test]
    fn a_zero_on_either_side_is_ignored() {
        let mut c = Calibrator::default();
        c.observe(0, 5);
        c.observe(5, 0);
        assert_eq!(c.ratio(), 1.0);
        assert!(!c.observed());
    }

    #[test]
    fn usage_sums_the_calibrated_categories_and_keeps_the_reserve() {
        let b = breakdown("abcdefgh", &[], &[json!({"role": "user", "content": "hello"})]);
        let u = usage(&b, &Calibrator::default(), Some(100), None);
        assert_eq!(u.limit, Some(100));
        assert_eq!(u.used, 2 + 6);
        assert_eq!(u.used, u.categories.iter().map(|c| c.tokens).sum::<u32>());
        assert_eq!(u.reserved_output, DEFAULT_OUTPUT_RESERVE);
        assert!(!u.calibrated && !u.trimmed);
        assert_eq!(usage(&b, &Calibrator::default(), None, Some(512)).reserved_output, 512);
    }

    #[test]
    fn usage_applies_the_calibrator_to_each_category() {
        let b = breakdown("abcdefgh", &[], &[json!({"role": "user", "content": "hello"})]);
        let mut c = Calibrator::default();
        c.observe(100, 150); // ratio 1.25
        let u = usage(&b, &c, None, None);
        assert!(u.calibrated);
        assert_eq!(u.categories.iter().find(|x| x.id == SYSTEM).unwrap().tokens, 3);
        assert_eq!(u.used, u.categories.iter().map(|c| c.tokens).sum::<u32>());
    }

    #[test]
    fn with_actual_scales_to_the_provider_count_and_sums_to_it() {
        let b = breakdown("abcdefgh", &[], &[json!({"role": "user", "content": "hello"})]);
        let estimate = usage(&b, &Calibrator::default(), Some(100), None);
        let actual = with_actual(&estimate, 20);
        assert_eq!(actual.used, 20);
        assert_eq!(actual.categories.iter().map(|c| c.tokens).sum::<u32>(), 20);
        assert_eq!(actual.limit, Some(100), "the rest of the usage is kept");
        assert!(actual.calibrated, "a provider count is calibrated");
        assert!(!estimate.calibrated, "the estimate itself is not changed");
    }

    #[test]
    fn the_rounding_difference_goes_to_the_largest_category() {
        let estimate = ContextUsage {
            limit: None,
            used: 3,
            reserved_output: 4096,
            trimmed: false,
            calibrated: false,
            categories: ["system", "tools", "user"]
                .iter()
                .map(|id| CategoryUsage { id: id.to_string(), tokens: 1 })
                .collect(),
        };
        // Each of three ones scales to 10/3, which rounds to 3 each: nine, so the first largest takes the missing one.
        let actual = with_actual(&estimate, 10);
        let tokens: Vec<u32> = actual.categories.iter().map(|c| c.tokens).collect();
        assert_eq!(tokens, [4, 3, 3]);
        assert_eq!(actual.used, 10);
    }

    #[test]
    fn with_actual_on_an_empty_estimate_puts_the_count_in_other() {
        let empty = usage(&breakdown("", &[], &[]), &Calibrator::default(), None, None);
        let actual = with_actual(&empty, 7);
        assert_eq!(actual.used, 7);
        assert_eq!(actual.categories.iter().find(|c| c.id == OTHER).unwrap().tokens, 7);
        assert_eq!(actual.categories.iter().map(|c| c.tokens).sum::<u32>(), 7);
    }

    #[test]
    fn the_usage_serialises_with_the_field_names_the_frontend_reads() {
        let u = usage(&breakdown("abcd", &[], &[]), &Calibrator::default(), None, Some(256));
        let value = serde_json::to_value(&u).unwrap();
        assert_eq!(value["limit"], Value::Null);
        assert_eq!(value["reserved_output"], 256);
        assert_eq!(value["trimmed"], false);
        assert_eq!(value["calibrated"], false);
        assert_eq!(value["categories"][0], json!({"id": "system", "tokens": 1}));
    }
}
