//! Bounded presentation of native events. This state never authorizes execution.
use pablo_core::{EventKind, RunEvent};
use std::collections::BTreeMap;
pub const TRANSCRIPT_BYTES: usize = 64 * 1024;
const LABEL_BYTES: usize = 256;
#[derive(Default)]
pub struct View {
    pub transcript: String,
    pub model: String,
    pub trace: String,
    pub status: String,
    pub usage: String,
    pub policy: String,
    pub activity: BTreeMap<String, String>,
    pub skills: Vec<String>,
    pub trimmed: bool,
    pub terminal_seen: bool,
    pub revision: u64,
    assistant_active: bool,
    observed: Option<pablo_core::task::Accounting>,
}
fn visible(c: char) -> char {
    if (c.is_control() && c != '\n')
        || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
    {
        '�'
    } else {
        c
    }
}
pub fn label(text: &str) -> String {
    let mut out = String::new();
    for c in text
        .chars()
        .map(visible)
        .map(|c| if c == '\n' { ' ' } else { c })
    {
        if out.len() + c.len_utf8() > LABEL_BYTES {
            break;
        }
        out.push(c);
    }
    out
}
impl View {
    pub fn reset(&mut self, input: &str) {
        let transcript = std::mem::take(&mut self.transcript);
        let trimmed = self.trimmed;
        let revision = self.revision.wrapping_add(1);
        *self = Self {
            transcript,
            trimmed,
            revision,
            status: "Preparing".into(),
            policy: "Static policy; no approvals".into(),
            ..Self::default()
        };
        if !self.transcript.is_empty() {
            self.push("\n\n---\n");
        }
        self.push("You: ");
        self.push(input);
        self.push("\n\n");
    }
    pub fn invalidate(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
    pub fn push(&mut self, text: &str) {
        self.invalidate();
        for c in text.chars().map(visible) {
            self.transcript.push(c);
            if self.transcript.len() > TRANSCRIPT_BYTES + 4096 {
                self.trim();
            }
        }
        self.trim();
    }
    fn trim(&mut self) {
        if self.transcript.len() <= TRANSCRIPT_BYTES {
            return;
        }
        let mut end = self.transcript.len() - TRANSCRIPT_BYTES;
        while !self.transcript.is_char_boundary(end) {
            end += 1;
        }
        self.transcript.drain(..end);
        self.trimmed = true;
    }
    pub fn event(&mut self, event: &RunEvent) {
        self.invalidate();
        let root = event.agent.as_ref().is_none_or(|agent| agent.depth() == 0);
        let id = label(
            event
                .agent
                .as_ref()
                .map(|a| a.agent_id())
                .unwrap_or(&event.run_id),
        );
        if root {
            self.trace = label(&event.trace_id);
        }
        if matches!(
            event.kind,
            EventKind::ModelStarted { .. }
                | EventKind::ModelFinished { .. }
                | EventKind::ToolStarted { .. }
        ) {
            let totals = self.observed.get_or_insert_with(Default::default);
            match &event.kind {
                EventKind::ModelStarted { .. } => {
                    totals.model_calls = totals.model_calls.saturating_add(1)
                }
                EventKind::ToolStarted { .. } => {
                    totals.tool_calls = totals.tool_calls.saturating_add(1)
                }
                EventKind::ModelFinished { usage, .. } => {
                    totals.usage.input_tokens = totals
                        .usage
                        .input_tokens
                        .zip(usage.input_tokens)
                        .and_then(|(a, b)| a.checked_add(b));
                    totals.usage.output_tokens = totals
                        .usage
                        .output_tokens
                        .zip(usage.output_tokens)
                        .and_then(|(a, b)| a.checked_add(b));
                    totals.cost_microusd = None;
                }
                _ => {}
            }
        }
        if root && let Some(accounting) = &event.accounting {
            self.observed = Some((**accounting).clone());
        }
        if let Some(accounting) = &self.observed {
            self.usage = format!(
                "Calls {} / tools {} | tokens in {} out {} | cost {} microUSD",
                accounting.model_calls,
                accounting.tool_calls,
                number(accounting.usage.input_tokens),
                number(accounting.usage.output_tokens),
                number(accounting.cost_microusd)
            );
        }
        match &event.kind {
            EventKind::RunStarted => {
                if root {
                    self.status = "Running".into();
                }
                self.active(
                    &id,
                    if root {
                        "Root running"
                    } else {
                        "Child running"
                    },
                );
            }
            EventKind::ModelStarted { provider, model } => {
                if root {
                    self.model = label(&format!("{provider} / {model}"));
                }
                self.active(&id, "Model streaming");
            }
            EventKind::TextDelta { text } => {
                if root && !self.assistant_active {
                    self.push("Assistant:\n");
                    self.assistant_active = true;
                }
                if !root {
                    self.push("[child] ");
                }
                self.push(text);
            }
            EventKind::ToolStarted { call } => {
                self.active(&id, &format!("Tool {}", label(&call.name)));
                self.push(&format!(
                    "\n\nTool: `{}` [{}] — running\n",
                    label(&call.name),
                    label(&call.id)
                ));
                self.assistant_active = false;
            }
            EventKind::ToolFinished {
                call_id,
                name,
                result,
            } => {
                self.push(&format!(
                    "Tool: `{}` [{}] — {:?}\n\n",
                    label(name),
                    label(call_id),
                    result.status
                ));
                self.assistant_active = false;
                self.active(&id, "Running");
                self.policy = label(&format!(
                    "Static policy | {} {:?}",
                    label(name),
                    result.status
                ));
                if let Some(value) = &result.subagent {
                    if let Some(agent) = value.get("agent") {
                        self.child(agent);
                    }
                    if let Some(agent) = value.get("snapshot").and_then(|v| v.get("agent")) {
                        self.child(agent);
                    }
                    if let Some(settled) = value.get("settled").and_then(|v| v.as_array()) {
                        for snapshot in settled.iter().take(16) {
                            if let Some(agent) = snapshot.get("agent") {
                                self.child(agent);
                            }
                        }
                    }
                }
            }
            #[cfg(unix)]
            EventKind::SkillActivated { skill, .. } => {
                let name = label(&skill.qualified_name);
                if self.skills.len() < 16 && !self.skills.contains(&name) {
                    self.skills.push(name);
                }
            }
            EventKind::A2aUpdate { remote } => {
                self.active(&id, &format!("Remote {:?}", remote.state))
            }
            EventKind::CompactionStarted => self.active(&id, "Compacting context"),
            EventKind::RunFinished { outcome } => {
                self.activity.remove(&id);
                if root {
                    self.terminal_seen = true;
                    self.status = label(outcome.label());
                    self.push(&format!("\n\n[{}]\n", outcome.label()));
                    self.assistant_active = false;
                    if let pablo_core::RunOutcome::PolicyDenied { rule } = outcome {
                        self.policy = label(&format!("Policy denied: {rule:?}"));
                    }
                } else {
                    self.push(&format!("\n[child {}]\n", outcome.label()));
                }
            }
            _ => {}
        }
    }
    fn child(&mut self, agent: &serde_json::Value) {
        if let Some(id) = agent["agent_id"].as_str() {
            let id = label(id);
            let state = agent["state"].as_str().unwrap_or("admitted");
            if state == "settled" {
                self.activity.remove(&id);
            } else {
                self.active(&id, &format!("Child {}", label(state)));
            }
        }
    }
    fn active(&mut self, id: &str, status: &str) {
        if self.activity.len() < 17 || self.activity.contains_key(id) {
            self.activity.insert(id.into(), label(status));
        }
    }
}
fn number(value: Option<u64>) -> String {
    value
        .map(|n| n.to_string())
        .unwrap_or_else(|| "unknown".into())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn untrusted_text_cannot_emit_terminal_or_bidi_controls_and_stays_bounded() {
        let mut view = View::default();
        view.push("\x1b]52;c;secret\x07\u{009b}2J\u{202e}name\n");
        assert!(!view.transcript.chars().any(|c| c.is_control() && c != '\n'));
        assert!(!view.transcript.contains('\u{202e}'));
        view.push(&"🦀".repeat(40_000));
        assert!(view.transcript.len() <= TRANSCRIPT_BYTES && view.trimmed);
        assert!(view.transcript.ends_with('🦀'));
        assert!(label(&"界".repeat(500)).len() <= LABEL_BYTES);
    }
    #[test]
    fn a_fresh_task_preserves_history_but_resets_execution_display() {
        let mut view = View::default();
        view.push("old output");
        view.active("old", "working");
        view.skills.push("old skill".into());
        view.reset("next task");
        assert!(view.transcript.contains("old output"));
        assert!(view.transcript.contains("You: next task"));
        assert!(view.activity.is_empty());
        assert!(view.skills.is_empty());
        assert_eq!(view.status, "Preparing");
    }
    fn event(kind: EventKind) -> RunEvent {
        let mut value = serde_json::json!({"schema_version":"c1.2","seq":1,"timestamp_unix_micros":1,"run_id":"root","session_id":"session","trace_id":"trace","span_id":"span","parent_span_id":null,"trace_flags":"01"});
        value.as_object_mut().unwrap().extend(
            serde_json::to_value(kind)
                .unwrap()
                .as_object()
                .unwrap()
                .clone(),
        );
        serde_json::from_value(value).unwrap()
    }
    #[test]
    fn native_tools_skills_children_and_usage_project_without_private_content() {
        let mut view = View::default();
        view.event(&event(EventKind::ModelStarted {
            provider: "fixture".into(),
            model: "selected".into(),
        }));
        assert_eq!(view.model, "fixture / selected");
        assert!(view.usage.contains("Calls 1"));
        assert_eq!(view.trace, "trace");
        view.event(&event(EventKind::ToolStarted {
            call: pablo_core::ToolCall {
                id: "call".into(),
                name: "fs.read".into(),
                arguments: serde_json::json!({"path":"PRIVATE_PATH"}),
            },
        }));
        assert!(
            view.activity
                .values()
                .any(|status| status == "Tool fs.read")
        );
        assert!(!view.transcript.contains("PRIVATE_PATH"));
        #[cfg(unix)]
        {
            view.event(&event(EventKind::SkillActivated {
                skill: pablo_core::skills::activation::ActivationRecord {
                    qualified_name: "host/read".into(),
                    root: "host".into(),
                    relative_path: "read".into(),
                    metadata_sha256: "digest".into(),
                    instruction_sha256: "digest".into(),
                    catalog_bytes: 1,
                    instruction_bytes: 1,
                },
                instructions: Some("PRIVATE_SKILL_BODY".into()),
            }));
            assert_eq!(view.skills, ["host/read"]);
            assert!(!view.transcript.contains("PRIVATE_SKILL_BODY"));
        }
        let mut result =
            pablo_core::tool::ToolResult::status(pablo_core::tool::ToolStatus::Completed);
        result.subagent = Some(Box::new(
            serde_json::json!({"agent":{"agent_id":"queued-child","state":"queued"}}),
        ));
        view.event(&event(EventKind::ToolFinished {
            call_id: "call".into(),
            name: "subagent".into(),
            result: result.clone(),
        }));
        assert_eq!(view.activity["queued-child"], "Child queued");
        result.subagent = Some(Box::new(
            serde_json::json!({"snapshot":{"agent":{"agent_id":"queued-child","state":"settled"}}}),
        ));
        view.event(&event(EventKind::ToolFinished {
            call_id: "stop".into(),
            name: "subagent".into(),
            result,
        }));
        assert!(!view.activity.contains_key("queued-child"));
        for i in 0..100 {
            view.active(&i.to_string(), "child");
        }
        assert_eq!(view.activity.len(), 17);
    }
}
