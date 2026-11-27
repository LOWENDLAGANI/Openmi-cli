use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use openmi_platform::AppPaths;

/// A reusable slash-command skill loaded from `~/.config/openmi/skills/*.md`
/// or `<workspace>/.openmi/skills/*.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDef {
    /// Slash command name without leading `/` (e.g. `"review"`, `"debug"`).
    pub name: String,
    /// One-line description shown in the `/` autocomplete palette.
    pub description: String,
    /// Optional tool allowlist restricting the turn when this skill runs.
    pub allowed_tools: Option<Vec<String>>,
    /// Markdown instruction body expanded into the user prompt.
    pub body: String,
    /// Source file path on disk.
    pub source: PathBuf,
}

impl SkillDef {
    /// Expand this skill with optional user-supplied trailing arguments.
    pub fn expand_prompt(&self, user_args: Option<&str>) -> String {
        match user_args.map(str::trim).filter(|s| !s.is_empty()) {
            Some(args) => format!(
                "[Skill: /{}]\n\n{}\n\nUser request / arguments:\n{}",
                self.name,
                self.body.trim(),
                args
            ),
            None => format!("[Skill: /{}]\n\n{}", self.name, self.body.trim()),
        }
    }
}

/// Load global and project skills (project `.openmi/skills/*.md` overrides global by `name`).
pub fn load_merged_skills(workspace_root: &Path) -> Vec<SkillDef> {
    let mut by_name: BTreeMap<String, SkillDef> = BTreeMap::new();

    if let Ok(paths) = AppPaths::resolve() {
        let global_dir = paths.config_dir.join("skills");
        for skill in load_skills_from_dir(&global_dir) {
            by_name.insert(skill.name.to_ascii_lowercase(), skill);
        }
    }

    let project_dir = workspace_root.join(".openmi").join("skills");
    for skill in load_skills_from_dir(&project_dir) {
        by_name.insert(skill.name.to_ascii_lowercase(), skill);
    }

    by_name.into_values().collect()
}

/// Load all `*.md` skill files from `dir` (silently skips missing directories).
pub fn load_skills_from_dir(dir: &Path) -> Vec<SkillDef> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut skills = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let is_md = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("md"));
        if !is_md {
            continue;
        }
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Some(skill) = parse_skill_markdown(&raw, &path) {
                skills.push(skill);
            }
        }
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// Parse a Markdown skill file with optional YAML frontmatter (`--- ... ---`).
pub fn parse_skill_markdown(raw: &str, source: &Path) -> Option<SkillDef> {
    let default_name = source
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())?;

    let mut name = default_name;
    let mut description = String::new();
    let mut allowed_tools: Option<Vec<String>> = None;
    let body: &str;

    let normalized = raw.replace("\r\n", "\n");
    if let Some(rest) = normalized.strip_prefix("---\n") {
        if let Some(end_idx) = rest.find("\n---\n").or_else(|| rest.find("\n---")) {
            let fm = &rest[..end_idx];
            let after_delim = &rest[end_idx + 4..];
            body = after_delim.strip_prefix('\n').unwrap_or(after_delim);

            let mut in_allowed_list = false;
            for line in fm.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                if in_allowed_list {
                    if let Some(item) = trimmed.strip_prefix("- ") {
                        let tool = unquote(item.trim());
                        if !tool.is_empty() {
                            allowed_tools.get_or_insert_with(Vec::new).push(tool);
                        }
                        continue;
                    } else {
                        in_allowed_list = false;
                    }
                }
                if let Some((k, v)) = trimmed.split_once(':') {
                    let key = k.trim().to_ascii_lowercase();
                    let val = v.trim();
                    match key.as_str() {
                        "name" => {
                            let cleaned = unquote(val).trim_start_matches('/').to_ascii_lowercase();
                            if !cleaned.is_empty() {
                                name = cleaned;
                            }
                        }
                        "description" => {
                            description = unquote(val);
                        }
                        "allowed_tools" | "allowed-tools" => {
                            if val.is_empty() {
                                in_allowed_list = true;
                            } else if val.starts_with('[') && val.ends_with(']') {
                                let inner = &val[1..val.len() - 1];
                                let list: Vec<String> = inner
                                    .split(',')
                                    .map(|s| unquote(s.trim()))
                                    .filter(|s| !s.is_empty())
                                    .collect();
                                if !list.is_empty() {
                                    allowed_tools = Some(list);
                                }
                            } else {
                                let list: Vec<String> = val
                                    .split(',')
                                    .map(|s| unquote(s.trim()))
                                    .filter(|s| !s.is_empty())
                                    .collect();
                                if !list.is_empty() {
                                    allowed_tools = Some(list);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        } else {
            body = normalized.as_str();
        }
    } else {
        body = normalized.as_str();
    }

    let trimmed_body = body.trim().to_string();
    if trimmed_body.is_empty() {
        return None;
    }
    if description.is_empty() {
        description = format!("Run custom skill /{name}");
    }

    Some(SkillDef {
        name,
        description,
        allowed_tools,
        body: trimmed_body,
        source: source.to_path_buf(),
    })
}

fn unquote(s: &str) -> String {
    let t = s.trim();
    if (t.starts_with('"') && t.ends_with('"') && t.len() >= 2)
        || (t.starts_with('\'') && t.ends_with('\'') && t.len() >= 2)
    {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_yaml_frontmatter_and_expands_skill() {
        let md = r#"---
name: review
description: "Review recent git changes for bugs"
allowed_tools: [read_file, grep, glob]
---
Inspect the diff and report any correctness or security issues.
"#;
        let skill = parse_skill_markdown(md, Path::new("review.md")).unwrap();
        assert_eq!(skill.name, "review");
        assert_eq!(skill.description, "Review recent git changes for bugs");
        assert_eq!(
            skill.allowed_tools,
            Some(vec!["read_file".into(), "grep".into(), "glob".into()])
        );
        let expanded = skill.expand_prompt(Some("focus on auth"));
        assert!(expanded.contains("[Skill: /review]"));
        assert!(expanded.contains("focus on auth"));
    }

    #[test]
    fn parses_yaml_list_allowed_tools_from_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let skills_dir = tmp.path().join(".openmi/skills");
        fs::create_dir_all(&skills_dir).unwrap();
        fs::write(
            skills_dir.join("debug.md"),
            "---\ndescription: Debug failing tests\nallowed_tools:\n  - read_file\n  - run_command\n---\nReproduce and fix the failing test.\n",
        )
        .unwrap();

        let loaded = load_skills_from_dir(&skills_dir);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "debug");
        assert_eq!(
            loaded[0].allowed_tools,
            Some(vec!["read_file".into(), "run_command".into()])
        );
    }
}
