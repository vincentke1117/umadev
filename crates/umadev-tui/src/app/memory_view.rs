use std::path::PathBuf;
use umadev_agent::memory_control::MemoryScope;

#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) enum MemoryTuiCommand {
    Inventory {
        scope: MemoryViewScope,
    },
    Capture {
        scope: umadev_agent::memory_control::MemoryScope,
        selector: Option<umadev_agent::memory_control::MemorySelector>,
        enabled: bool,
    },
    Recall {
        scope: umadev_agent::memory_control::MemoryScope,
        selector: Option<umadev_agent::memory_control::MemorySelector>,
        enabled: bool,
    },
    RetentionView {
        scope: MemoryViewScope,
        store: Option<umadev_agent::memory_control::MemoryStore>,
    },
    RetentionSet {
        scope: umadev_agent::memory_control::MemoryScope,
        store: umadev_agent::memory_control::MemoryStore,
        days: u32,
    },
    RetentionClear {
        scope: umadev_agent::memory_control::MemoryScope,
        store: umadev_agent::memory_control::MemoryStore,
    },
    RetentionRun {
        scope: umadev_agent::memory_control::MemoryScope,
        store: umadev_agent::memory_control::MemoryStore,
        confirmed: bool,
    },
    Export {
        scope: umadev_agent::memory_control::MemoryScope,
        selector: umadev_agent::memory_control::MemorySelector,
        destination: PathBuf,
        confirmed: bool,
    },
    Forget {
        scope: umadev_agent::memory_control::MemoryScope,
        selector: umadev_agent::memory_control::MemorySelector,
        confirmed: bool,
    },
    ClearCache {
        store: umadev_agent::memory_control::MemoryStore,
        confirmed: bool,
    },
}

impl MemoryTuiCommand {
    pub(super) const fn mutates(&self) -> bool {
        match self {
            Self::Capture { .. }
            | Self::Recall { .. }
            | Self::RetentionSet { .. }
            | Self::RetentionClear { .. } => true,
            Self::RetentionRun { confirmed, .. }
            | Self::Export { confirmed, .. }
            | Self::Forget { confirmed, .. }
            | Self::ClearCache { confirmed, .. } => *confirmed,
            Self::Inventory { .. } | Self::RetentionView { .. } => false,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) enum MemoryParseError {
    Usage,
    UnclosedQuote,
    InvalidArgument(String),
    MissingScope,
    OneScopeRequired,
    ProjectScopeRequired,
    MissingStore,
    ExactStoreRequired,
    UnknownSelector(String),
    MissingOutput,
    AbsoluteOutputRequired,
    InvalidDays,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum MemoryViewScope {
    Project,
    Global,
    All,
}

impl MemoryViewScope {
    pub(super) fn scopes(self) -> &'static [MemoryScope] {
        use MemoryScope::{Global, Project};
        match self {
            Self::Project => &[Project],
            Self::Global => &[Global],
            Self::All => &[Project, Global],
        }
    }
}

pub(super) fn curated_lesson_status_key(status: umadev_agent::CuratedLessonStatus) -> &'static str {
    use umadev_agent::CuratedLessonStatus;
    match status {
        CuratedLessonStatus::Hypothesis => "lessons.status.hypothesis",
        CuratedLessonStatus::Corroborated => "lessons.status.corroborated",
        CuratedLessonStatus::Validated => "lessons.status.validated",
        CuratedLessonStatus::Invalidated => "lessons.status.invalidated",
    }
}

pub(super) fn curated_lesson_source(lang: umadev_i18n::Lang, source_kind: &str) -> String {
    match source_kind {
        "pitfall" => umadev_i18n::t(lang, "lessons.source.pitfall").to_string(),
        "belief" => umadev_i18n::t(lang, "lessons.source.belief").to_string(),
        "validated_pattern" => umadev_i18n::t(lang, "lessons.source.validated_pattern").to_string(),
        other => umadev_i18n::tf(lang, "lessons.source.other", &[other]),
    }
}

pub(super) fn pitfall_status_key(status: umadev_agent::PitfallStatus) -> &'static str {
    use umadev_agent::PitfallStatus;
    match status {
        PitfallStatus::Hypothesis => "pitfalls.status.hypothesis",
        PitfallStatus::Corroborated => "pitfalls.status.corroborated",
        PitfallStatus::Validated => "pitfalls.status.validated",
        PitfallStatus::Invalidated => "pitfalls.status.invalidated",
    }
}

pub(super) fn pitfall_status_icon(status: umadev_agent::PitfallStatus) -> &'static str {
    use umadev_agent::PitfallStatus;
    match status {
        PitfallStatus::Hypothesis => "[pitfall]",
        PitfallStatus::Corroborated => "[evidence]",
        PitfallStatus::Validated => "[ok]",
        PitfallStatus::Invalidated => "[warn]",
    }
}

pub(super) fn compact_audit_id(value: &str, head: usize, tail: usize) -> String {
    let value = value.trim();
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= head + tail + 1 {
        return value.to_string();
    }
    let start: String = chars.iter().take(head).collect();
    let end: String = chars.iter().skip(chars.len() - tail).collect();
    format!("{start}…{end}")
}
