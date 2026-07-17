use umadev_runtime::Usage;

/// Quality-preserving live usage state for one TUI conversation.
#[derive(Debug, Clone, Default)]
pub(crate) struct SessionUsageMeter {
    tokens: u64,
    seen: bool,
    incomplete: bool,
    cost_usd_ticks: Option<i64>,
    exact_context_input: Option<u64>,
}

impl SessionUsageMeter {
    pub(crate) fn apply(&mut self, usage: Option<Usage>) {
        let had_prior_report = self.seen;
        self.seen = true;
        let Some(usage) = usage else {
            self.incomplete = true;
            self.cost_usd_ticks = None;
            self.exact_context_input = None;
            return;
        };

        self.tokens = self.tokens.saturating_add(usage.total_tokens);
        self.incomplete |= usage.usage_incomplete;
        let current_cost = usage.trusted_cost_usd_ticks();
        self.cost_usd_ticks = if had_prior_report {
            self.cost_usd_ticks
                .zip(current_cost)
                .and_then(|(left, right)| left.checked_add(right))
        } else {
            current_cost
        };
        self.exact_context_input = (!usage.usage_incomplete).then_some(usage.input_tokens);
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(crate) const fn tokens(&self) -> u64 {
        self.tokens
    }

    pub(crate) const fn has_report(&self) -> bool {
        self.seen
    }

    pub(crate) const fn is_incomplete(&self) -> bool {
        self.incomplete
    }

    pub(crate) const fn exact_cost_usd_ticks(&self) -> Option<i64> {
        self.cost_usd_ticks
    }

    pub(crate) const fn exact_context_input(&self) -> Option<u64> {
        self.exact_context_input
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_and_missing_reports_never_become_exact_or_free() {
        let mut meter = SessionUsageMeter::default();
        let incomplete = Usage {
            usage_incomplete: true,
            cost_usd_ticks: Some(99),
            ..Usage::exact(12, 3)
        };
        meter.apply(Some(incomplete));
        assert_eq!(meter.tokens(), 15);
        assert!(meter.is_incomplete());
        assert_eq!(meter.exact_cost_usd_ticks(), None);
        assert_eq!(meter.exact_context_input(), None);

        meter.apply(None);
        assert_eq!(meter.tokens(), 15);
        assert!(meter.is_incomplete());
        assert_eq!(meter.exact_cost_usd_ticks(), None);

        meter.reset();
        meter.apply(Some(Usage::default()));
        assert!(meter.has_report());
        assert_eq!(meter.tokens(), 0);
        assert!(meter.is_incomplete());
        assert_eq!(meter.exact_cost_usd_ticks(), None);
    }

    #[test]
    fn exact_cost_accumulates_only_while_every_turn_has_one() {
        let mut meter = SessionUsageMeter::default();
        meter.apply(Some(Usage {
            cost_usd_ticks: Some(10),
            ..Usage::exact(3, 2)
        }));
        meter.apply(Some(Usage {
            cost_usd_ticks: Some(20),
            ..Usage::exact(4, 1)
        }));
        assert_eq!(meter.tokens(), 10);
        assert_eq!(meter.exact_cost_usd_ticks(), Some(30));
        meter.apply(Some(Usage::exact(1, 1)));
        assert_eq!(meter.exact_cost_usd_ticks(), None);
    }
}
