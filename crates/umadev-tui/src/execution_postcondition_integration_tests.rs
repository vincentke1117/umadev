#[tokio::test]
async fn resident_contract_blocks_actual_out_of_scope_bash_write() {
    let tmp = tempfile::TempDir::new().unwrap();
    let target = tmp.path().join("src/outside.rs");
    let (sink, mut engine_rx) = ChannelSink::new();
    let sink = Arc::new(sink);
    let (route_tx, mut route_rx) = tokio::sync::mpsc::unbounded_channel();

    let (intent_fork, _fork_sent, _fork_ended) = FakeChatSession::new(vec![vec![
        umadev_runtime::SessionEvent::TextDelta(
            serde_json::json!({
                "class": "quick_edit",
                "authorization": "mutating",
                "kind": "light",
                "complexity": "simple",
                "scope": ["src/allowed.rs"],
                "confidence": 0.99
            })
            .to_string(),
        ),
        umadev_runtime::SessionEvent::TurnDone {
            status: umadev_runtime::TurnStatus::Completed,
            usage: None,
        },
    ]]);
    let (writer, _sent, _ended) = FakeChatSession::new(vec![vec![
        umadev_runtime::SessionEvent::ToolCall {
            name: "Bash".into(),
            input: serde_json::json!({"command": "write src/outside.rs"}),
        },
        umadev_runtime::SessionEvent::ToolResult {
            ok: true,
            summary: "write complete".into(),
        },
        umadev_runtime::SessionEvent::ToolCall {
            name: "Bash".into(),
            input: serde_json::json!({"command": "cargo check"}),
        },
        umadev_runtime::SessionEvent::ToolResult {
            ok: true,
            summary: "cargo check passed".into(),
        },
        umadev_runtime::SessionEvent::TextDelta("done".into()),
        umadev_runtime::SessionEvent::TurnDone {
            status: umadev_runtime::TurnStatus::Completed,
            usage: None,
        },
    ]]);
    let writer = writer
        .with_fork(Box::new(intent_fork))
        .with_send_effect(move || {
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(target, "pub fn outside() {}\n").unwrap();
        });
    let holder = ChatSessionHolder::from_mutex(tokio::sync::Mutex::new(Some(
        ResidentChat::Primed(Box::new(writer)),
    )));

    drive_chat_session_turn(chat_turn(
        "只修改 src/allowed.rs",
        holder,
        sink,
        route_tx,
        tmp.path().to_path_buf(),
    ))
    .await;

    match route_rx.try_recv() {
        Ok(RouteDecision::Failed(note)) => {
            assert!(note.contains("execution-path-out-of-scope"));
            assert!(note.contains("src/outside.rs"));
        }
        other => panic!("expected contract failure, got {other:?}"),
    }
    assert!(
        route_rx.try_recv().is_err(),
        "failure is the only terminal decision"
    );
    while let Ok(event) = engine_rx.try_recv() {
        if let EngineEvent::Note(note) = event {
            assert!(
                !note.contains("本轮实际文件变更"),
                "a blocked turn must not emit a success-looking fact line: {note}"
            );
        }
    }
}

#[tokio::test]
async fn resident_contract_guards_question_pause_and_interrupted_success_exits() {
    let early_exits = [
        (
            "question pause",
            vec![umadev_runtime::SessionEvent::ToolCall {
                name: "AskUserQuestion".into(),
                input: serde_json::json!({"questions": [{
                    "header": "Choice",
                    "question": "Continue?",
                    "options": [{"label": "Yes"}]
                }]}),
            }],
        ),
        (
            "interrupted",
            vec![umadev_runtime::SessionEvent::TurnDone {
                status: umadev_runtime::TurnStatus::Interrupted,
                usage: None,
            }],
        ),
    ];

    for (label, events) in early_exits {
        let tmp = tempfile::TempDir::new().unwrap();
        let target = tmp.path().join("src/outside.rs");
        let (sink, _engine_rx) = ChannelSink::new();
        let (route_tx, mut route_rx) = tokio::sync::mpsc::unbounded_channel();
        let (intent_fork, _, _) = FakeChatSession::new(vec![vec![
            umadev_runtime::SessionEvent::TextDelta(
                serde_json::json!({
                    "class": "quick_edit",
                    "authorization": "mutating",
                    "kind": "light",
                    "complexity": "simple",
                    "scope": ["src/allowed.rs"],
                    "confidence": 0.99
                })
                .to_string(),
            ),
            umadev_runtime::SessionEvent::TurnDone {
                status: umadev_runtime::TurnStatus::Completed,
                usage: None,
            },
        ]]);
        let (writer, _, _) = FakeChatSession::new(vec![events]);
        let writer = writer
            .with_fork(Box::new(intent_fork))
            .with_send_effect(move || {
                std::fs::create_dir_all(target.parent().unwrap()).unwrap();
                std::fs::write(target, "outside\n").unwrap();
            });
        let holder = ChatSessionHolder::from_mutex(tokio::sync::Mutex::new(Some(
            ResidentChat::Primed(Box::new(writer)),
        )));

        drive_chat_session_turn(chat_turn(
            "只修改 src/allowed.rs",
            holder,
            Arc::new(sink),
            route_tx,
            tmp.path().to_path_buf(),
        ))
        .await;

        assert!(
            matches!(route_rx.try_recv(), Ok(RouteDecision::Failed(note))
                if note.contains("execution-path-out-of-scope")),
            "{label} cannot bypass the execution post-condition"
        );
        assert!(
            route_rx.try_recv().is_err(),
            "{label} emitted a second terminal decision"
        );
    }
}
