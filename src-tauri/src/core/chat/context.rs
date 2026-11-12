//! The context budget, slice 1: what a model request costs in tokens, by category, and how that estimate is calibrated
//! against the provider's own count. Nothing here changes a request. The breakdown is made from the wire messages as
//! they are sent, so it counts what the model reads, and the usage is what the context meter shows. The category ids
//! are strings, so a new category needs no change to the frontend's type.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::llm::LlmError;

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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// The largest limit or count a stored usage may hold, and the most categories it may list. Anything above is taken for
/// a damaged file.
const SANE_TOKENS: u32 = 100_000_000;
const SANE_CATEGORIES: usize = 16;
/// The longest category id a stored usage may hold, in bytes (ids are ASCII).
const SANE_ID_BYTES: usize = 40;

impl ContextUsage {
    /// True when a usage read from a file could have come from `usage` or `with_actual`: the counts are not absurd, there
    /// are few categories, each id is a short lower-case word, and the categories sum to `used` (to within one token of
    /// rounding). A usage that fails this is dropped when it is read (see `ConversationStore`).
    pub fn is_sane(&self) -> bool {
        let ids_ok = self.categories.iter().all(|c| {
            !c.id.is_empty()
                && c.id.len() <= SANE_ID_BYTES
                && c.id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        });
        let sum: u64 = self.categories.iter().map(|c| u64::from(c.tokens)).sum();
        self.used <= SANE_TOKENS
            && self.limit.is_none_or(|limit| limit <= SANE_TOKENS)
            && self.categories.len() <= SANE_CATEGORIES
            && ids_ok
            && sum.abs_diff(u64::from(self.used)) <= 1
    }
}

/// The tokens of one category. `used` is the sum of the categories.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// The words of a provider's error for a prompt that is too large, in lower case. They are matched in the message and in
/// the error kind. A 413 is an overflow whatever its text says (see `is_context_overflow`).
const OVERFLOW_WORDS: [&str; 13] = [
    "context length",
    "context_length",
    "context window",
    "context size",
    "maximum context",
    "max context",
    "prompt is too long",
    "input is too long",
    "too many tokens",
    "reduce the length of the messages",
    "request too large",
    "request entity too large",
    "payload too large",
];

/// The words of a provider's error about how often or how much the account may use the model (a rate limit, a quota), in
/// lower case. Such an error is not an overflow even when its text also has one of `OVERFLOW_WORDS`, such as "too many
/// tokens per minute".
const LIMIT_ON_USE_WORDS: [&str; 8] =
    ["per minute", "per day", "per hour", "rate limit", "rate-limit", "rate_limit", "quota", "requests per"];

/// True for an error that says the request is too large for the model's context window: a 413, or a 400 or 422 (or a
/// provider error inside a 200 body) whose message or kind has one of `OVERFLOW_WORDS` and none of `LIMIT_ON_USE_WORDS`.
/// Any other error is not an overflow, whatever its wording.
pub fn is_context_overflow(e: &LlmError) -> bool {
    match e {
        LlmError::Http { status: 413, .. } => true,
        LlmError::Http { status: 400 | 422, kind, message, .. } | LlmError::Provider { kind, message, .. } => {
            let text = format!("{} {message}", kind.as_deref().unwrap_or_default()).to_lowercase();
            let overflow = OVERFLOW_WORDS.iter().any(|word| text.contains(word));
            overflow && !LIMIT_ON_USE_WORDS.iter().any(|word| text.contains(word))
        }
        _ => false,
    }
}

/// The words after which a provider names the model's context limit, in lower case.
const LIMIT_ANCHORS: [&str; 7] =
    ["maximum context length", "context length", "context window", "context size", "max context", "limit of", "limit is"];
/// How many characters after an anchor the number may start.
const LIMIT_WINDOW: usize = 60;
/// The smallest and the largest context limit that is taken from a message.
pub const LIMIT_MIN: u32 = 2000;
pub const LIMIT_MAX: u32 = 100_000_000;

/// The context limit that a provider's error text names, if it names one: the first number of at least 1000 (commas
/// allowed, as in "8,192") that starts within 60 characters after one of `LIMIT_ANCHORS`, taking the anchors in the order
/// they appear. None when there is no such number, or when it is outside [`LIMIT_MIN`, `LIMIT_MAX`].
pub fn limit_in_message(msg: &str) -> Option<u32> {
    let lower = msg.to_lowercase();
    let mut ends: Vec<usize> = LIMIT_ANCHORS
        .iter()
        .flat_map(|anchor| lower.match_indices(anchor).map(move |(at, _)| at + anchor.len()))
        .collect();
    ends.sort_unstable();
    let number = ends.into_iter().find_map(|end| first_number_at_least(&lower[end..], 1000, LIMIT_WINDOW))?;
    u32::try_from(number).ok().filter(|n| (LIMIT_MIN..=LIMIT_MAX).contains(n))
}

/// The first run of digits in `text` (commas allowed inside it) that reads as at least `min` and starts before the
/// character at `starts_before`. A run is read whole, so a number is not cut at that point.
fn first_number_at_least(text: &str, min: u64, starts_before: usize) -> Option<u64> {
    let mut run = String::new();
    let mut start = 0;
    // A space at the end closes a run that runs to the end of the text.
    for (index, c) in text.chars().chain(std::iter::once(' ')).enumerate() {
        if c.is_ascii_digit() || (c == ',' && !run.is_empty()) {
            if run.is_empty() {
                start = index;
            }
            run.push(c);
            continue;
        }
        if start < starts_before
            && let Some(n) = run.replace(',', "").parse::<u64>().ok().filter(|n| *n >= min)
        {
            return Some(n);
        }
        run.clear();
    }
    None
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

    /// A usage with the given categories and used count, the rest as `usage` leaves it.
    fn stored_usage(used: u32, limit: Option<u32>, categories: &[(&str, u32)]) -> ContextUsage {
        ContextUsage {
            limit,
            used,
            reserved_output: 4096,
            trimmed: false,
            calibrated: true,
            categories: categories
                .iter()
                .map(|(id, tokens)| CategoryUsage { id: id.to_string(), tokens: *tokens })
                .collect(),
        }
    }

    #[test]
    fn a_usage_that_could_have_been_written_is_sane() {
        assert!(stored_usage(0, None, &[]).is_sane(), "an empty usage");
        assert!(stored_usage(30, Some(128_000), &[("system", 10), ("user", 20)]).is_sane());
        assert!(stored_usage(30, None, &[("system", 10), ("user", 19)]).is_sane(), "one token of rounding is allowed");
        assert!(stored_usage(30, None, &[("system", 10), ("user", 21)]).is_sane(), "and one token the other way");
        assert!(stored_usage(100_000_000, Some(100_000_000), &[("other", 100_000_000)]).is_sane(), "the largest counts");
        let sixteen: Vec<(&str, u32)> = (0..16).map(|_| ("x", 1)).collect();
        assert!(stored_usage(16, None, &sixteen).is_sane(), "sixteen categories");
        assert!(stored_usage(1, None, &[("a_1_b", 1)]).is_sane(), "lower case, digits and underscores");
    }

    #[test]
    fn a_damaged_usage_is_not_sane() {
        let long_id = "a".repeat(41);
        let cases = [
            ("categories sum to less than used", stored_usage(30, None, &[("system", 10)])),
            ("categories sum to more than used", stored_usage(10, None, &[("system", 10), ("user", 10)])),
            ("used above the largest count", stored_usage(100_000_001, None, &[("other", 100_000_001)])),
            ("limit above the largest count", stored_usage(1, Some(100_000_001), &[("other", 1)])),
            ("seventeen categories", stored_usage(17, None, &(0..17).map(|_| ("x", 1)).collect::<Vec<_>>())),
            ("a category id over 40 bytes", stored_usage(1, None, &[(long_id.as_str(), 1)])),
            ("an empty category id", stored_usage(1, None, &[("", 1)])),
            ("a category id with a space", stored_usage(1, None, &[("tool output", 1)])),
            ("an upper-case category id", stored_usage(1, None, &[("System", 1)])),
            ("a category id with a dash", stored_usage(1, None, &[("tool-output", 1)])),
            ("a category id with a non-ASCII letter", stored_usage(1, None, &[("é", 1)])),
        ];
        for (what, usage) in cases {
            assert!(!usage.is_sane(), "{what} must not be sane: {usage:?}");
        }
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

    fn http(status: u16, kind: Option<&str>, message: &str) -> LlmError {
        LlmError::Http { status, code: None, kind: kind.map(str::to_string), message: message.to_string() }
    }

    fn provider(kind: Option<&str>, message: &str) -> LlmError {
        LlmError::Provider { code: None, kind: kind.map(str::to_string), message: message.to_string() }
    }

    #[test]
    fn overflow_errors_are_a_413_or_a_400_422_or_provider_error_with_the_wording() {
        assert!(is_context_overflow(&http(413, None, "too big")), "a 413 whatever its text");
        assert!(is_context_overflow(&http(400, None, "This model's maximum context length is 128000 tokens.")));
        assert!(is_context_overflow(&http(400, Some("context_length_exceeded"), "Bad")), "the kind alone is enough");
        assert!(is_context_overflow(&http(422, None, "Prompt is too long: 250000 tokens > 200000 maximum")));
        assert!(is_context_overflow(&http(400, None, "Input is too long for requested model.")));
        assert!(is_context_overflow(&http(400, None, "Request too large for this model")));
        assert!(is_context_overflow(&http(400, None, "Please reduce the length of the messages or completion.")));
        assert!(is_context_overflow(&provider(None, "Your input exceeds the context window of this model")));
        assert!(is_context_overflow(&provider(Some("invalid_request_error"), "too many tokens in the prompt")));
    }

    #[test]
    fn other_errors_are_not_overflows() {
        assert!(!is_context_overflow(&http(401, None, "invalid key, maximum context length")), "a 401 is not an overflow");
        assert!(!is_context_overflow(&http(429, None, "rate limit exceeded")));
        assert!(!is_context_overflow(&http(500, None, "context length: internal failure")));
        assert!(!is_context_overflow(&http(400, None, "invalid model")));
        assert!(!is_context_overflow(&http(400, None, "bad request")));
        assert!(!is_context_overflow(&http(400, None, "rate limit reached, try again")));
        assert!(!is_context_overflow(&http(422, None, "image input is not supported")));
        assert!(!is_context_overflow(&LlmError::Transport("context length".into())));
        assert!(!is_context_overflow(&LlmError::Parse("too many tokens".into())));
        assert!(!is_context_overflow(&LlmError::Cancelled));
    }

    #[test]
    fn a_rate_limit_or_quota_error_is_not_an_overflow_even_when_it_says_too_many_tokens() {
        assert!(!is_context_overflow(&http(400, None, "Rate limit reached: too many tokens per minute")));
        assert!(!is_context_overflow(&http(429, None, "Rate limit reached: too many tokens per minute")));
        assert!(!is_context_overflow(&http(400, None, "quota exceeded: too many tokens")));
        assert!(!is_context_overflow(&provider(Some("rate_limit_error"), "too many tokens in the prompt")));
        assert!(!is_context_overflow(&http(422, None, "too many requests per day: too many tokens")));
        assert!(!is_context_overflow(&http(400, None, "rate-limited: too many tokens")));
        // The 413 is an overflow whatever its text says, as before.
        assert!(is_context_overflow(&http(413, None, "Rate limit reached: too many tokens per minute")));
    }

    #[test]
    fn the_limit_is_the_first_number_after_an_anchor() {
        let openai = "This model's maximum context length is 128000 tokens. However, your messages resulted in 130123 tokens.";
        assert_eq!(limit_in_message(openai), Some(128000), "the first number, not the one that came to the request");
        assert_eq!(limit_in_message("context length of 8,192 tokens"), Some(8192));
        assert_eq!(limit_in_message("Context window: 1,000,000 tokens"), Some(1_000_000));
        assert_eq!(limit_in_message("the limit is 32768"), Some(32768));
        assert_eq!(limit_in_message("limit of 4096, and more"), Some(4096));
        assert_eq!(limit_in_message("Your input has 130123 tokens, above the context length of 128000."), Some(128000));
    }

    #[test]
    fn a_limit_that_is_missing_small_far_or_out_of_range_is_none() {
        assert_eq!(limit_in_message("prompt is too long"), None, "no anchor");
        assert_eq!(limit_in_message("130123 tokens is more than the context length"), None, "the number is before the anchor");
        assert_eq!(limit_in_message("context length is 512 tokens"), None, "below 1000");
        assert_eq!(limit_in_message("context length is 1500 tokens"), None, "below the 2000 minimum");
        assert_eq!(limit_in_message("context length is 200000000 tokens"), None, "above the maximum");
        assert_eq!(limit_in_message(&format!("context length{}128000", " ".repeat(60))), None, "starts past 60 characters");
    }

    #[test]
    fn a_number_is_read_whole_when_it_starts_within_the_window() {
        let at_59 = format!("context length{}128000", " ".repeat(59));
        assert_eq!(limit_in_message(&at_59), Some(128000), "starts at character 59, so it is read whole");
    }
}
