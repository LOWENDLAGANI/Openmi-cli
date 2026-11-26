use std::fs;
use std::path::Path;

use openmi_checkpoints::CheckpointStore;
use similar::TextDiff;

/// Render a unified diff for all files modified in the latest checkpoint run relative to `cwd`.
pub fn render_latest_checkpoint_diff(checkpoints: &CheckpointStore, cwd: &Path) -> String {
    let Some(cp) = checkpoints.checkpoints().last() else {
        return "No checkpoints recorded yet.".to_string();
    };
    if cp.originals.is_empty() {
        return "No file changes recorded in the latest run.".to_string();
    }

    let mut paths: Vec<_> = cp.originals.keys().collect();
    paths.sort();

    let mut sections = Vec::new();
    for path in paths {
        let orig_bytes = cp.originals.get(path).and_then(|o| o.as_deref());
        let orig_text = orig_bytes
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default();

        let curr_text = fs::read(path)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();

        if orig_text == curr_text {
            continue;
        }

        let rel = path.strip_prefix(cwd).unwrap_or(path);
        let rel_str = rel.display().to_string();
        let diff = TextDiff::from_lines(&orig_text, &curr_text);
        let unified = diff
            .unified_diff()
            .context_radius(3)
            .header(&format!("a/{rel_str}"), &format!("b/{rel_str}"))
            .to_string();
        sections.push(unified);
    }

    if sections.is_empty() {
        "No differences from checkpoint baseline.".to_string()
    } else {
        sections.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_unified_diff_from_checkpoint_originals() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("hello.rs");
        fs::write(&file, "fn main() {\n    println!(\"old\");\n}\n").unwrap();

        let mut store = CheckpointStore::new();
        store.begin_run();
        store.snapshot_before_write(&file).unwrap();

        fs::write(&file, "fn main() {\n    println!(\"new\");\n}\n").unwrap();

        let diff = render_latest_checkpoint_diff(&store, tmp.path());
        assert!(diff.contains("--- a/hello.rs"));
        assert!(diff.contains("+++ b/hello.rs"));
        assert!(diff.contains("-    println!(\"old\");"));
        assert!(diff.contains("+    println!(\"new\");"));
    }
}
