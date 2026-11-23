//! Harvested ProMAX parameter references (Tier 1 JSON + Tier 2 `.menu`) used for inspector
//! tooltips, bounds and choice lists.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default)]
pub struct SpecParam {
    pub name: String,
    pub prompt: String,
    pub kind: String,
    pub default: Option<String>,
    pub help: String,
    pub choices: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default)]
pub struct ToolSpec {
    pub menu_id: String,
    pub label: String,
    pub source: String,
    pub params: Vec<SpecParam>,
}

#[derive(Default)]
pub struct SpecLibrary {
    pub root: Option<PathBuf>,
    by_id: HashMap<String, ToolSpec>,
}

impl SpecLibrary {
    pub fn discover() -> Self {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Ok(p) = std::env::var("RUSTSEIS_SPECS") {
            candidates.push(PathBuf::from(p));
        }
        if let Ok(exe) = std::env::current_exe() {
            for anc in exe.ancestors().skip(1).take(5) {
                candidates.push(anc.join("specs"));
            }
        }
        if let Ok(cwd) = std::env::current_dir() {
            candidates.push(cwd.join("specs"));
            candidates.push(cwd.join("rustseis").join("specs"));
        }
        candidates.push(PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../specs")));
        let root = candidates.into_iter().find(|p| p.join("generated").is_dir() || p.join("raw_tier2").is_dir());
        match root {
            Some(r) => Self::load(&r),
            None => Self::default(),
        }
    }

    pub fn load(root: &Path) -> Self {
        let mut by_id = HashMap::new();
        if let Ok(rd) = std::fs::read_dir(root.join("generated").join("tools")) {
            for e in rd.flatten() {
                if let Some(spec) = std::fs::read_to_string(e.path()).ok().and_then(|t| parse_tier1_json(&t)) {
                    by_id.insert(spec.menu_id.to_ascii_lowercase(), spec);
                }
            }
        }
        if let Ok(rd) = std::fs::read_dir(root.join("raw_tier2").join("menus")) {
            for e in rd.flatten() {
                let path = e.path();
                let stem = path.file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
                if let Ok(text) = std::fs::read(&path) {
                    let text = String::from_utf8_lossy(&text);
                    let spec = parse_menu(&stem, &text);
                    by_id.entry(stem).or_insert(spec);
                }
            }
        }
        Self { root: Some(root.to_path_buf()), by_id }
    }

    pub fn get(&self, menu_id: &str) -> Option<&ToolSpec> {
        let id = menu_id.to_ascii_lowercase();
        self.by_id.get(&id).or_else(|| match id.as_str() {
            "radon_trans" | "taup" => self.by_id.get("radon_trans").or_else(|| self.by_id.get("radon_taup")),
            "trimstat" => self.by_id.get("trimstat").or_else(|| self.by_id.get("timetrimstat")),
            "pwr_stat_new" => self.by_id.get("pwr_stat"),
            _ => None,
        })
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

fn parse_tier1_json(text: &str) -> Option<ToolSpec> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let menu_id = v.get("menu_id")?.as_str()?.to_string();
    let label = v.get("label").and_then(|x| x.as_str()).unwrap_or(&menu_id).to_string();
    let params = v
        .get("parameters")
        .and_then(|p| p.as_array())
        .map(|arr| {
            arr.iter()
                .map(|p| SpecParam {
                    name: p.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    prompt: p.get("prompt").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    kind: p.get("kind").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    default: p.get("default").and_then(|x| x.as_str()).map(|s| s.to_string()),
                    help: p.get("help").and_then(|x| x.as_str()).unwrap_or("").trim().to_string(),
                    choices: Vec::new(),
                })
                .collect()
        })
        .unwrap_or_default();
    Some(ToolSpec { menu_id, label, source: "ProMAX Tier 1 menu".into(), params })
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    let s = s.strip_prefix('"').unwrap_or(s);
    s.strip_suffix('"').unwrap_or(s).to_string()
}

/// Minimal ProMAX `.menu` reader: `parameter:`, `text:`, `type:`, `type_desc:` choices,
/// `value:` and `mouse_text:`.
pub fn parse_menu(stem: &str, text: &str) -> ToolSpec {
    let mut spec = ToolSpec { menu_id: stem.to_string(), label: stem.to_string(), source: "ProMAX Tier 2 menu".into(), params: Vec::new() };
    let mut cur: Option<SpecParam> = None;
    let mut in_choices = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with(';') || line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("label:") {
            if cur.is_none() {
                spec.label = unquote(rest);
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("parameter:") {
            if let Some(p) = cur.take() {
                spec.params.push(p);
            }
            cur = Some(SpecParam { name: rest.trim().to_string(), ..Default::default() });
            in_choices = false;
            continue;
        }
        let Some(p) = cur.as_mut() else { continue };
        if let Some(rest) = line.strip_prefix("text:") {
            p.prompt = unquote(rest);
        } else if let Some(rest) = line.strip_prefix("type:") {
            p.kind = rest.trim().trim_end_matches(':').to_string();
        } else if let Some(rest) = line.strip_prefix("value:") {
            p.default = Some(unquote(rest));
        } else if let Some(rest) = line.strip_prefix("mouse_text:") {
            p.help = unquote(rest);
        } else if let Some(rest) = line.strip_prefix("type_desc:") {
            in_choices = true;
            parse_choice_line(rest, &mut p.choices);
        } else if in_choices && line.starts_with('(') {
            parse_choice_line(line, &mut p.choices);
        } else {
            in_choices = false;
        }
    }
    if let Some(p) = cur.take() {
        spec.params.push(p);
    }
    spec
}

fn parse_choice_line(line: &str, out: &mut Vec<(String, String)>) {
    // ("Label" 1 "description")
    let mut rest = line;
    while let Some(start) = rest.find("(\"") {
        let after = &rest[start + 2..];
        let Some(end_label) = after.find('"') else { break };
        let label = after[..end_label].to_string();
        let tail = &after[end_label + 1..];
        let desc = tail
            .find('"')
            .and_then(|a| tail[a + 1..].find('"').map(|b| tail[a + 1..a + 1 + b].to_string()))
            .unwrap_or_default();
        out.push((label, desc));
        rest = tail;
        if let Some(close) = rest.find(')') {
            rest = &rest[close + 1..];
        } else {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_menu_parameters_and_choices() {
        let text = r#"
'(
 name: FXDECON
label: "F-X Decon"
parameter: FLT_TYPE
text: "TYPE of filter"
type: circular:
type_desc: (
 ("Wiener Levinson"  1 "Apply complex Wiener Levinson prediction filter." )
 ("L2 NORM ADAPTIVE"  2 "Apply LMS L2 norm adaptive prediction filter." ) )
mouse_text: "Prediction filter type."

parameter: PCTN
text: "Percentage of white noise"
type: typein:
value: 0.0
"#;
        let s = parse_menu("fxdecon", text);
        assert_eq!(s.label, "F-X Decon");
        assert_eq!(s.params.len(), 2);
        assert_eq!(s.params[0].choices.len(), 2);
        assert_eq!(s.params[0].choices[1].0, "L2 NORM ADAPTIVE");
        assert_eq!(s.params[1].default.as_deref(), Some("0.0"));
    }

    #[test]
    fn discovers_workspace_specs() {
        let lib = SpecLibrary::discover();
        assert!(lib.len() >= 37, "expected harvested specs, got {}", lib.len());
        assert!(lib.get("agc").is_some());
        assert!(lib.get("fxdecon").is_some());
    }
}
