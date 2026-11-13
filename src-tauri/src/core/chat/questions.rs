//! The clarification questions the model may ask the user mid-turn: the `ask_user` tool, its validation, the
//! result it returns to the model, and the gate a turn waits on.
//!
//! The tool is chat-local like `todo_write`: the agent loop handles it itself, so it never touches the shell or the
//! approval decision. The model sends 1 to 4 multiple-choice questions; the UI answers them, skips them, or leaves
//! them unsettled (timeout, cancel). Whatever settles them becomes the tool result, so the turn always goes on: a
//! question result is never an error and never carries the denial prefix.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::core::tools::ToolSchema;

pub const TOOL_NAME: &str = "ask_user";
pub const MAX_QUESTIONS: usize = 4;
pub const MAX_OPTIONS: usize = 4;
pub const MIN_OPTIONS: usize = 2;
pub const MAX_HEADER: usize = 12;
pub const MAX_QUESTION: usize = 500;
pub const MAX_LABEL: usize = 80;
pub const MAX_DESCRIPTION: usize = 300;
pub const MAX_OTHER: usize = 2000;
/// How long a question waits for the UI before the turn goes on without the answer.
pub const TIMEOUT: Duration = Duration::from_secs(300);
/// The model-facing result is capped here, not at the small limit other tools use, so the answers survive.
pub const ANSWER_LIMIT: usize = 20000;

/// One option of a question, as the model sends it and the UI shows it. Free text ("Other") is always available and
/// is not an option.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub recommended: bool,
}

/// One question the model asked. `id` is assigned by the backend in order (`q1` and up).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    pub id: String,
    pub question: String,
    pub header: String,
    pub options: Vec<QuestionOption>,
    #[serde(default)]
    pub multiple: bool,
}

/// The UI's answer to one question: the chosen option indices, free text, or skipped. An empty `other` counts as
/// absent (the field just was not cleared).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerInput {
    pub question_id: String,
    #[serde(default)]
    pub selected: Vec<u32>,
    #[serde(default)]
    pub other: Option<String>,
    #[serde(default)]
    pub skipped: bool,
}

/// The `ask_user` tool as the model sees it. It is not one of the backend tools in `tools::schemas()`: the agent loop
/// handles it itself.
pub fn schema() -> ToolSchema {
    ToolSchema {
        name: TOOL_NAME,
        description: "Ask the user 1 to 4 clarification questions and wait for the answer. Use it for preferences, \
            ambiguity and implementation choices. Never for permission (approval handles that), and never for facts a \
            read tool can answer. Give 2 to 4 short options per question, at most one recommended, and free text is \
            always available. The result tells you what the user chose, skipped, or left unanswered (timed_out, \
            cancelled): then continue on your best judgment without asking the same question again.",
        parameters: json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": MAX_QUESTIONS,
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": {"type": "string", "maxLength": MAX_QUESTION},
                            "header": {"type": "string", "maxLength": MAX_HEADER},
                            "multiple": {"type": "boolean"},
                            "options": {
                                "type": "array",
                                "minItems": MIN_OPTIONS,
                                "maxItems": MAX_OPTIONS,
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": {"type": "string", "maxLength": MAX_LABEL},
                                        "description": {"type": "string", "maxLength": MAX_DESCRIPTION},
                                        "recommended": {"type": "boolean"},
                                    },
                                    "required": ["label"],
                                },
                            },
                        },
                        "required": ["question", "header", "options"],
                    },
                },
            },
            "required": ["questions"],
        }),
    }
}

/// Validates the arguments of an `ask_user` call and assigns the question ids in order. The error text is for the
/// model: it names the question and the rule.
pub fn parse(args: &Value) -> Result<Vec<Question>, String> {
    let items = args
        .get("questions")
        .and_then(Value::as_array)
        .ok_or_else(|| "questions must be an array of 1 to 4 questions".to_string())?;
    if items.is_empty() || items.len() > MAX_QUESTIONS {
        return Err(format!("questions has {} items; send 1 to {MAX_QUESTIONS} questions", items.len()));
    }
    items.iter().enumerate().map(|(index, item)| parse_question(index + 1, item)).collect()
}

/// Validates one question: non-empty text within the limits, 2 to 4 options, at most one recommended.
fn parse_question(number: usize, item: &Value) -> Result<Question, String> {
    let text = |field: &str, limit: usize| {
        let raw = item.get(field).and_then(Value::as_str).unwrap_or_default();
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(format!("question {number}: {field} is empty"));
        }
        let length = trimmed.chars().count();
        if length > limit {
            return Err(format!("question {number}: {field} is {length} characters; the limit is {limit}"));
        }
        Ok(trimmed.to_string())
    };
    let question = text("question", MAX_QUESTION)?;
    let header = text("header", MAX_HEADER)?;
    let options = item.get("options").and_then(Value::as_array).ok_or_else(|| {
        format!("question {number}: options must be an array of {MIN_OPTIONS} to {MAX_OPTIONS} options")
    })?;
    if options.len() < MIN_OPTIONS || options.len() > MAX_OPTIONS {
        return Err(format!(
            "question {number}: options has {} items; send {MIN_OPTIONS} to {MAX_OPTIONS} options",
            options.len()
        ));
    }
    let mut parsed = Vec::with_capacity(options.len());
    for (index, option) in options.iter().enumerate() {
        let option_number = index + 1;
        let raw_label = option.get("label").and_then(Value::as_str).unwrap_or_default();
        let label = raw_label.trim();
        if label.is_empty() {
            return Err(format!("question {number} option {option_number}: label is empty"));
        }
        let length = label.chars().count();
        if length > MAX_LABEL {
            return Err(format!(
                "question {number} option {option_number}: label is {length} characters; the limit is {MAX_LABEL}"
            ));
        }
        let description = option.get("description").and_then(Value::as_str).unwrap_or_default().trim().to_string();
        let length = description.chars().count();
        if length > MAX_DESCRIPTION {
            return Err(format!(
                "question {number} option {option_number}: description is {length} characters; the limit is \
                 {MAX_DESCRIPTION}"
            ));
        }
        let recommended = option.get("recommended").and_then(Value::as_bool).unwrap_or(false);
        parsed.push(QuestionOption { label: label.to_string(), description, recommended });
    }
    if parsed.iter().filter(|o| o.recommended).count() > 1 {
        return Err(format!("question {number}: at most one option may be recommended"));
    }
    Ok(Question {
        id: format!("q{number}"),
        question,
        header,
        options: parsed,
        multiple: item.get("multiple").and_then(Value::as_bool).unwrap_or(false),
    })
}

/// Validates the UI's answers against the stored questions: exactly one answer per question, no unknown or duplicate
/// ids, indices in range and unique, single-select at most one, other text within the limit, and a skipped answer with
/// nothing in it. The error text is for the UI, which can show it and let the user retry: the question stays pending.
pub fn validate_answers(questions: &[Question], answers: &[AnswerInput]) -> Result<(), String> {
    if answers.len() != questions.len() {
        return Err(format!("expected {} answers, got {}", questions.len(), answers.len()));
    }
    // The counts match, so known and unique ids mean every question is answered exactly once.
    let mut seen = HashSet::new();
    for answer in answers {
        if !seen.insert(answer.question_id.as_str()) {
            return Err(format!("duplicate answer for `{}`", answer.question_id));
        }
        let question = questions
            .iter()
            .find(|q| q.id == answer.question_id)
            .ok_or_else(|| format!("unknown question_id `{}`", answer.question_id))?;
        validate_answer(question, answer)?;
    }
    Ok(())
}

/// Validates one answer: indices name distinct options in range (at most one without `multiple`), other text fits, a
/// skip carries nothing, and a non-skip answers something (an option or free text). The UI never sends an empty
/// answer; this closes the hole for a malformed client.
fn validate_answer(question: &Question, answer: &AnswerInput) -> Result<(), String> {
    let other = answer.other.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if answer.skipped {
        if !answer.selected.is_empty() || other.is_some() {
            return Err(format!("question `{}`: a skipped answer has no selected options or other text", question.id));
        }
        return Ok(());
    }
    if answer.selected.is_empty() && other.is_none() {
        return Err(format!(
            "question `{}`: answer is empty; select an option, type an answer, or skip",
            question.id
        ));
    }
    let mut seen = HashSet::new();
    for &index in &answer.selected {
        if !seen.insert(index) {
            return Err(format!("question `{}`: option index {index} is answered twice", question.id));
        }
        if index as usize >= question.options.len() {
            return Err(format!(
                "question `{}`: option index {index} is out of range (0 to {})",
                question.id,
                question.options.len() - 1
            ));
        }
    }
    if !question.multiple && answer.selected.len() > 1 {
        return Err(format!("question `{}`: one option at most without multiple", question.id));
    }
    if let Some(text) = other {
        let length = text.chars().count();
        if length > MAX_OTHER {
            return Err(format!(
                "question `{}`: other text is {length} characters; the limit is {MAX_OTHER}",
                question.id
            ));
        }
    }
    Ok(())
}

/// The model-facing result of answered questions: one entry per question in order, with the ids, the text, the status,
/// the chosen labels, and the free text. `selected` is left out when empty, `other` when absent.
pub fn result(questions: &[Question], answers: &[AnswerInput]) -> Value {
    let by_id: HashMap<&str, &AnswerInput> = answers.iter().map(|a| (a.question_id.as_str(), a)).collect();
    let entries: Vec<Value> = questions
        .iter()
        .map(|question| {
            let mut entry = serde_json::Map::new();
            entry.insert("question_id".into(), json!(question.id));
            entry.insert("question".into(), json!(question.question));
            entry.insert("header".into(), json!(question.header));
            // A question without its answer cannot happen (decide only takes full sets); it reads as skipped.
            match by_id.get(question.id.as_str()).filter(|a| !a.skipped) {
                Some(answer) => {
                    entry.insert("status".into(), json!("answered"));
                    let labels: Vec<&str> = answer
                        .selected
                        .iter()
                        .filter_map(|&index| question.options.get(index as usize).map(|o| o.label.as_str()))
                        .collect();
                    if !labels.is_empty() {
                        entry.insert("selected".into(), json!(labels));
                    }
                    if let Some(text) = answer.other.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                        entry.insert("other".into(), json!(text));
                    }
                }
                None => {
                    entry.insert("status".into(), json!("skipped"));
                }
            }
            Value::Object(entry)
        })
        .collect();
    json!({"answers": entries})
}

/// The result when every question settles the same way without an answer: timed out, or cancelled.
pub fn status_result(questions: &[Question], status: &str) -> Value {
    let entries: Vec<Value> = questions
        .iter()
        .map(|question| {
            json!({"question_id": question.id, "question": question.question, "header": question.header, "status": status})
        })
        .collect();
    json!({"answers": entries})
}

/// The UI's reply to one waiting question: its answers, or cancelled when the wait was cut short.
#[derive(Debug)]
enum Reply {
    Answers(Vec<AnswerInput>),
    Cancelled,
}

/// One question waiting for the UI, keyed by the call id of its `QuestionRequest` event.
struct PendingAsk {
    questions: Vec<Question>,
    tx: Sender<Reply>,
}

/// Questions waiting for answers, keyed by call id. An ask nobody answers in time ends as timed out; a cancelled one
/// ends as cancelled. Both are results, never errors, so the turn goes on.
pub struct QuestionGate {
    pending: Mutex<HashMap<String, PendingAsk>>,
    timeout: Duration,
}

impl Default for QuestionGate {
    fn default() -> Self {
        Self::new()
    }
}

impl QuestionGate {
    pub fn new() -> Self {
        Self::with_timeout(TIMEOUT)
    }

    pub fn with_timeout(timeout: Duration) -> Self {
        QuestionGate { pending: Mutex::new(HashMap::new()), timeout }
    }

    /// Registers `call_id`, runs `announce` so the UI can show the question, then waits. The return is the model-facing
    /// result: answered, skipped, timed out, or cancelled. Nothing is left pending on any path.
    pub fn ask(&self, call_id: &str, questions: Vec<Question>, announce: impl FnOnce()) -> Value {
        let (tx, rx) = mpsc::channel();
        self.lock().insert(call_id.to_string(), PendingAsk { questions: questions.clone(), tx });
        announce();
        match rx.recv_timeout(self.timeout) {
            Ok(Reply::Answers(answers)) => {
                self.lock().remove(call_id);
                result(&questions, &answers)
            }
            Ok(Reply::Cancelled) => {
                self.lock().remove(call_id);
                status_result(&questions, "cancelled")
            }
            Err(RecvTimeoutError::Timeout) => {
                // A `decide` that removed the entry first still wins: its answer is already queued.
                self.lock().remove(call_id);
                match rx.try_recv() {
                    Ok(Reply::Answers(answers)) => result(&questions, &answers),
                    Ok(Reply::Cancelled) => status_result(&questions, "cancelled"),
                    Err(_) => status_result(&questions, "timed_out"),
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                self.lock().remove(call_id);
                status_result(&questions, "cancelled")
            }
        }
    }

    /// Answers a pending question. On invalid answers returns Err and keeps the question pending, so the UI can fix
    /// them and retry. Unknown ids are Err too.
    pub fn decide(&self, call_id: &str, answers: Vec<AnswerInput>) -> Result<(), String> {
        // The lock is held from the lookup through the send, so a timeout racing here either wins wholly (unknown id
        // there) or loses wholly (its try_recv finds this answer).
        let pending = &mut *self.lock();
        let Some(ask) = pending.get(call_id) else {
            return Err(format!("no pending question {call_id}"));
        };
        validate_answers(&ask.questions, &answers)?;
        let ask = pending.remove(call_id).expect("pending question checked above");
        let _ = ask.tx.send(Reply::Answers(answers));
        Ok(())
    }

    /// Cancels every pending question: each waiting ask returns a cancelled result.
    pub fn cancel_all(&self) {
        for (_, ask) in self.lock().drain() {
            let _ = ask.tx.send(Reply::Cancelled);
        }
    }

    /// True while `call_id` waits for the UI. For tests only: the wait itself is the coverage elsewhere.
    #[cfg(test)]
    fn is_pending(&self, call_id: &str) -> bool {
        self.lock().contains_key(call_id)
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, PendingAsk>> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;

    fn args(items: Value) -> Value {
        json!({"questions": items})
    }

    fn option(label: &str) -> Value {
        json!({"label": label, "description": format!("about {label}")})
    }

    fn single() -> Value {
        args(json!([{"question": "Which capa?", "header": "Capa", "options": [option("a"), option("b")]}]))
    }

    fn question(number: usize, multiple: bool) -> Question {
        Question {
            id: format!("q{number}"),
            question: "Which capa?".to_string(),
            header: "Capa".to_string(),
            options: vec![
                QuestionOption { label: "a".to_string(), description: "about a".to_string(), recommended: true },
                QuestionOption { label: "b".to_string(), description: "about b".to_string(), recommended: false },
            ],
            multiple,
        }
    }

    fn answer(id: &str, selected: &[u32], other: Option<&str>, skipped: bool) -> AnswerInput {
        AnswerInput {
            question_id: id.to_string(),
            selected: selected.to_vec(),
            other: other.map(str::to_string),
            skipped,
        }
    }

    fn err(args: Value) -> String {
        parse(&args).expect_err("refused")
    }

    #[test]
    fn valid_questions_get_ids_in_order_and_defaults() {
        let questions = parse(&single()).unwrap();
        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0].id, "q1");
        assert!(!questions[0].multiple);
        assert!(!questions[0].options[1].recommended);
        let multi = parse(&args(json!([
            {"question": "One?", "header": "First", "multiple": true, "options": [option("a"), option("b"), option("c")]},
            {"question": "Two?", "header": "Second", "options": [option("x"), option("y")]},
        ])))
        .unwrap();
        assert_eq!([multi[0].id.as_str(), multi[1].id.as_str()], ["q1", "q2"]);
        assert!(multi[0].multiple);
    }

    #[test]
    fn the_schema_names_the_tool_and_its_limits() {
        let s = schema();
        assert_eq!(s.name, "ask_user");
        assert_eq!(s.parameters["required"], json!(["questions"]));
        assert_eq!(s.parameters["properties"]["questions"]["maxItems"], json!(MAX_QUESTIONS));
        assert!(!s.description.is_empty());
    }

    #[test]
    fn questions_must_be_one_to_four() {
        assert!(err(json!({})).contains("questions must be an array"));
        assert!(err(args(json!([]))).contains("1 to 4"));
        let five: Vec<Value> = (0..5).map(|i| json!({"question": format!("Q{i}?"), "header": "H", "options": [option("a"), option("b")]})).collect();
        assert!(err(args(json!(five))).contains("1 to 4"));
    }

    #[test]
    fn text_fields_must_be_non_empty_and_within_their_limits() {
        assert!(err(args(json!([{"question": "   ", "header": "H", "options": [option("a"), option("b")]}]))).contains("question is empty"));
        assert!(err(args(json!([{"question": "Q?", "header": "", "options": [option("a"), option("b")]}]))).contains("header is empty"));
        let long = "é".repeat(MAX_QUESTION + 1);
        let why = err(args(json!([{"question": long, "header": "H", "options": [option("a"), option("b")]}])));
        assert!(why.contains("501 characters") && why.contains("limit is 500"), "{why}");
        let long_header = "h".repeat(MAX_HEADER + 1);
        let why = err(args(json!([{"question": "Q?", "header": long_header, "options": [option("a"), option("b")]}])));
        assert!(why.contains("13 characters") && why.contains("limit is 12"), "{why}");
        // The limits are in characters, and the boundary fits.
        let maxed = parse(&args(json!([{
            "question": "é".repeat(MAX_QUESTION),
            "header": "h".repeat(MAX_HEADER),
            "options": [{"label": "l".repeat(MAX_LABEL), "description": "d".repeat(MAX_DESCRIPTION)}, option("b")],
        }])))
        .unwrap();
        assert_eq!(maxed[0].options[0].label.chars().count(), MAX_LABEL);
    }

    #[test]
    fn options_must_be_two_to_four_with_labels_and_at_most_one_recommended() {
        let one = args(json!([{"question": "Q?", "header": "H", "options": [option("a")]}]));
        assert!(err(one).contains("2 to 4"), "one option");
        let five: Vec<Value> = (0..5).map(|i| option(&format!("o{i}"))).collect();
        assert!(err(args(json!([{"question": "Q?", "header": "H", "options": five}]))).contains("2 to 4"), "five options");
        let no_label = args(json!([{"question": "Q?", "header": "H", "options": [json!({"description": "x"}), option("b")]}]));
        assert!(err(no_label).contains("label is empty"));
        let long_label = "l".repeat(MAX_LABEL + 1);
        let why = err(args(json!([{"question": "Q?", "header": "H", "options": [json!({"label": long_label}), option("b")]}])));
        assert!(why.contains("81 characters") && why.contains("limit is 80"), "{why}");
        let long_desc = "d".repeat(MAX_DESCRIPTION + 1);
        let why = err(args(json!([{"question": "Q?", "header": "H", "options": [json!({"label": "a", "description": long_desc}), option("b")]}])));
        assert!(why.contains("301 characters") && why.contains("limit is 300"), "{why}");
        let both = || json!({"question": "Q?", "header": "H", "options": [
            {"label": "a", "recommended": true}, {"label": "b", "recommended": true}]});
        assert!(err(args(json!([both()]))).contains("at most one option may be recommended"));
    }

    #[test]
    fn answers_need_exactly_one_per_question_with_known_unique_ids() {
        let questions = vec![question(1, false), question(2, false)];
        assert!(validate_answers(&questions, &[]).unwrap_err().contains("expected 2 answers, got 0"));
        let unknown = vec![answer("q1", &[0], None, false), answer("q9", &[0], None, false)];
        assert!(validate_answers(&questions, &unknown).unwrap_err().contains("unknown question_id `q9`"));
        let duplicate = vec![answer("q1", &[0], None, false), answer("q1", &[1], None, false)];
        assert!(validate_answers(&questions, &duplicate).unwrap_err().contains("duplicate answer for `q1`"));
        let valid = vec![answer("q2", &[1], None, false), answer("q1", &[0], None, false)];
        assert!(validate_answers(&questions, &valid).is_ok(), "order does not matter");
    }

    #[test]
    fn answer_indices_are_distinct_in_range_and_single_select_takes_one() {
        let questions = vec![question(1, false)];
        let out = |selected: &[u32]| validate_answers(&questions, &[answer("q1", selected, None, false)]).unwrap_err();
        assert!(out(&[7]).contains("out of range (0 to 1)"));
        assert!(out(&[0, 0]).contains("answered twice"));
        assert!(out(&[0, 1]).contains("without multiple"));
        let multi = vec![question(1, true)];
        assert!(validate_answers(&multi, &[answer("q1", &[0, 1], None, false)]).is_ok());
        assert!(validate_answers(&multi, &[answer("q1", &[1, 0], None, false)]).is_ok(), "order does not matter");
    }

    #[test]
    fn other_text_is_trimmed_limited_and_forbidden_on_a_skip() {
        let questions = vec![question(1, false)];
        let ok = |other: Option<&str>, skipped: bool| {
            validate_answers(&questions, &[answer("q1", &[], other, skipped)]).is_ok()
        };
        assert!(!ok(Some("   "), false), "an empty non-skipped answer is rejected: select, type, or skip");
        assert!(!ok(None, false), "no selection and no text is rejected");
        assert!(ok(None, true));
        assert!(ok(Some("   "), true), "an empty other counts as absent on a skip too");
        assert!(!ok(Some("typed"), true), "a skip carries nothing");
        assert!(!validate_answers(&questions, &[answer("q1", &[0], None, true)]).is_ok(), "a skip selects nothing");
        let long = "x".repeat(MAX_OTHER + 1);
        let why = validate_answers(&questions, &[answer("q1", &[], Some(&long), false)]).unwrap_err();
        assert!(why.contains("2001 characters") && why.contains("limit is 2000"), "{why}");
    }

    #[test]
    fn a_non_skipped_answer_with_nothing_chosen_is_empty() {
        let questions = vec![question(1, false)];
        let why = validate_answers(&questions, &[answer("q1", &[], None, false)]).unwrap_err();
        assert!(why.contains("answer is empty") && why.contains("or skip"), "{why}");
        let blank = validate_answers(&questions, &[answer("q1", &[], Some("   "), false)]).unwrap_err();
        assert!(blank.contains("answer is empty"), "{blank}");
        // An option or free text settles it; a skip settles it without either.
        assert!(validate_answers(&questions, &[answer("q1", &[0], None, false)]).is_ok());
        assert!(validate_answers(&questions, &[answer("q1", &[], Some("free"), false)]).is_ok());
        assert!(validate_answers(&questions, &[answer("q1", &[], None, true)]).is_ok());
    }

    #[test]
    fn the_result_matches_the_documented_shape() {
        let questions = parse(&args(json!([
            {"question": "Which capa?", "header": "Capa", "multiple": true, "options": [option("A"), option("B")]},
            {"question": "Which size?", "header": "Size", "options": [option("s"), option("m")]},
        ])))
        .unwrap();
        let answers = vec![answer("q1", &[0, 1], Some("typed"), false), answer("q2", &[], None, true)];
        assert!(validate_answers(&questions, &answers).is_ok());
        assert_eq!(
            result(&questions, &answers),
            json!({"answers": [
                {"question_id": "q1", "question": "Which capa?", "header": "Capa", "status": "answered",
                 "selected": ["A", "B"], "other": "typed"},
                {"question_id": "q2", "question": "Which size?", "header": "Size", "status": "skipped"},
            ]})
        );
    }

    #[test]
    fn the_result_leaves_out_empty_selected_and_absent_other() {
        let questions = vec![question(1, false)];
        let bare = result(&questions, &[answer("q1", &[], Some("   "), false)]);
        assert_eq!(
            bare,
            json!({"answers": [{"question_id": "q1", "question": "Which capa?", "header": "Capa", "status": "answered"}]})
        );
        let just_other = result(&questions, &[answer("q1", &[], Some("  free  "), false)]);
        assert_eq!(just_other["answers"][0]["other"], "free", "other text is trimmed: {just_other}");
        assert!(just_other["answers"][0].get("selected").is_none());
    }

    #[test]
    fn the_status_result_marks_every_question() {
        let questions = vec![question(1, false), question(2, true)];
        for status in ["timed_out", "cancelled"] {
            let value = status_result(&questions, status);
            assert_eq!(value["answers"].as_array().unwrap().len(), 2);
            for entry in value["answers"].as_array().unwrap() {
                assert_eq!(entry["status"], status);
                assert!(entry.get("selected").is_none() && entry.get("other").is_none());
            }
        }
    }

    #[test]
    fn a_decided_answer_is_the_result_and_leaves_nothing_pending() {
        let gate = QuestionGate::with_timeout(Duration::from_secs(5));
        let questions = vec![question(1, false)];
        let value = thread::scope(|s| {
            let gate_ref = &gate;
            let handle = s.spawn(move || gate_ref.ask("call-1", questions.clone(), || {
                gate_ref.decide("call-1", vec![answer("q1", &[1], Some("x"), false)]).expect("decided");
            }));
            handle.join().unwrap()
        });
        assert_eq!(value["answers"][0]["status"], "answered");
        assert_eq!(value["answers"][0]["selected"], json!(["b"]));
        assert!(!gate.is_pending("call-1"), "the answered question is gone");
    }

    #[test]
    fn skip_all_is_a_result_and_a_cancel_is_cancelled() {
        let gate = QuestionGate::with_timeout(Duration::from_secs(5));
        let questions = vec![question(1, false), question(2, true)];
        let value = thread::scope(|s| {
            let gate_ref = &gate;
            let qs = questions.clone();
            let handle = s.spawn(move || gate_ref.ask("call-1", qs, || {
                gate_ref
                    .decide(
                        "call-1",
                        vec![answer("q1", &[], None, true), answer("q2", &[], None, true)],
                    )
                    .expect("skipped");
            }));
            handle.join().unwrap()
        });
        assert!(value["answers"].as_array().unwrap().iter().all(|a| a["status"] == "skipped"));

        let gate = QuestionGate::with_timeout(Duration::from_secs(5));
        let value = thread::scope(|s| {
            let gate_ref = &gate;
            let qs = questions.clone();
            let handle = s.spawn(move || gate_ref.ask("call-2", qs, || gate_ref.cancel_all()));
            handle.join().unwrap()
        });
        assert!(value["answers"].as_array().unwrap().iter().all(|a| a["status"] == "cancelled"), "{value}");
        assert!(!gate.is_pending("call-2"));
    }

    #[test]
    fn an_unanswered_ask_times_out_and_a_late_decide_fails() {
        let gate = QuestionGate::with_timeout(Duration::from_millis(30));
        let value = gate.ask("call-1", vec![question(1, false)], || {});
        assert_eq!(value["answers"][0]["status"], "timed_out");
        let err = gate.decide("call-1", vec![answer("q1", &[0], None, false)]).expect_err("too late");
        assert_eq!(err, "no pending question call-1");
        assert!(!gate.is_pending("call-1"));
    }

    #[test]
    fn an_unknown_id_is_an_error() {
        let gate = QuestionGate::with_timeout(Duration::from_secs(5));
        assert_eq!(gate.decide("nope", vec![]).unwrap_err(), "no pending question nope");
    }

    #[test]
    fn an_invalid_decide_leaves_the_question_pending_for_a_retry() {
        let gate = QuestionGate::with_timeout(Duration::from_secs(5));
        let questions = vec![question(1, false)];
        let value = thread::scope(|s| {
            let gate_ref = &gate;
            let (registered_tx, registered_rx) = mpsc::channel();
            let handle =
                s.spawn(move || gate_ref.ask("call-1", questions.clone(), || registered_tx.send(()).unwrap()));
            registered_rx.recv().unwrap();
            assert!(gate_ref.is_pending("call-1"));
            let bad = vec![answer("q9", &[0], None, false)];
            let err = gate.decide("call-1", bad).expect_err("unknown question");
            assert!(err.contains("unknown question_id"), "{err}");
            assert!(gate.is_pending("call-1"), "the invalid answer keeps the question pending");
            gate.decide("call-1", vec![answer("q1", &[0], None, false)]).expect("the retry works");
            handle.join().unwrap()
        });
        assert_eq!(value["answers"][0]["selected"], json!(["a"]));
        assert!(!gate.is_pending("call-1"));
    }

    /// The decide lands just as the wait ends: either side may win, but they agree. A decide that won the lock sends
    /// before the timeout's removal, so its `try_recv` finds the answer; a decide after the removal finds nothing.
    /// Both outcomes are asserted, so the timing only picks the path, never the verdict.
    #[test]
    fn a_decide_racing_the_timeout_agrees_with_the_wait() {
        for _ in 0..50 {
            let gate = QuestionGate::with_timeout(Duration::from_millis(20));
            let questions = vec![question(1, false)];
            thread::scope(|s| {
                let gate_ref = &gate;
                let handle = s.spawn(move || gate_ref.ask("call-1", questions.clone(), || {}));
                thread::sleep(Duration::from_millis(20));
                let decided = gate_ref.decide("call-1", vec![answer("q1", &[0], None, false)]);
                let value = handle.join().unwrap();
                match decided {
                    Ok(()) => assert_eq!(value["answers"][0]["status"], "answered"),
                    Err(err) => {
                        assert_eq!(err, "no pending question call-1");
                        assert_eq!(value["answers"][0]["status"], "timed_out");
                    }
                }
            });
        }
    }

    /// The biggest valid result fits the answer limit with room to spare, so the projection never cuts it mid-JSON.
    #[test]
    fn the_maximum_valid_result_fits_the_answer_limit_and_parses() {
        let option = |i: usize| {
            json!({"label": "l".repeat(MAX_LABEL), "description": "d".repeat(MAX_DESCRIPTION), "recommended": i == 0})
        };
        let items: Vec<Value> = (0..MAX_QUESTIONS)
            .map(|q| {
                json!({
                    "question": "é".repeat(MAX_QUESTION),
                    "header": "h".repeat(MAX_HEADER),
                    "multiple": q % 2 == 0,
                    "options": (0..MAX_OPTIONS).map(option).collect::<Vec<_>>(),
                })
            })
            .collect();
        let questions = parse(&args(json!(items))).unwrap();
        let answers: Vec<AnswerInput> = questions
            .iter()
            .enumerate()
            .map(|(i, q)| answer(
                &q.id,
                &(0..MAX_OPTIONS as u32).collect::<Vec<_>>()[..if i % 2 == 0 { MAX_OPTIONS } else { 1 }],
                Some(&"o".repeat(MAX_OTHER)),
                false,
            ))
            .collect();
        assert!(validate_answers(&questions, &answers).is_ok());
        let text = result(&questions, &answers).to_string();
        assert!(text.len() < ANSWER_LIMIT, "the maximum result is {} bytes, the limit is {ANSWER_LIMIT}", text.len());
        let _parsed: Value = serde_json::from_str(&text).expect("the result is JSON");
    }
}
