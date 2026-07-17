use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use umadev_knowledge::{
    evaluate_abstentions, evaluate_rankings, retrieve, AbstentionJudgment, RetrievalConfig,
    RetrievalEngine, RetrievalJudgment,
};
use umadev_spec::Phase;

struct HomeGuard {
    _lock: MutexGuard<'static, ()>,
    home: Option<String>,
    userprofile: Option<String>,
}

impl HomeGuard {
    fn isolate(path: &std::path::Path) -> Self {
        static ENV_LOCK: Mutex<()> = Mutex::new(());
        let lock = ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
        let guard = Self {
            _lock: lock,
            home: std::env::var("HOME").ok(),
            userprofile: std::env::var("USERPROFILE").ok(),
        };
        std::env::set_var("HOME", path);
        std::env::set_var("USERPROFILE", path);
        guard
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match &self.home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
        match &self.userprofile {
            Some(value) => std::env::set_var("USERPROFILE", value),
            None => std::env::remove_var("USERPROFILE"),
        }
    }
}

#[test]
fn curated_corpus_meets_multilingual_bm25_release_floor() {
    let temp = tempfile::tempdir().unwrap();
    let isolated_home = tempfile::tempdir().unwrap();
    let _home = HomeGuard::isolate(isolated_home.path());
    let knowledge_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("knowledge");
    assert!(
        knowledge_dir.is_dir(),
        "curated knowledge corpus is missing"
    );
    let config = RetrievalConfig {
        enabled: true,
        engine: RetrievalEngine::Bm25,
        top_k: 10,
        custom_dirs: Vec::new(),
    };
    let fixtures = [
        (
            "auth-session-zh",
            "登录鉴权 JWT refresh token 轮换 session 会话安全",
            vec!["backend/01-standards/auth-implementation.md"],
        ),
        (
            "seo-web-vitals-zh",
            "网站 SEO 搜索引擎优化 structured data Core Web Vitals",
            vec!["frontend/01-standards/seo-and-web-vitals.md"],
        ),
        (
            "database-migration",
            "PostgreSQL schema database migration rollback zero downtime 索引回滚",
            vec!["development/02-playbooks/database-migration-playbook.md"],
        ),
        (
            "cjk-export",
            "CSV Excel PDF 导出中文乱码 CJK encoding BOM 字体",
            vec!["backend/01-standards/cjk-in-exports-and-documents.md"],
        ),
        (
            "agent-memory",
            "AI Agent 长期记忆 episodic semantic procedural memory context compression",
            vec!["agentic-delivery/01-standards/self-improving-memory-and-regression-sets.md"],
        ),
        (
            "release-rollback",
            "progressive delivery canary release rollback recovery feature flag",
            vec!["release-engineering/03-checklists/release-rollback-readiness-checklist.md"],
        ),
        (
            "idempotent-payment-retry-zh",
            "支付请求超时后安全重试，避免重复扣款，需要幂等键和响应重放",
            vec!["backend/01-standards/idempotency-and-safe-retries.md"],
        ),
        (
            "kubernetes-crashloop",
            "Kubernetes Pod CrashLoopBackOff OOM readiness probe 排查容器反复重启",
            vec!["cloud-native/02-playbooks/k8s-troubleshooting-playbook.md"],
        ),
        (
            "accessible-keyboard-focus",
            "WCAG keyboard only navigation visible focus screen reader accessibility 验收",
            vec![
                "frontend/01-standards/accessibility-acceptance-gate.md",
                "frontend/01-standards/accessibility-standard.md",
            ],
        ),
        (
            "rag-prompt-injection",
            "RAG 检索内容里的 prompt injection 会诱导 Agent 调用危险工具，如何隔离不可信上下文",
            vec![
                "ai/prompt-and-tool-guardrails.md",
                "ai/ai-data-security-and-compliance-playbook.md",
            ],
        ),
        (
            "websocket-reliability",
            "WebSocket heartbeat reconnect exponential backoff backpressure message ordering",
            vec!["backend/01-standards/realtime-and-websocket.md"],
        ),
        (
            "distributed-transaction",
            "跨服务事务不用 2PC，比较 saga transactional outbox 补偿和幂等消费者",
            vec![
                "architecture/distributed-transactions.md",
                "backend/01-standards/microservices-and-distributed.md",
            ],
        ),
        (
            "software-supply-chain",
            "dependency confusion SBOM provenance artifact signing SLSA 软件供应链防护",
            vec!["security/01-standards/supply-chain-security.md"],
        ),
        (
            "rust-borrowing",
            "Rust borrow checker ownership lifetime mutable reference Send Sync 并发安全",
            vec!["development/01-standards/rust-complete.md"],
        ),
        (
            "e2e-flakiness",
            "端到端测试偶发失败 flaky test selector wait retry Playwright 稳定性治理",
            vec!["testing/02-playbooks/e2e-testing-playbook.md"],
        ),
        (
            "slo-burn-rate",
            "SLO error budget burn rate multi-window alerting tracing metrics logs 可观测性",
            vec!["observability/01-standards/observability-and-slo-operations.md"],
        ),
        (
            "localization-rtl-plurals",
            "本地化 ICU plural RTL locale fallback date number formatting 国际化",
            vec!["frontend/01-standards/i18n-and-localization.md"],
        ),
        (
            "gdpr-erasure",
            "GDPR data subject access request right to erasure retention consent 数据删除",
            vec!["security/01-standards/data-protection-gdpr.md"],
        ),
    ];
    let judgments = fixtures
        .into_iter()
        .map(|(id, query, relevant)| {
            let ranked = retrieve(temp.path(), &knowledge_dir, &config, query, Phase::Research)
                .into_iter()
                .map(|hit| hit.chunk.meta.path)
                .collect();
            RetrievalJudgment {
                id: id.into(),
                relevant: relevant.into_iter().map(str::to_string).collect(),
                ranked,
            }
        })
        .collect::<Vec<_>>();
    let report = evaluate_rankings(&judgments, 5);
    assert!(
        report.recall_at_k >= 0.75,
        "Recall@5 regressed: {report:?}\n{judgments:#?}"
    );
    assert!(
        report.mrr >= 0.60,
        "MRR regressed: {report:?}\n{judgments:#?}"
    );
    assert!(
        report.ndcg_at_k >= 0.65,
        "nDCG@5 regressed: {report:?}\n{judgments:#?}"
    );
    assert!(
        report.misses.is_empty(),
        "retrieval misses: {report:?}\n{judgments:#?}"
    );
}

#[test]
fn bilingual_and_abstention_release_contract_is_reproducible_offline() {
    let temp = tempfile::tempdir().unwrap();
    let isolated_home = tempfile::tempdir().unwrap();
    let _home = HomeGuard::isolate(isolated_home.path());
    let knowledge_dir = temp.path().join("knowledge");
    std::fs::create_dir_all(knowledge_dir.join("security")).unwrap();
    std::fs::create_dir_all(knowledge_dir.join("backend")).unwrap();
    std::fs::write(
        knowledge_dir.join("security/credential-rotation.md"),
        "# Credential lifecycle\n\n## Rotation\n\nRotate authentication credentials before expiry.",
    )
    .unwrap();
    std::fs::write(
        knowledge_dir.join("backend/idempotency.md"),
        "# 支付可靠性\n\n## 请求处理\n\n支付接口必须使用幂等键，超时后允许安全重试。",
    )
    .unwrap();
    let config = RetrievalConfig {
        enabled: true,
        engine: RetrievalEngine::Bm25,
        top_k: 5,
        custom_dirs: Vec::new(),
    };
    let positive = [
        (
            "zh-to-en-auth",
            "登录凭证轮换",
            "security/credential-rotation.md",
        ),
        (
            "en-to-zh-retry",
            "idempotent retry",
            "backend/idempotency.md",
        ),
    ]
    .into_iter()
    .map(|(id, query, relevant)| RetrievalJudgment {
        id: id.into(),
        relevant: vec![relevant.into()],
        ranked: retrieve(temp.path(), &knowledge_dir, &config, query, Phase::Research)
            .into_iter()
            .map(|hit| hit.chunk.meta.path)
            .collect(),
    })
    .collect::<Vec<_>>();
    let ranking = evaluate_rankings(&positive, 3);
    assert_eq!(ranking.recall_at_k, 1.0, "{ranking:?}\n{positive:#?}");
    assert!(ranking.misses.is_empty(), "{ranking:?}\n{positive:#?}");

    let negative = ["quasar pottery musical notation", "火星陶器十二音作曲"]
        .into_iter()
        .enumerate()
        .map(|(index, query)| AbstentionJudgment {
            id: format!("negative-{index}"),
            ranked: retrieve(temp.path(), &knowledge_dir, &config, query, Phase::Research)
                .into_iter()
                .map(|hit| hit.chunk.meta.path)
                .collect(),
        })
        .collect::<Vec<_>>();
    let abstention = evaluate_abstentions(&negative);
    assert_eq!(abstention.accuracy, 1.0, "{abstention:?}\n{negative:#?}");
    assert!(abstention.false_positives.is_empty());
}

#[test]
fn project_local_learned_corpus_never_crosses_project_boundary() {
    let temp = tempfile::tempdir().unwrap();
    let isolated_home = tempfile::tempdir().unwrap();
    let _home = HomeGuard::isolate(isolated_home.path());
    let project_a = temp.path().join("project-a");
    let project_b = temp.path().join("project-b");
    let knowledge_dir = temp.path().join("knowledge");
    std::fs::create_dir_all(project_a.join(".umadev/learned/private")).unwrap();
    std::fs::create_dir_all(&project_b).unwrap();
    std::fs::create_dir_all(&knowledge_dir).unwrap();
    std::fs::write(
        project_a.join(".umadev/learned/private/tenant-alpha.md"),
        "# Private lesson\n\n## Incident\n\ntenantalphaprivate recovery sequence",
    )
    .unwrap();
    let config = RetrievalConfig {
        enabled: true,
        engine: RetrievalEngine::Bm25,
        top_k: 5,
        custom_dirs: Vec::new(),
    };

    let a = retrieve(
        &project_a,
        &knowledge_dir,
        &config,
        "tenantalphaprivate",
        Phase::Research,
    );
    assert!(!a.is_empty(), "project A must recall its own local lesson");
    let b = retrieve(
        &project_b,
        &knowledge_dir,
        &config,
        "tenantalphaprivate",
        Phase::Research,
    );
    assert!(
        b.is_empty(),
        "project B must not index project A's local memory: {b:?}"
    );
}
