use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Permission modes governing tool execution and write access.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    ReadOnly,
    #[default]
    Ask,
    Trusted,
    FullAccess,
}

impl FromStr for PermissionMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "read_only" | "readonly" | "read-only" => Ok(Self::ReadOnly),
            "ask" => Ok(Self::Ask),
            "trusted" => Ok(Self::Trusted),
            "full_access" | "fullaccess" | "full-access" => Ok(Self::FullAccess),
            other => Err(format!("unknown permission mode: {other}")),
        }
    }
}

/// Request presented to an `ApprovalHandler` when a tool call requires user consent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequest {
    pub call_id: String,
    pub tool_name: String,
    pub summary: String,
}

/// Decision returned by an `ApprovalHandler`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approve,
    ApproveAlways,
    Deny,
}

/// UI-provided handler for interactive or non-interactive tool approvals.
#[async_trait]
pub trait ApprovalHandler: Send + Sync {
    async fn request_approval(&self, req: ApprovalRequest) -> ApprovalDecision;
}

/// Default non-interactive handler that denies all approval prompts.
pub struct DenyAllApprovalHandler;

#[async_trait]
impl ApprovalHandler for DenyAllApprovalHandler {
    async fn request_approval(&self, _req: ApprovalRequest) -> ApprovalDecision {
        ApprovalDecision::Deny
    }
}

/// Interactive clarification request presented to the user via `ask_user`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskUserRequest {
    pub call_id: String,
    pub question: String,
    pub options: Vec<String>,
    pub allow_custom: bool,
}

/// UI-provided handler for interactive `ask_user` prompts.
#[async_trait]
pub trait AskUserHandler: Send + Sync {
    /// Prompt the user and return their selected/typed answer, or `None` if cancelled/unavailable.
    async fn ask_user(&self, req: AskUserRequest) -> Option<String>;
}

/// Default non-interactive handler for headless environments without a TTY.
pub struct NonInteractiveAskUserHandler;

#[async_trait]
impl AskUserHandler for NonInteractiveAskUserHandler {
    async fn ask_user(&self, _req: AskUserRequest) -> Option<String> {
        None
    }
}

/// Granular glob-based permission rules loaded from global and project `settings.json`.
///
/// Rule format examples:
/// - `"Bash(git status *)"` / `"Bash(cargo test *)"` / `"Bash(rm -rf *)"`
/// - `"Edit(.env*)"` / `"Write(src/**)"` / `"Read(*.rs)"`
/// - `"delete_file"` / `"mcp__fs__*"`
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRules {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub ask: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
}

impl PermissionRules {
    pub fn is_empty(&self) -> bool {
        self.allow.is_empty() && self.ask.is_empty() && self.deny.is_empty()
    }

    /// Merge project-level rules onto global rules (`deny` > protected > `ask` > `allow`).
    pub fn merge(&self, other: &PermissionRules) -> PermissionRules {
        let mut merged = self.clone();
        for r in &other.allow {
            if !merged.allow.contains(r) {
                merged.allow.push(r.clone());
            }
        }
        for r in &other.ask {
            if !merged.ask.contains(r) {
                merged.ask.push(r.clone());
            }
        }
        for r in &other.deny {
            if !merged.deny.contains(r) {
                merged.deny.push(r.clone());
            }
        }
        merged
    }
}

/// Final decision from evaluating permission rules + mode for a single tool invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionOutcome {
    Allow,
    Ask,
    Deny(String),
}

/// Normalize a workspace-relative path for consistent glob matching.
fn normalize_rel_path(path: &str) -> String {
    let s = path.trim().replace('\\', "/");
    s.strip_prefix("./").unwrap_or(&s).to_string()
}

/// Returns `true` if `path` points to a sensitive file that must never be silently modified
/// even in `Trusted` or `FullAccess` modes.
pub fn is_sensitive_path(path: &str) -> bool {
    let norm = normalize_rel_path(path);
    if norm.is_empty() {
        return false;
    }
    let lower = norm.to_ascii_lowercase();
    let basename = lower.rsplit('/').next().unwrap_or(&lower);

    // Protect .env and .env.*
    if basename == ".env" || basename.starts_with(".env.") {
        return true;
    }
    // Protect .git/hooks and anything inside it
    if lower == ".git/hooks"
        || lower.starts_with(".git/hooks/")
        || lower.contains("/.git/hooks/")
        || lower.ends_with("/.git/hooks")
    {
        return true;
    }
    // Protect .openmi/settings.json
    if lower == ".openmi/settings.json" || lower.ends_with("/.openmi/settings.json") {
        return true;
    }
    false
}

/// Extract target paths or command strings from tool arguments JSON.
fn extract_tool_targets(tool_name: &str, args_json: &str) -> Vec<String> {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(args_json) else {
        return Vec::new();
    };
    match tool_name {
        "run_command" => val
            .get("command")
            .and_then(|v| v.as_str())
            .map(|s| vec![s.trim().to_string()])
            .unwrap_or_default(),
        "move_file" => {
            let mut targets = Vec::new();
            if let Some(from) = val.get("from").and_then(|v| v.as_str()) {
                targets.push(normalize_rel_path(from));
            }
            if let Some(to) = val.get("to").and_then(|v| v.as_str()) {
                targets.push(normalize_rel_path(to));
            }
            targets
        }
        "read_file" | "edit_file" | "write_file" | "delete_file" | "list_dir" | "grep" | "glob" => {
            val.get("path")
                .and_then(|v| v.as_str())
                .map(|s| vec![normalize_rel_path(s)])
                .unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

fn rule_tool_matches(rule_tool: &str, tool_name: &str) -> bool {
    match rule_tool.to_ascii_lowercase().as_str() {
        "bash" | "shell" | "run_command" => tool_name == "run_command",
        "edit" => matches!(
            tool_name,
            "edit_file" | "write_file" | "delete_file" | "move_file"
        ),
        "edit_file" => tool_name == "edit_file",
        "write" | "write_file" => tool_name == "write_file",
        "delete" | "delete_file" => tool_name == "delete_file",
        "move" | "move_file" => tool_name == "move_file",
        "read" | "read_file" => tool_name == "read_file",
        _ => crate::tools::glob_match(rule_tool, tool_name),
    }
}

/// Match a simple wildcard pattern (`*` matches any characters including `/`, `?` matches one char).
fn command_wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star_p, mut star_t) = (None, 0usize);

    while ti < t.len() {
        if pi < p.len() && p[pi] == '*' {
            while pi < p.len() && p[pi] == '*' {
                pi += 1;
            }
            star_p = Some(pi);
            star_t = ti;
        } else if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if let Some(sp) = star_p {
            star_t += 1;
            ti = star_t;
            pi = sp;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Match a single rule string (e.g. `"Bash(git status *)"` or `"Edit(.env*)"` or `"delete_file"`)
/// against `tool_name` and its extracted `targets`.
pub fn matches_permission_rule(rule: &str, tool_name: &str, args_json: &str) -> bool {
    let trimmed = rule.trim();
    if trimmed.is_empty() {
        return false;
    }

    if let Some(open_idx) = trimmed.find('(') {
        if let Some(close_idx) = trimmed.rfind(')') {
            if close_idx > open_idx {
                let rule_tool = trimmed[..open_idx].trim();
                let pattern = trimmed[open_idx + 1..close_idx].trim();
                if !rule_tool_matches(rule_tool, tool_name) {
                    return false;
                }
                if pattern == "*" || pattern.is_empty() {
                    return true;
                }
                let targets = extract_tool_targets(tool_name, args_json);
                return targets.iter().any(|t| {
                    if tool_name == "run_command" {
                        command_wildcard_match(pattern, t)
                    } else {
                        let base = t.rsplit('/').next().unwrap_or(t);
                        crate::tools::glob_match(pattern, t)
                            || crate::tools::glob_match(pattern, base)
                    }
                });
            }
        }
    }

    rule_tool_matches(trimmed, tool_name)
}

/// Evaluate granular permission rules, sensitive-path protection, and `PermissionMode`
/// for a built-in tool invocation.
///
/// Precedence:
/// 1. Explicit `deny` rules -> `Deny`
/// 2. Sensitive path protection on mutating file tools -> `Deny` (in ReadOnly) or `Ask`
/// 3. Explicit `ask` rules -> `Deny` (in ReadOnly if mutating) or `Ask`
/// 4. Explicit `allow` rules -> `Deny` (in ReadOnly if mutating) or `Allow`
/// 5. Fallback to default `needs_approval(tool_name, mode)`
pub fn evaluate_permission(
    tool_name: &str,
    args_json: &str,
    mode: PermissionMode,
    rules: &PermissionRules,
) -> PermissionOutcome {
    // 1. Deny rules always take top precedence.
    for rule in &rules.deny {
        if matches_permission_rule(rule, tool_name, args_json) {
            return PermissionOutcome::Deny(format!(
                "tool `{tool_name}` blocked by permission deny rule `{rule}`"
            ));
        }
    }

    let risk = crate::tools::classify_tool_risk(tool_name);
    let is_mutating_file_tool = matches!(
        tool_name,
        "edit_file" | "write_file" | "delete_file" | "move_file"
    );

    // 2. Protect sensitive paths (.git/hooks, .env*, .openmi/settings.json) from silent edits.
    if is_mutating_file_tool {
        let targets = extract_tool_targets(tool_name, args_json);
        if let Some(sensitive) = targets.iter().find(|t| is_sensitive_path(t)) {
            if mode == PermissionMode::ReadOnly {
                return PermissionOutcome::Deny(format!(
                    "tool `{tool_name}` on sensitive path `{sensitive}` is blocked in ReadOnly mode"
                ));
            }
            return PermissionOutcome::Ask;
        }
    }

    // 3. Explicit ask rules.
    for rule in &rules.ask {
        if matches_permission_rule(rule, tool_name, args_json) {
            if risk == crate::tools::ToolRisk::Mutating && mode == PermissionMode::ReadOnly {
                return PermissionOutcome::Deny(format!(
                    "tool `{tool_name}` is blocked in ReadOnly permission mode"
                ));
            }
            return PermissionOutcome::Ask;
        }
    }

    // 4. Explicit allow rules.
    for rule in &rules.allow {
        if matches_permission_rule(rule, tool_name, args_json) {
            if risk == crate::tools::ToolRisk::Mutating && mode == PermissionMode::ReadOnly {
                return PermissionOutcome::Deny(format!(
                    "tool `{tool_name}` is blocked in ReadOnly permission mode"
                ));
            }
            return PermissionOutcome::Allow;
        }
    }

    // 5. Fallback to mode-based risk policy.
    match crate::tools::needs_approval(tool_name, mode) {
        Ok(true) => PermissionOutcome::Ask,
        Ok(false) => PermissionOutcome::Allow,
        Err(msg) => PermissionOutcome::Deny(msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_permission_modes() {
        assert_eq!(
            "read-only".parse::<PermissionMode>().unwrap(),
            PermissionMode::ReadOnly
        );
        assert_eq!(
            "ask".parse::<PermissionMode>().unwrap(),
            PermissionMode::Ask
        );
        assert_eq!(
            "trusted".parse::<PermissionMode>().unwrap(),
            PermissionMode::Trusted
        );
        assert_eq!(
            "full-access".parse::<PermissionMode>().unwrap(),
            PermissionMode::FullAccess
        );
        assert!("invalid".parse::<PermissionMode>().is_err());
    }

    #[test]
    fn granular_rules_allow_ask_deny_and_sensitive_paths() {
        let rules = PermissionRules {
            allow: vec![
                "Bash(git status*)".into(),
                "Bash(cargo test *)".into(),
            ],
            ask: vec!["Bash(git push*)".into()],
            deny: vec!["Bash(rm -rf *)".into(), "Edit(.env*)".into()],
        };

        // Allow rule overrides Ask mode
        assert_eq!(
            evaluate_permission(
                "run_command",
                r#"{"command":"git status -s"}"#,
                PermissionMode::Ask,
                &rules
            ),
            PermissionOutcome::Allow
        );

        // Ask rule overrides Trusted mode
        assert_eq!(
            evaluate_permission(
                "run_command",
                r#"{"command":"git push origin main"}"#,
                PermissionMode::Trusted,
                &rules
            ),
            PermissionOutcome::Ask
        );

        // Deny rule overrides FullAccess mode
        assert!(matches!(
            evaluate_permission(
                "run_command",
                r#"{"command":"rm -rf /"}"#,
                PermissionMode::FullAccess,
                &rules
            ),
            PermissionOutcome::Deny(_)
        ));

        // Edit(.env*) deny rule blocks write_file too
        assert!(matches!(
            evaluate_permission(
                "write_file",
                r#"{"path":".env.local","content":"SECRET=1"}"#,
                PermissionMode::FullAccess,
                &rules
            ),
            PermissionOutcome::Deny(_)
        ));

        // Sensitive paths force Ask even in Trusted/FullAccess with empty rules
        let empty = PermissionRules::default();
        assert_eq!(
            evaluate_permission(
                "write_file",
                r#"{"path":".git/hooks/pre-commit","content":"echo hi"}"#,
                PermissionMode::FullAccess,
                &empty
            ),
            PermissionOutcome::Ask
        );
        assert_eq!(
            evaluate_permission(
                "edit_file",
                r#"{"path":".openmi/settings.json","old_string":"a","new_string":"b"}"#,
                PermissionMode::Trusted,
                &empty
            ),
            PermissionOutcome::Ask
        );
        assert_eq!(
            evaluate_permission(
                "move_file",
                r#"{"from":"foo.txt","to":".env"}"#,
                PermissionMode::Trusted,
                &empty
            ),
            PermissionOutcome::Ask
        );
    }
}
