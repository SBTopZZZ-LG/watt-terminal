//! Trimming the wire copy of a request to fit the context limit (slice 2 of the context budget). The stored history is
//! never changed: `History::messages_with` builds the trimmed messages from a `TrimPlan`, and `LlmAgent` picks the plan
//! with `select`. The rungs leave out more and more of the older content, in this order: old tool outputs, then old
//! documents, then whole old turns. The first plan whose calibrated estimate fits the budget is sent.

use crate::core::chat::context;

/// Said in the system text of a trimmed request. It is part of the estimate, and never stored.
pub const TRIM_NOTE: &str = "Note: some older messages or tool outputs were omitted to fit the context window.";
/// What stands in for an old tool output on the wire. It is generic: the tool may not be `get_command`.
pub const OLD_OUTPUT_NOTE: &str = "[older tool output omitted to fit the context window]";
/// Put at the start of the first kept user message when older turns were dropped.
pub const OMITTED_TURNS_NOTE: &str = "[earlier conversation omitted to fit the context window]";

/// What a trimmed request leaves out. The default (`none`) is the untrimmed request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TrimPlan {
    /// Keep this many of the newest tool results; older ones are sent as `OLD_OUTPUT_NOTE`. None keeps all of them.
    pub keep_tool_results: Option<usize>,
    /// Replace the text of each document in a user message by a note, except in the last user message.
    pub elide_old_docs: bool,
    /// Leave out this many of the oldest turns. A turn is a user message and what follows it up to the next one.
    pub drop_turns: usize,
}

impl TrimPlan {
    pub fn none() -> Self {
        Self::default()
    }

    /// True for the untrimmed request, which is sent exactly as the history stores it.
    pub fn is_none(&self) -> bool {
        *self == Self::none()
    }
}

/// The rungs before any turn is dropped. Rung 0 (the untrimmed request) is the first entry.
const RUNGS: [TrimPlan; 5] = [
    TrimPlan { keep_tool_results: None, elide_old_docs: false, drop_turns: 0 },
    TrimPlan { keep_tool_results: Some(8), elide_old_docs: false, drop_turns: 0 },
    TrimPlan { keep_tool_results: Some(4), elide_old_docs: false, drop_turns: 0 },
    TrimPlan { keep_tool_results: Some(2), elide_old_docs: true, drop_turns: 0 },
    TrimPlan { keep_tool_results: Some(1), elide_old_docs: true, drop_turns: 0 },
];

/// The tokens a request may take: 90% of the context limit (rounded down), less the reserve for the reply. Negative when
/// the reserve alone is more than that, in which case nothing fits.
pub fn budget(limit: u32, reserve: u32) -> i64 {
    i64::from(limit) * 9 / 10 - i64::from(reserve)
}

/// The context limit for the one retry after an overflow that named no limit: the smallest one whose budget (see `budget`)
/// is 70% of the failed request's estimate, rounded down, plus the reserve. The result is kept within the limits that are
/// taken from a provider's error (see `context::LIMIT_MIN` and `context::LIMIT_MAX`).
pub fn overflow_limit(estimate: u32, reserve: u32) -> u32 {
    let wanted_budget = i64::from(estimate) * 7 / 10;
    // budget(L) >= n holds exactly when 9L >= 10n, so the smallest such L is ceil(10n / 9).
    let needed = (wanted_budget + i64::from(reserve)) * 10;
    let limit = (needed + 8) / 9;
    u32::try_from(limit.clamp(i64::from(context::LIMIT_MIN), i64::from(context::LIMIT_MAX))).unwrap_or(context::LIMIT_MAX)
}

/// The system text of a request under `plan`: the text itself, with the trim note added when anything is left out.
pub fn system_for(system: &str, plan: &TrimPlan) -> String {
    if plan.is_none() { system.to_string() } else { format!("{system}\n\n{TRIM_NOTE}") }
}

/// The first plan whose estimate fits `budget`, tried rung by rung and then by dropping turns. `max_drop` is the most
/// turns that may be dropped (the turns less the last one). `estimate` gives the calibrated estimate of the request under
/// a plan. The estimate must fall as more is left out; the smallest turn count that fits is then found by bisection.
/// Err carries the estimate of the smallest request when none fits (including when the budget is not positive).
pub fn select(budget: i64, max_drop: usize, mut estimate: impl FnMut(&TrimPlan) -> u32) -> Result<TrimPlan, u32> {
    let fits = |tokens: u32| budget > 0 && i64::from(tokens) <= budget;
    for plan in RUNGS {
        if fits(estimate(&plan)) {
            return Ok(plan);
        }
    }
    let dropping = |drop_turns: usize| TrimPlan { drop_turns, ..RUNGS[RUNGS.len() - 1] };
    let smallest = estimate(&dropping(max_drop));
    if max_drop == 0 || !fits(smallest) {
        return Err(smallest);
    }
    // Invariant: `hi` fits. Each step keeps it so, and the loop ends at the smallest count that fits.
    let (mut lo, mut hi) = (1, max_drop);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if fits(estimate(&dropping(mid))) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    Ok(dropping(lo))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// A made-up estimate by rung: 100 untrimmed, then 90, 80, 70, 60, and 10 less for each dropped turn.
    fn rung_estimate(plan: &TrimPlan) -> u32 {
        let base: u32 = match plan.keep_tool_results {
            None => 100,
            Some(8) => 90,
            Some(4) => 80,
            Some(2) => 70,
            Some(_) => 60,
        };
        base.saturating_sub(10 * plan.drop_turns as u32)
    }

    #[test]
    fn the_budget_is_ninety_percent_of_the_limit_less_the_reserve() {
        assert_eq!(budget(1000, 512), 388);
        assert_eq!(budget(1112, 990), 10, "floor(1000.8) less 990");
        assert_eq!(budget(1, 0), 0);
        assert_eq!(budget(10, 20), -11, "a reserve above the limit leaves nothing");
    }

    #[test]
    fn the_untrimmed_request_is_the_none_plan_and_adds_no_note() {
        assert!(TrimPlan::none().is_none());
        assert_eq!(system_for("sys", &TrimPlan::none()), "sys");
        let trimmed = TrimPlan { drop_turns: 1, ..TrimPlan::none() };
        assert!(!trimmed.is_none());
        assert_eq!(system_for("sys", &trimmed), format!("sys\n\n{TRIM_NOTE}"));
    }

    #[test]
    fn a_request_that_fits_is_sent_untrimmed() {
        assert_eq!(select(100, 5, rung_estimate), Ok(TrimPlan::none()));
    }

    #[test]
    fn the_rungs_are_tried_in_order_and_the_first_that_fits_wins() {
        let plan = |keep, elide| TrimPlan { keep_tool_results: keep, elide_old_docs: elide, drop_turns: 0 };
        assert_eq!(select(95, 5, rung_estimate), Ok(plan(Some(8), false)));
        assert_eq!(select(85, 5, rung_estimate), Ok(plan(Some(4), false)));
        assert_eq!(select(75, 5, rung_estimate), Ok(plan(Some(2), true)));
        assert_eq!(select(65, 5, rung_estimate), Ok(plan(Some(1), true)));
    }

    #[test]
    fn turns_are_dropped_only_when_the_last_rung_is_not_enough() {
        // 60 at the last rung; one dropped turn takes it to 50.
        let drop_one = TrimPlan { keep_tool_results: Some(1), elide_old_docs: true, drop_turns: 1 };
        assert_eq!(select(55, 5, rung_estimate), Ok(drop_one));
        // 40 with two dropped, 30 with three.
        let drop_three = TrimPlan { drop_turns: 3, ..drop_one };
        assert_eq!(select(35, 5, rung_estimate), Ok(drop_three));
    }

    #[test]
    fn when_nothing_fits_the_smallest_estimate_is_returned() {
        assert_eq!(select(5, 2, rung_estimate), Err(40), "the estimate with two turns dropped");
        assert_eq!(select(55, 0, rung_estimate), Err(60), "no turn can be dropped: the last rung is the smallest");
    }

    #[test]
    fn a_budget_that_is_not_positive_fits_nothing() {
        assert_eq!(select(0, 3, |_| 0), Err(0));
        assert_eq!(select(-5, 0, |_| 0), Err(0));
    }

    #[test]
    fn the_smallest_turn_count_that_fits_is_found_by_bisection() {
        // 2000 untrimmed, then 1000 - 100 per dropped turn: 450 fits from six dropped turns (400), not five (500).
        let estimate = |plan: &TrimPlan| if plan.drop_turns == 0 { 2000 } else { 1000 - 100 * plan.drop_turns as u32 };
        let drop_six = TrimPlan { keep_tool_results: Some(1), elide_old_docs: true, drop_turns: 6 };
        assert_eq!(select(450, 9, estimate), Ok(drop_six));
    }

    #[test]
    fn bisection_takes_few_estimates_for_a_long_history() {
        let calls = Cell::new(0);
        let estimate = |plan: &TrimPlan| {
            calls.set(calls.get() + 1);
            if plan.drop_turns == 0 { 5000 } else { 1000 - plan.drop_turns as u32 }
        };
        // Five rungs, the smallest request, then bisection over 1..=1000: about ten more.
        assert_eq!(select(500, 1000, estimate).map(|p| p.drop_turns), Ok(500));
        assert!(calls.get() <= 5 + 1 + 10 + 1, "{} estimates", calls.get());
    }

    #[test]
    fn the_overflow_limit_is_the_smallest_whose_budget_is_seventy_percent_of_the_estimate() {
        // 70% of 10000 is 7000; the budget of 8347 is 7512 - 512 = 7000, and 8346 gives 6999.
        assert_eq!(overflow_limit(10_000, 512), 8347);
        assert!(budget(8347, 512) >= 7000 && budget(8346, 512) < 7000);
        // Zero reserve, a small estimate: below the minimum, so the minimum.
        assert_eq!(overflow_limit(100, 0), context::LIMIT_MIN);
        // A huge estimate: above the maximum, so the maximum.
        assert_eq!(overflow_limit(u32::MAX, 0), context::LIMIT_MAX);
    }
}
