use super::{plan_step_glyph, App, ChatRole};

fn queued_text_preview(text: &str) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = compact.chars();
    let preview = chars.by_ref().take(120).collect::<String>();
    if chars.next().is_some() {
        format!("{preview}…")
    } else {
        preview
    }
}

impl App {
    pub(super) fn show_plan_status(&mut self) {
        self.align_queued_dispatch_kinds();
        let has_plan = !self.plan_steps.is_empty();
        let has_review = !self.critic_verdicts.is_empty();
        let has_queue = self.queued_count() > 0;
        if !has_plan && !has_review && !has_queue {
            self.push(ChatRole::System, umadev_i18n::t(self.lang, "plan.none"));
            self.push(
                ChatRole::System,
                umadev_i18n::t(self.lang, "plan.steer.usage"),
            );
            return;
        }

        let mut body = String::new();
        if has_plan {
            let done = self
                .plan_steps
                .iter()
                .filter(|step| step.status == "done")
                .count();
            body.push_str(&format!(
                "{} {done}/{}\n",
                umadev_i18n::t(self.lang, "plan.panel.title"),
                self.plan_steps.len()
            ));
            for step in &self.plan_steps {
                body.push_str(&format!(
                    "  {} {} · {}\n",
                    plan_step_glyph(&step.status),
                    step.id,
                    step.title
                ));
            }
        }

        if has_review {
            let accepts = self
                .critic_verdicts
                .iter()
                .filter(|critic| critic.accepts)
                .count();
            let blocking = self.critic_verdicts.len() - accepts;
            body.push_str(&umadev_i18n::tf(
                self.lang,
                "plan.review.section",
                &[&accepts.to_string(), &blocking.to_string()],
            ));
            body.push('\n');
            for critic in &self.critic_verdicts {
                let verdict = if critic.accepts {
                    umadev_i18n::t(self.lang, "plan.review.accept").to_string()
                } else {
                    umadev_i18n::tf(
                        self.lang,
                        "plan.review.block",
                        &[&critic.blocking.len().max(1).to_string()],
                    )
                };
                body.push_str(&format!("  [{}] {verdict}\n", critic.seat));
                if !critic.accepts {
                    for finding in &critic.blocking {
                        if !finding.trim().is_empty() {
                            body.push_str(&format!("    - {}\n", finding.trim()));
                        }
                    }
                }
            }
        }

        if has_queue {
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(&umadev_i18n::tf(
                self.lang,
                "plan.queue.section",
                &[&self.queued_count().to_string()],
            ));
            body.push('\n');
            let mut position = 1usize;
            for text in &self.queued_chat {
                body.push_str(&umadev_i18n::tf(
                    self.lang,
                    "plan.queue.next_turn",
                    &[&position.to_string(), &queued_text_preview(text)],
                ));
                body.push('\n');
                position += 1;
            }
            for text in &self.queued_steer {
                body.push_str(&umadev_i18n::tf(
                    self.lang,
                    "plan.queue.current_task",
                    &[&position.to_string(), &queued_text_preview(text)],
                ));
                body.push('\n');
                position += 1;
            }
        }

        body.push_str(umadev_i18n::t(self.lang, "plan.steer.usage"));
        self.push(ChatRole::UmaDev, body);
    }
}

#[cfg(test)]
mod tests {
    use super::queued_text_preview;

    #[test]
    fn queued_preview_is_single_line_and_bounded() {
        let text = format!("  one\n\ttwo {}", "界".repeat(130));
        let preview = queued_text_preview(&text);
        assert!(!preview.contains('\n'));
        assert!(preview.ends_with('…'));
        assert_eq!(preview.chars().count(), 121);
    }
}
