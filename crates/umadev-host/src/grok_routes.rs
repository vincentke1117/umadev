//! Source-locked Grok Build subagent routing and turn convergence.
//!
//! Grok sends child-session traffic over the same ACP stream as the root. A
//! child becomes authoritative only after a valid `subagent_spawned` envelope
//! from an already-authorized parent. This module keeps that trust graph
//! independent from transcript presentation and makes replay replacement
//! atomic.

use std::collections::{HashMap, HashSet, VecDeque};

use serde_json::Value;
use umadev_runtime::{TurnStatus, Usage};

const MAX_ROUTES: usize = 1_024;
const MAX_EVENT_IDS: usize = 4_096;
const MAX_ID_CHARS: usize = 512;
const MAX_TYPE_CHARS: usize = 256;
const MAX_DESCRIPTION_CHARS: usize = 4_096;
const MAX_OUTPUT_CHARS: usize = 1_048_576;
const MAX_TOOLS: usize = 256;

#[derive(Debug, Clone)]
struct RouteNode {
    subagent_id: String,
    spawn_parent: String,
    current_parent: String,
    turn_prompt_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct SessionRouteGraph {
    root: Option<String>,
    nodes: HashMap<String, RouteNode>,
    children: HashMap<String, HashSet<String>>,
    subagents: HashMap<String, String>,
}

impl SessionRouteGraph {
    fn rooted(root: &str) -> Self {
        Self {
            root: Some(root.to_string()),
            ..Self::default()
        }
    }

    fn authorizes(&self, session_id: &str) -> bool {
        self.root.as_deref() == Some(session_id) || self.nodes.contains_key(session_id)
    }

    fn authorizes_retired_lifecycle(&self, params: &Value, session_id: &str) -> bool {
        let Some(update) = params.get("update") else {
            return false;
        };
        let Some(kind) = wire_string(update, "sessionUpdate", "session_update") else {
            return false;
        };
        if !matches!(kind, "subagent_progress" | "subagent_finished") {
            return false;
        }
        let Some(child) = wire_string(update, "child_session_id", "childSessionId") else {
            return false;
        };
        self.nodes
            .get(child)
            .is_some_and(|node| node.spawn_parent == session_id)
    }

    fn insert_spawn(
        &mut self,
        spawn: &Spawned,
        active_turn_prompt: Option<&str>,
    ) -> Result<bool, RouteError> {
        if !self.authorizes(&spawn.parent_session_id) {
            return Err(RouteError::Unauthorized);
        }
        if self.root.as_deref() == Some(spawn.child_session_id.as_str())
            || spawn.parent_session_id == spawn.child_session_id
        {
            return Err(RouteError::Malformed);
        }
        if let Some(existing) = self.nodes.get(&spawn.child_session_id) {
            return if existing.subagent_id == spawn.subagent_id
                && (existing.spawn_parent == spawn.parent_session_id
                    || existing.current_parent == spawn.parent_session_id)
            {
                Ok(false)
            } else {
                Err(RouteError::Collision)
            };
        }
        if let Some(existing_child) = self.subagents.get(&spawn.subagent_id) {
            return if existing_child == &spawn.child_session_id {
                Ok(false)
            } else {
                Err(RouteError::Collision)
            };
        }
        if self.nodes.len() >= MAX_ROUTES {
            return Err(RouteError::Capacity);
        }
        if self.is_descendant_of(&spawn.parent_session_id, &spawn.child_session_id) {
            return Err(RouteError::Cycle);
        }
        let turn_prompt_id = if self.root.as_deref() == Some(&spawn.parent_session_id) {
            spawn
                .parent_prompt_id
                .clone()
                .or_else(|| active_turn_prompt.map(str::to_string))
        } else {
            self.nodes
                .get(&spawn.parent_session_id)
                .and_then(|parent| parent.turn_prompt_id.clone())
        };
        self.nodes.insert(
            spawn.child_session_id.clone(),
            RouteNode {
                subagent_id: spawn.subagent_id.clone(),
                spawn_parent: spawn.parent_session_id.clone(),
                current_parent: spawn.parent_session_id.clone(),
                turn_prompt_id,
            },
        );
        self.subagents
            .insert(spawn.subagent_id.clone(), spawn.child_session_id.clone());
        self.children
            .entry(spawn.parent_session_id.clone())
            .or_default()
            .insert(spawn.child_session_id.clone());
        Ok(true)
    }

    fn is_descendant_of(&self, candidate: &str, ancestor: &str) -> bool {
        let mut current = Some(candidate);
        let mut visited = HashSet::new();
        while let Some(session_id) = current {
            if session_id == ancestor {
                return true;
            }
            if !visited.insert(session_id) {
                return true;
            }
            current = self
                .nodes
                .get(session_id)
                .map(|node| node.current_parent.as_str());
        }
        false
    }

    fn remove_finished(&mut self, finished: &Finished) -> Result<bool, RouteError> {
        let Some(node) = self.nodes.get(&finished.child_session_id) else {
            return Ok(false);
        };
        if node.subagent_id != finished.subagent_id {
            return Err(RouteError::Collision);
        }
        if finished.envelope_session_id != node.current_parent
            && finished.envelope_session_id != node.spawn_parent
        {
            return Err(RouteError::Unauthorized);
        }
        let node = self
            .nodes
            .remove(&finished.child_session_id)
            .expect("route checked above");
        self.subagents.remove(&node.subagent_id);
        if let Some(siblings) = self.children.get_mut(&node.current_parent) {
            siblings.remove(&finished.child_session_id);
            if siblings.is_empty() {
                self.children.remove(&node.current_parent);
            }
        }
        if let Some(grandchildren) = self.children.remove(&finished.child_session_id) {
            for grandchild in &grandchildren {
                if let Some(route) = self.nodes.get_mut(grandchild) {
                    route.current_parent.clone_from(&node.current_parent);
                }
            }
            self.children
                .entry(node.current_parent)
                .or_default()
                .extend(grandchildren);
        }
        Ok(true)
    }

    fn validate_progress(&self, progress: &Progress) -> Result<(), RouteError> {
        let Some(node) = self.nodes.get(&progress.child_session_id) else {
            return Err(RouteError::Unauthorized);
        };
        if node.subagent_id != progress.subagent_id
            || (progress.parent_session_id != node.spawn_parent
                && progress.parent_session_id != node.current_parent)
        {
            return Err(RouteError::Unauthorized);
        }
        Ok(())
    }

    fn live_subagent_ids(&self) -> Vec<String> {
        let mut ids = self
            .nodes
            .values()
            .map(|node| node.subagent_id.clone())
            .collect::<Vec<_>>();
        ids.sort();
        ids
    }

    fn relevant_live_count(&self, prompt_id: Option<&str>) -> usize {
        let Some(prompt_id) = prompt_id else {
            return 0;
        };
        self.nodes
            .values()
            .filter(|node| node.turn_prompt_id.as_deref() == Some(prompt_id))
            .count()
    }

    fn reconcile_parent(
        &mut self,
        parent: &str,
        snapshots: &[RunningSnapshot],
    ) -> Result<Vec<String>, RouteError> {
        if !self.authorizes(parent) {
            return Err(RouteError::Unauthorized);
        }
        let mut desired = HashSet::with_capacity(snapshots.len());
        for snapshot in snapshots {
            if snapshot.parent_session_id != parent
                || !desired.insert(snapshot.child_session_id.clone())
            {
                return Err(RouteError::Malformed);
            }
        }

        let stale = self
            .nodes
            .iter()
            .filter(|(child, node)| {
                node.spawn_parent == parent
                    && node.current_parent == parent
                    && !desired.contains(*child)
            })
            .map(|(child, node)| (child.clone(), node.subagent_id.clone()))
            .collect::<Vec<_>>();
        for (child_session_id, subagent_id) in stale {
            let finished = Finished {
                event_id: String::new(),
                envelope_session_id: parent.to_string(),
                subagent_id,
                child_session_id,
                will_wake: false,
            };
            self.remove_finished(&finished)?;
        }

        let mut children = Vec::with_capacity(snapshots.len());
        for snapshot in snapshots {
            let spawn = Spawned {
                event_id: String::new(),
                envelope_session_id: parent.to_string(),
                subagent_id: snapshot.subagent_id.clone(),
                parent_session_id: snapshot.parent_session_id.clone(),
                child_session_id: snapshot.child_session_id.clone(),
                parent_prompt_id: None,
            };
            self.insert_spawn(&spawn, None)?;
            children.push(snapshot.child_session_id.clone());
        }
        Ok(children)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_field_names)]
pub(crate) struct RunningSnapshot {
    pub(crate) subagent_id: String,
    pub(crate) parent_session_id: String,
    pub(crate) child_session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_field_names)]
struct Spawned {
    event_id: String,
    envelope_session_id: String,
    subagent_id: String,
    parent_session_id: String,
    child_session_id: String,
    parent_prompt_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_field_names)]
struct Progress {
    event_id: Option<String>,
    envelope_session_id: String,
    subagent_id: String,
    parent_session_id: String,
    child_session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Finished {
    event_id: String,
    envelope_session_id: String,
    subagent_id: String,
    child_session_id: String,
    will_wake: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Lifecycle {
    Spawned(Spawned),
    Progress(Progress),
    Finished(Finished),
}

impl Lifecycle {
    fn event_id(&self) -> Option<&str> {
        match self {
            Self::Spawned(event) => Some(&event.event_id),
            Self::Progress(event) => event.event_id.as_deref(),
            Self::Finished(event) => Some(&event.event_id),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LifecycleEffect {
    Started {
        subagent_id: String,
    },
    Progress,
    Finished {
        subagent_id: String,
        child_session_id: String,
        will_wake: bool,
    },
    Duplicate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RouteError {
    Malformed,
    Unauthorized,
    Collision,
    Cycle,
    Capacity,
}

#[derive(Debug, Clone)]
struct Terminal {
    status: TurnStatus,
    usage: Option<Usage>,
}

impl Terminal {
    fn merge(&mut self, later: Self) {
        self.usage = merge_usage(self.usage, later.usage);
        self.status = merge_status(
            std::mem::replace(&mut self.status, TurnStatus::Completed),
            later.status,
        );
    }
}

#[derive(Debug, Default)]
struct TurnConvergence {
    prompt_id: Option<String>,
    root_terminal: Option<Terminal>,
    synthetic_terminal: Option<Terminal>,
    expected_wakes: HashSet<String>,
}

impl TurnConvergence {
    fn begin(&mut self, prompt_id: &str) {
        *self = Self {
            prompt_id: Some(prompt_id.to_string()),
            ..Self::default()
        };
    }

    fn remember_wake(&mut self, subagent_id: &str) {
        if self.prompt_id.is_some() {
            self.expected_wakes
                .insert(format!("subagent-completed-{subagent_id}"));
        }
    }

    fn root_terminal(
        &mut self,
        prompt_id: &str,
        status: TurnStatus,
        usage: Option<Usage>,
        live_routes: usize,
    ) -> Option<ConvergedTerminal> {
        if self.prompt_id.as_deref() != Some(prompt_id) || self.root_terminal.is_some() {
            return None;
        }
        self.root_terminal = Some(Terminal { status, usage });
        self.take_if_ready(live_routes)
    }

    fn synthetic_terminal(
        &mut self,
        prompt_id: &str,
        status: TurnStatus,
        usage: Option<Usage>,
        live_routes: usize,
    ) -> Option<ConvergedTerminal> {
        if !self.expected_wakes.remove(prompt_id) {
            return None;
        }
        let terminal = Terminal { status, usage };
        if let Some(existing) = &mut self.synthetic_terminal {
            existing.merge(terminal);
        } else {
            self.synthetic_terminal = Some(terminal);
        }
        self.take_if_ready(live_routes)
    }

    fn take_if_ready(&mut self, live_routes: usize) -> Option<ConvergedTerminal> {
        if live_routes != 0 || !self.expected_wakes.is_empty() {
            return None;
        }
        let mut terminal = self.root_terminal.take()?;
        if let Some(synthetic) = self.synthetic_terminal.take() {
            // A synthetic `subagent-completed-*` turn is a new model prompt,
            // not the child ledger that Grok may already have folded into the
            // root prompt. Its own usage is therefore distinct and must be
            // included. `merge_usage` keeps the total incomplete whenever one
            // of these prompt-level reports is absent or already incomplete.
            terminal.merge(synthetic);
        }
        self.prompt_id = None;
        Some(ConvergedTerminal {
            status: terminal.status,
            usage: terminal.usage,
        })
    }

    fn cancel_children(&mut self) {
        self.expected_wakes.clear();
    }
}

/// Root/descendant authority plus one logical UmaDev turn's convergence state.
#[derive(Debug, Default)]
pub(crate) struct SessionRouteState {
    active: SessionRouteGraph,
    replay: Option<SessionRouteGraph>,
    seen_event_ids: HashSet<String>,
    event_order: VecDeque<String>,
    turn: TurnConvergence,
}

impl SessionRouteState {
    pub(crate) fn activate_root(&mut self, root: &str) -> Result<(), RouteError> {
        validate_identifier(root)?;
        self.active = SessionRouteGraph::rooted(root);
        self.replay = None;
        self.seen_event_ids.clear();
        self.event_order.clear();
        self.turn = TurnConvergence::default();
        Ok(())
    }

    pub(crate) fn begin_replay(&mut self, root: &str) -> Result<(), RouteError> {
        validate_identifier(root)?;
        self.active = SessionRouteGraph::rooted(root);
        self.replay = Some(SessionRouteGraph::rooted(root));
        self.seen_event_ids.clear();
        self.event_order.clear();
        self.turn = TurnConvergence::default();
        Ok(())
    }

    pub(crate) fn commit_replay(&mut self) {
        if let Some(replay) = self.replay.take() {
            self.active = replay;
        }
    }

    pub(crate) fn clear_failed_replay(&mut self) {
        self.active = SessionRouteGraph::default();
        self.replay = None;
        self.seen_event_ids.clear();
        self.event_order.clear();
        self.turn = TurnConvergence::default();
    }

    pub(crate) fn authorizes(&self, params: &Value, replaying: bool) -> bool {
        let Some(session_id) = wire_string(params, "sessionId", "session_id") else {
            return true;
        };
        let graph = self.graph(replaying);
        graph.authorizes(session_id) || graph.authorizes_retired_lifecycle(params, session_id)
    }

    pub(crate) fn apply_lifecycle(
        &mut self,
        params: &Value,
        replaying: bool,
    ) -> Result<Option<LifecycleEffect>, RouteError> {
        let Some(lifecycle) = parse_lifecycle(params)? else {
            return Ok(None);
        };
        if lifecycle
            .event_id()
            .is_some_and(|event_id| self.seen_event_ids.contains(event_id))
        {
            return Ok(Some(LifecycleEffect::Duplicate));
        }

        let active_turn_prompt = self.turn.prompt_id.clone();
        let effect = {
            let graph = self.graph_mut(replaying);
            match &lifecycle {
                Lifecycle::Spawned(spawn) => {
                    if spawn.envelope_session_id != spawn.parent_session_id {
                        return Err(RouteError::Unauthorized);
                    }
                    let inserted = graph.insert_spawn(spawn, active_turn_prompt.as_deref())?;
                    if inserted {
                        LifecycleEffect::Started {
                            subagent_id: spawn.subagent_id.clone(),
                        }
                    } else {
                        LifecycleEffect::Duplicate
                    }
                }
                Lifecycle::Progress(progress) => {
                    if progress.envelope_session_id != progress.parent_session_id {
                        return Err(RouteError::Unauthorized);
                    }
                    graph.validate_progress(progress)?;
                    LifecycleEffect::Progress
                }
                Lifecycle::Finished(finished) => {
                    let removed = graph.remove_finished(finished)?;
                    if removed {
                        LifecycleEffect::Finished {
                            subagent_id: finished.subagent_id.clone(),
                            child_session_id: finished.child_session_id.clone(),
                            will_wake: finished.will_wake,
                        }
                    } else {
                        LifecycleEffect::Duplicate
                    }
                }
            }
        };
        if let Some(event_id) = lifecycle.event_id() {
            self.remember_event(event_id);
        }
        if let LifecycleEffect::Finished {
            subagent_id,
            will_wake: true,
            ..
        } = &effect
        {
            self.turn.remember_wake(subagent_id);
        }
        Ok(Some(effect))
    }

    pub(crate) fn reconcile_running(
        &mut self,
        parent: &str,
        result: &Value,
    ) -> Result<Vec<String>, RouteError> {
        let snapshots = parse_running_snapshots(result)?;
        self.active.reconcile_parent(parent, &snapshots)
    }

    pub(crate) fn live_subagent_ids(&self) -> Vec<String> {
        self.active.live_subagent_ids()
    }

    pub(crate) fn begin_turn(&mut self, prompt_id: &str) {
        self.turn.begin(prompt_id);
    }

    pub(crate) fn settle_root(
        &mut self,
        prompt_id: &str,
        status: TurnStatus,
        usage: Option<Usage>,
    ) -> Option<ConvergedTerminal> {
        let live_routes = self
            .active
            .relevant_live_count(self.turn.prompt_id.as_deref());
        self.turn
            .root_terminal(prompt_id, status, usage, live_routes)
    }

    pub(crate) fn settle_synthetic(
        &mut self,
        prompt_id: &str,
        status: TurnStatus,
        usage: Option<Usage>,
    ) -> Option<ConvergedTerminal> {
        let live_routes = self
            .active
            .relevant_live_count(self.turn.prompt_id.as_deref());
        self.turn
            .synthetic_terminal(prompt_id, status, usage, live_routes)
    }

    pub(crate) fn settle_after_lifecycle(&mut self) -> Option<ConvergedTerminal> {
        let live_routes = self
            .active
            .relevant_live_count(self.turn.prompt_id.as_deref());
        self.turn.take_if_ready(live_routes)
    }

    pub(crate) fn cancel_descendants(&mut self) -> Vec<String> {
        let live = self.active.live_subagent_ids();
        let root = self.active.root.clone();
        self.active = root
            .as_deref()
            .map_or_else(SessionRouteGraph::default, SessionRouteGraph::rooted);
        self.replay = None;
        self.turn.cancel_children();
        live
    }

    fn graph(&self, replaying: bool) -> &SessionRouteGraph {
        if replaying {
            self.replay.as_ref().unwrap_or(&self.active)
        } else {
            &self.active
        }
    }

    fn graph_mut(&mut self, replaying: bool) -> &mut SessionRouteGraph {
        if replaying {
            if let Some(replay) = self.replay.as_mut() {
                return replay;
            }
        }
        &mut self.active
    }

    fn remember_event(&mut self, event_id: &str) {
        if !self.seen_event_ids.insert(event_id.to_string()) {
            return;
        }
        self.event_order.push_back(event_id.to_string());
        while self.event_order.len() > MAX_EVENT_IDS {
            if let Some(expired) = self.event_order.pop_front() {
                self.seen_event_ids.remove(&expired);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConvergedTerminal {
    pub(crate) status: TurnStatus,
    pub(crate) usage: Option<Usage>,
}

fn parse_lifecycle(params: &Value) -> Result<Option<Lifecycle>, RouteError> {
    let Some(update) = params.get("update") else {
        return Ok(None);
    };
    let Some(kind) = wire_string(update, "sessionUpdate", "session_update") else {
        return Ok(None);
    };
    if !matches!(
        kind,
        "subagent_spawned" | "subagent_progress" | "subagent_finished"
    ) {
        return Ok(None);
    }
    let envelope_session_id = required_id(params, "sessionId", "session_id")?;
    let subagent_id = required_id(update, "subagent_id", "subagentId")?;
    let child_session_id = required_id(update, "child_session_id", "childSessionId")?;
    let lifecycle = match kind {
        "subagent_spawned" => parse_spawned(
            params,
            update,
            envelope_session_id,
            subagent_id,
            child_session_id,
        )?,
        "subagent_progress" => parse_progress(
            params,
            update,
            envelope_session_id,
            subagent_id,
            child_session_id,
        )?,
        "subagent_finished" => parse_finished(
            params,
            update,
            envelope_session_id,
            subagent_id,
            child_session_id,
        )?,
        _ => unreachable!("lifecycle kind filtered above"),
    };
    Ok(Some(lifecycle))
}

fn parse_spawned(
    params: &Value,
    update: &Value,
    envelope_session_id: String,
    subagent_id: String,
    child_session_id: String,
) -> Result<Lifecycle, RouteError> {
    let event_id = required_event_id(params)?;
    let parent_session_id = required_id(update, "parent_session_id", "parentSessionId")?;
    bounded_required_text(update, "subagent_type", "subagentType", MAX_TYPE_CHARS)?;
    bounded_required_text(update, "description", "description", MAX_DESCRIPTION_CHARS)?;
    let parent_prompt_id =
        bounded_optional_identifier(update, "parent_prompt_id", "parentPromptId")?;
    bounded_optional_text(
        update,
        "effective_context_source",
        "effectiveContextSource",
        MAX_TYPE_CHARS,
    )?;
    bounded_optional_text(update, "capability_mode", "capabilityMode", MAX_TYPE_CHARS)?;
    bounded_optional_text(update, "persona", "persona", MAX_TYPE_CHARS)?;
    bounded_optional_text(update, "role", "role", MAX_TYPE_CHARS)?;
    bounded_optional_text(update, "model", "model", MAX_ID_CHARS)?;
    bounded_optional_id(update, "resumed_from", "resumedFrom")?;
    if wire_value(update, "context_normalized", "contextNormalized")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(RouteError::Malformed);
    }
    Ok(Lifecycle::Spawned(Spawned {
        event_id,
        envelope_session_id,
        subagent_id,
        parent_session_id,
        child_session_id,
        parent_prompt_id,
    }))
}

fn parse_progress(
    params: &Value,
    update: &Value,
    envelope_session_id: String,
    subagent_id: String,
    child_session_id: String,
) -> Result<Lifecycle, RouteError> {
    let parent_session_id = required_id(update, "parent_session_id", "parentSessionId")?;
    required_u64(update, "duration_ms", "durationMs")?;
    required_u32(update, "turn_count", "turnCount")?;
    required_u32(update, "tool_call_count", "toolCallCount")?;
    required_u64(update, "tokens_used", "tokensUsed")?;
    required_u64(update, "context_window_tokens", "contextWindowTokens")?;
    if required_u64(update, "context_usage_pct", "contextUsagePct")? > 100 {
        return Err(RouteError::Malformed);
    }
    required_u32(update, "error_count", "errorCount")?;
    validate_tools(update)?;
    Ok(Lifecycle::Progress(Progress {
        event_id: optional_event_id(params)?,
        envelope_session_id,
        subagent_id,
        parent_session_id,
        child_session_id,
    }))
}

fn parse_finished(
    params: &Value,
    update: &Value,
    envelope_session_id: String,
    subagent_id: String,
    child_session_id: String,
) -> Result<Lifecycle, RouteError> {
    if !matches!(
        update.get("status").and_then(Value::as_str),
        Some("completed" | "failed" | "cancelled")
    ) {
        return Err(RouteError::Malformed);
    }
    required_u32(update, "tool_calls", "toolCalls")?;
    required_u32(update, "turns", "turns")?;
    required_u64(update, "duration_ms", "durationMs")?;
    optional_u64(update, "tokens_used", "tokensUsed")?;
    bounded_optional_text(update, "error", "error", MAX_DESCRIPTION_CHARS)?;
    bounded_optional_text(update, "output", "output", MAX_OUTPUT_CHARS)?;
    let will_wake = match wire_value(update, "will_wake", "willWake") {
        None => false,
        Some(value) => value.as_bool().ok_or(RouteError::Malformed)?,
    };
    Ok(Lifecycle::Finished(Finished {
        event_id: required_event_id(params)?,
        envelope_session_id,
        subagent_id,
        child_session_id,
        will_wake,
    }))
}

fn validate_tools(value: &Value) -> Result<(), RouteError> {
    let tools = wire_value(value, "tools_used", "toolsUsed")
        .and_then(Value::as_array)
        .ok_or(RouteError::Malformed)?;
    if tools.len() > MAX_TOOLS
        || tools.iter().any(|tool| {
            tool.as_str()
                .is_none_or(|tool| !valid_text(tool, MAX_TYPE_CHARS))
        })
    {
        Err(RouteError::Malformed)
    } else {
        Ok(())
    }
}

fn parse_running_snapshots(result: &Value) -> Result<Vec<RunningSnapshot>, RouteError> {
    let subagents = result
        .get("subagents")
        .or_else(|| result.pointer("/result/subagents"))
        .and_then(Value::as_array)
        .ok_or(RouteError::Malformed)?;
    if subagents.len() > MAX_ROUTES {
        return Err(RouteError::Capacity);
    }
    subagents
        .iter()
        .map(|snapshot| {
            let subagent_id = required_id(snapshot, "subagent_id", "subagentId")?;
            let parent_session_id = required_id(snapshot, "parent_session_id", "parentSessionId")?;
            let child_session_id = required_id(snapshot, "child_session_id", "childSessionId")?;
            bounded_required_text(snapshot, "subagent_type", "subagentType", MAX_TYPE_CHARS)?;
            bounded_required_text(
                snapshot,
                "description",
                "description",
                MAX_DESCRIPTION_CHARS,
            )?;
            required_u64(snapshot, "started_at_epoch_ms", "startedAtEpochMs")?;
            required_u64(snapshot, "duration_ms", "durationMs")?;
            required_u32(snapshot, "turn_count", "turnCount")?;
            required_u32(snapshot, "tool_call_count", "toolCallCount")?;
            required_u64(snapshot, "tokens_used", "tokensUsed")?;
            required_u64(snapshot, "context_window_tokens", "contextWindowTokens")?;
            if required_u64(snapshot, "context_usage_pct", "contextUsagePct")? > 100 {
                return Err(RouteError::Malformed);
            }
            required_u32(snapshot, "error_count", "errorCount")?;
            validate_tools(snapshot)?;
            Ok(RunningSnapshot {
                subagent_id,
                parent_session_id,
                child_session_id,
            })
        })
        .collect()
}

fn required_event_id(params: &Value) -> Result<String, RouteError> {
    optional_event_id(params)?.ok_or(RouteError::Malformed)
}

fn optional_event_id(params: &Value) -> Result<Option<String>, RouteError> {
    let value = params
        .pointer("/_meta/eventId")
        .or_else(|| params.pointer("/_meta/event_id"));
    match value {
        None => Ok(None),
        Some(value) => {
            let id = value.as_str().ok_or(RouteError::Malformed)?;
            validate_identifier(id)?;
            Ok(Some(id.to_string()))
        }
    }
}

fn required_id(value: &Value, snake: &str, camel: &str) -> Result<String, RouteError> {
    let id = wire_string(value, snake, camel).ok_or(RouteError::Malformed)?;
    validate_identifier(id)?;
    Ok(id.to_string())
}

fn bounded_optional_id(value: &Value, snake: &str, camel: &str) -> Result<(), RouteError> {
    let Some(value) = wire_value(value, snake, camel) else {
        return Ok(());
    };
    if value.is_null() {
        return Ok(());
    }
    validate_identifier(value.as_str().ok_or(RouteError::Malformed)?)
}

fn bounded_optional_identifier(
    value: &Value,
    snake: &str,
    camel: &str,
) -> Result<Option<String>, RouteError> {
    let Some(value) = wire_value(value, snake, camel) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let id = value.as_str().ok_or(RouteError::Malformed)?;
    validate_identifier(id)?;
    Ok(Some(id.to_string()))
}

fn validate_identifier(value: &str) -> Result<(), RouteError> {
    if valid_text(value, MAX_ID_CHARS) && !value.trim().is_empty() {
        Ok(())
    } else {
        Err(RouteError::Malformed)
    }
}

fn bounded_required_text<'a>(
    value: &'a Value,
    snake: &str,
    camel: &str,
    max_chars: usize,
) -> Result<&'a str, RouteError> {
    let text = wire_string(value, snake, camel).ok_or(RouteError::Malformed)?;
    valid_text(text, max_chars)
        .then_some(text)
        .ok_or(RouteError::Malformed)
}

fn bounded_optional_text(
    value: &Value,
    snake: &str,
    camel: &str,
    max_chars: usize,
) -> Result<(), RouteError> {
    let Some(value) = wire_value(value, snake, camel) else {
        return Ok(());
    };
    if value.is_null() {
        return Ok(());
    }
    let text = value.as_str().ok_or(RouteError::Malformed)?;
    if valid_text(text, max_chars) {
        Ok(())
    } else {
        Err(RouteError::Malformed)
    }
}

fn valid_text(value: &str, max_chars: usize) -> bool {
    !value.chars().any(char::is_control) && value.chars().take(max_chars + 1).count() <= max_chars
}

fn required_u64(value: &Value, snake: &str, camel: &str) -> Result<u64, RouteError> {
    wire_value(value, snake, camel)
        .and_then(Value::as_u64)
        .ok_or(RouteError::Malformed)
}

fn required_u32(value: &Value, snake: &str, camel: &str) -> Result<u32, RouteError> {
    u32::try_from(required_u64(value, snake, camel)?).map_err(|_| RouteError::Malformed)
}

fn optional_u64(value: &Value, snake: &str, camel: &str) -> Result<(), RouteError> {
    match wire_value(value, snake, camel) {
        None | Some(Value::Null) => Ok(()),
        Some(value) if value.as_u64().is_some() => Ok(()),
        Some(_) => Err(RouteError::Malformed),
    }
}

fn wire_value<'a>(value: &'a Value, snake: &str, camel: &str) -> Option<&'a Value> {
    value.get(snake).or_else(|| value.get(camel))
}

fn wire_string<'a>(value: &'a Value, snake: &str, camel: &str) -> Option<&'a str> {
    wire_value(value, snake, camel).and_then(Value::as_str)
}

fn merge_usage(left: Option<Usage>, right: Option<Usage>) -> Option<Usage> {
    match (left, right) {
        (None, None) => None,
        // Each side is a distinct logical prompt. If one terminal omitted usage,
        // the known side remains a useful lower bound but the aggregate is not
        // exact and no cost may survive.
        (Some(usage), None) | (None, Some(usage)) => Some(usage.into_incomplete()),
        (Some(left), Some(right)) => Some(left.merge(right)),
    }
}

fn merge_status(first: TurnStatus, later: TurnStatus) -> TurnStatus {
    match (first, later) {
        (TurnStatus::Failed(reason), _) | (_, TurnStatus::Failed(reason)) => {
            TurnStatus::Failed(reason)
        }
        (TurnStatus::Interrupted, _) | (_, TurnStatus::Interrupted) => TurnStatus::Interrupted,
        (TurnStatus::Truncated, _) | (_, TurnStatus::Truncated) => TurnStatus::Truncated,
        (TurnStatus::Completed, TurnStatus::Completed) => TurnStatus::Completed,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn spawn(parent: &str, child: &str, subagent: &str, event: &str) -> Value {
        json!({
            "sessionId":parent,
            "_meta":{"eventId":event},
            "update":{
                "sessionUpdate":"subagent_spawned",
                "subagent_id":subagent,
                "parent_session_id":parent,
                "parent_prompt_id":"prompt-1",
                "child_session_id":child,
                "subagent_type":"general-purpose",
                "description":"verify the project",
                "context_normalized":false
            }
        })
    }

    fn finish(parent: &str, child: &str, subagent: &str, event: &str, will_wake: bool) -> Value {
        json!({
            "sessionId":parent,
            "_meta":{"eventId":event},
            "update":{
                "sessionUpdate":"subagent_finished",
                "subagent_id":subagent,
                "child_session_id":child,
                "status":"completed",
                "tool_calls":2,
                "turns":1,
                "duration_ms":30,
                "tokens_used":10,
                "will_wake":will_wake
            }
        })
    }

    fn running(parent: &str, child: &str, subagent: &str) -> Value {
        json!({
            "subagentId":subagent,
            "parentSessionId":parent,
            "childSessionId":child,
            "subagentType":"general-purpose",
            "description":"verify",
            "startedAtEpochMs":1,
            "durationMs":2,
            "turnCount":0,
            "toolCallCount":0,
            "tokensUsed":0,
            "contextWindowTokens":1000,
            "contextUsagePct":0,
            "toolsUsed":[],
            "errorCount":0
        })
    }

    #[test]
    fn spawn_precedes_and_authorizes_child_interactions_but_forged_ids_fail() {
        let mut routes = SessionRouteState::default();
        routes.activate_root("root").unwrap();
        assert!(!routes.authorizes(&json!({"sessionId":"child"}), false));
        assert!(matches!(
            routes.apply_lifecycle(&spawn("root", "child", "agent", "e1"), false),
            Ok(Some(LifecycleEffect::Started { .. }))
        ));
        assert!(routes.authorizes(&json!({"sessionId":"child"}), false));
        assert!(!routes.authorizes(&json!({"sessionId":"forged"}), false));
    }

    #[test]
    fn nested_finish_reparents_live_grandchild_without_authorizing_retired_parent() {
        let mut routes = SessionRouteState::default();
        routes.activate_root("root").unwrap();
        routes
            .apply_lifecycle(&spawn("root", "child", "a", "e1"), false)
            .unwrap();
        routes
            .apply_lifecycle(&spawn("child", "grandchild", "b", "e2"), false)
            .unwrap();
        routes
            .apply_lifecycle(&finish("root", "child", "a", "e3", false), false)
            .unwrap();
        assert!(routes.authorizes(&json!({"sessionId":"grandchild"}), false));
        assert!(!routes.authorizes(&json!({"sessionId":"child"}), false));
        assert!(routes.authorizes(&finish("child", "grandchild", "b", "e4", false), false));
        assert!(matches!(
            routes.apply_lifecycle(&finish("child", "grandchild", "b", "e4", false), false),
            Ok(Some(LifecycleEffect::Finished { .. }))
        ));
    }

    #[test]
    fn lifecycle_identity_and_field_bounds_are_fail_closed() {
        let mut routes = SessionRouteState::default();
        routes.activate_root("root").unwrap();
        let mut wrong_parent = spawn("root", "child", "a", "e1");
        wrong_parent["update"]["parent_session_id"] = json!("foreign");
        assert_eq!(
            routes.apply_lifecycle(&wrong_parent, false),
            Err(RouteError::Unauthorized)
        );
        let mut control = spawn("root", "child\nforged", "a", "e2");
        assert_eq!(
            routes.apply_lifecycle(&control, false),
            Err(RouteError::Malformed)
        );
        control["update"]["child_session_id"] = json!("child");
        control["update"]["description"] = json!("x".repeat(MAX_DESCRIPTION_CHARS + 1));
        assert_eq!(
            routes.apply_lifecycle(&control, false),
            Err(RouteError::Malformed)
        );
    }

    #[test]
    fn event_id_and_live_overlap_are_idempotent() {
        let mut routes = SessionRouteState::default();
        routes.activate_root("root").unwrap();
        let edge = spawn("root", "child", "a", "e1");
        routes.apply_lifecycle(&edge, false).unwrap();
        assert_eq!(
            routes.apply_lifecycle(&edge, false).unwrap(),
            Some(LifecycleEffect::Duplicate)
        );
        let overlap = spawn("root", "child", "a", "e2");
        assert_eq!(
            routes.apply_lifecycle(&overlap, false).unwrap(),
            Some(LifecycleEffect::Duplicate)
        );
        assert_eq!(routes.live_subagent_ids(), ["a"]);
    }

    #[test]
    fn replay_replacement_is_atomic_and_failure_clears_authority() {
        let mut routes = SessionRouteState::default();
        routes.activate_root("old-root").unwrap();
        routes.begin_replay("root").unwrap();
        routes
            .apply_lifecycle(&spawn("root", "child", "a", "e1"), true)
            .unwrap();
        assert!(!routes.authorizes(&json!({"sessionId":"child"}), false));
        assert!(routes.authorizes(&json!({"sessionId":"child"}), true));
        routes.commit_replay();
        assert!(routes.authorizes(&json!({"sessionId":"child"}), false));

        routes.begin_replay("root").unwrap();
        routes.clear_failed_replay();
        assert!(!routes.authorizes(&json!({"sessionId":"root"}), false));
        assert!(!routes.authorizes(&json!({"sessionId":"child"}), false));
    }

    #[test]
    fn list_running_heals_orphans_and_discovers_nested_parents() {
        let mut routes = SessionRouteState::default();
        routes.activate_root("root").unwrap();
        routes
            .apply_lifecycle(&spawn("root", "stale", "stale-agent", "e1"), false)
            .unwrap();
        let children = routes
            .reconcile_running(
                "root",
                &json!({"subagents":[running("root", "child", "a")] }),
            )
            .unwrap();
        assert_eq!(children, ["child"]);
        assert!(!routes.authorizes(&json!({"sessionId":"stale"}), false));
        routes
            .reconcile_running(
                "child",
                &json!({"subagents":[running("child", "grandchild", "b")] }),
            )
            .unwrap();
        assert!(routes.authorizes(&json!({"sessionId":"grandchild"}), false));
    }

    #[test]
    fn root_terminal_waits_for_descendants_then_converges_once() {
        let mut routes = SessionRouteState::default();
        routes.activate_root("root").unwrap();
        routes.begin_turn("prompt-1");
        routes
            .apply_lifecycle(&spawn("root", "child", "a", "e1"), false)
            .unwrap();
        assert!(routes
            .settle_root("prompt-1", TurnStatus::Completed, None)
            .is_none());
        routes
            .apply_lifecycle(&finish("root", "child", "a", "e2", false), false)
            .unwrap();
        assert_eq!(
            routes.settle_after_lifecycle(),
            Some(ConvergedTerminal {
                status: TurnStatus::Completed,
                usage: None
            })
        );
        assert!(routes.settle_after_lifecycle().is_none());
    }

    #[test]
    fn will_wake_adds_the_distinct_synthetic_prompt_without_losing_quality() {
        let mut routes = SessionRouteState::default();
        routes.activate_root("root").unwrap();
        routes.begin_turn("prompt-1");
        routes
            .apply_lifecycle(&spawn("root", "child", "a", "e1"), false)
            .unwrap();
        assert!(routes
            .settle_root("prompt-1", TurnStatus::Completed, Some(Usage::exact(3, 2)))
            .is_none());
        routes
            .apply_lifecycle(&finish("root", "child", "a", "e2", true), false)
            .unwrap();
        assert!(routes.settle_after_lifecycle().is_none());
        assert!(routes
            .settle_synthetic("subagent-completed-forged", TurnStatus::Completed, None)
            .is_none());
        assert_eq!(
            routes.settle_synthetic(
                "subagent-completed-a",
                TurnStatus::Completed,
                Some(Usage::exact(5, 4))
            ),
            Some(ConvergedTerminal {
                status: TurnStatus::Completed,
                usage: Some(Usage::exact(8, 6))
            })
        );
        assert!(routes
            .settle_synthetic("subagent-completed-a", TurnStatus::Completed, None)
            .is_none());
    }

    #[test]
    fn cancel_clears_descendants_and_wake_waits_boundedly() {
        let mut routes = SessionRouteState::default();
        routes.activate_root("root").unwrap();
        routes.begin_turn("prompt-1");
        routes
            .apply_lifecycle(&spawn("root", "child", "a", "e1"), false)
            .unwrap();
        routes
            .apply_lifecycle(&finish("root", "child", "a", "e2", true), false)
            .unwrap();
        assert_eq!(routes.cancel_descendants(), Vec::<String>::new());
        assert_eq!(
            routes.settle_root("prompt-1", TurnStatus::Interrupted, None),
            Some(ConvergedTerminal {
                status: TurnStatus::Interrupted,
                usage: None
            })
        );
    }
}
