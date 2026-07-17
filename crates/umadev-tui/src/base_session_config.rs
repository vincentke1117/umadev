//! Native, typed configuration changes for an idle resident base session.

use std::sync::Arc;

use umadev_agent::{ChannelSink, EngineEvent, EventSink};
use umadev_runtime::{SessionCapability, SessionError};

use super::ChatSessionHolder;

pub(super) fn spawn_thinking_change(
    holder: ChatSessionHolder,
    sink: Arc<ChannelSink>,
    lang: umadev_i18n::Lang,
    backend_id: String,
    enabled: bool,
) {
    let generation = holder.generation();
    tokio::spawn(async move {
        let mut guard = holder.lock().await;
        if holder.generation() != generation {
            sink.emit(EngineEvent::Note(
                umadev_i18n::t(lang, "thinking.session_changed").to_string(),
            ));
            return;
        }
        let Some(resident) = guard.as_mut() else {
            sink.emit(EngineEvent::Note(
                umadev_i18n::t(lang, "thinking.not_active").to_string(),
            ));
            return;
        };
        let result = resident.session_mut().set_thinking(enabled).await;
        if holder.generation() != generation {
            sink.emit(EngineEvent::Note(
                umadev_i18n::t(lang, "thinking.session_changed").to_string(),
            ));
            return;
        }
        match result {
            Ok(update) => {
                sink.emit(EngineEvent::BaseSessionState { backend_id, update });
                sink.emit(EngineEvent::Note(umadev_i18n::tf(
                    lang,
                    "thinking.changed",
                    &[if enabled { "on" } else { "off" }],
                )));
            }
            Err(SessionError::CapabilityUnsupported(SessionCapability::SetThinking)) => {
                sink.emit(EngineEvent::Note(
                    umadev_i18n::t(lang, "thinking.unsupported").to_string(),
                ));
            }
            Err(error) => sink.emit(EngineEvent::Note(umadev_i18n::tf(
                lang,
                "thinking.failed",
                &[&safe_config_error(&error.to_string())],
            ))),
        }
    });
}

fn safe_config_error(value: &str) -> String {
    umadev_agent::base_error::strip_ansi(value)
        .chars()
        .filter(|character| !character.is_control())
        .take(512)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ResidentChat;
    use umadev_runtime::{
        ApprovalDecision, BaseSession, SessionCapabilities, SessionEvent, SessionStateUpdate,
    };

    struct ThinkingSession;

    #[async_trait::async_trait]
    impl BaseSession for ThinkingSession {
        fn capabilities(&self) -> SessionCapabilities {
            SessionCapabilities {
                set_thinking: true,
                ..SessionCapabilities::default()
            }
        }

        async fn set_thinking(
            &mut self,
            enabled: bool,
        ) -> Result<SessionStateUpdate, SessionError> {
            Ok(SessionStateUpdate::ThinkingChanged {
                enabled: Some(enabled),
                can_enable: true,
                can_disable: true,
            })
        }

        async fn send_turn(&mut self, _directive: String) -> Result<(), SessionError> {
            Ok(())
        }

        async fn next_event(&mut self) -> Option<SessionEvent> {
            None
        }

        async fn respond(
            &mut self,
            _req_id: &str,
            _decision: ApprovalDecision,
        ) -> Result<(), SessionError> {
            Ok(())
        }

        async fn interrupt(&mut self) -> Result<(), SessionError> {
            Ok(())
        }

        async fn end(&mut self) -> Result<(), SessionError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn thinking_change_uses_the_resident_control_plane_and_emits_confirmed_state() {
        let holder = ChatSessionHolder::new(Some(ResidentChat::Primed(Box::new(ThinkingSession))));
        let (sink, mut events) = ChannelSink::new();
        spawn_thinking_change(
            holder.clone(),
            Arc::new(sink),
            umadev_i18n::Lang::En,
            "kimi-code".to_string(),
            false,
        );

        let state = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            state,
            EngineEvent::BaseSessionState {
                backend_id,
                update: SessionStateUpdate::ThinkingChanged {
                    enabled: Some(false),
                    can_enable: true,
                    can_disable: true,
                }
            } if backend_id == "kimi-code"
        ));
        let note = events.recv().await.unwrap();
        assert!(matches!(note, EngineEvent::Note(message) if message.contains("confirmed")));
        assert!(
            holder.lock().await.is_some(),
            "configuration must not consume the resident"
        );
    }
}
