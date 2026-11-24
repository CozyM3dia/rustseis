//! Editable flow document. `FlowSpec` stores `flow_if` / `flow_split` children inside params
//! JSON; the editor lifts them into a real tree so they can be shown as nested blocks,
//! selected, reordered and edited, then lowers the tree back to a `FlowSpec` on save / run.

use rustseis_flow::{FlowSpec, FlowStep};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct FlowNode {
    pub uid: u64,
    pub tool: String,
    pub enabled: bool,
    pub label: Option<String>,
    /// Params without the nested step lists (those live in `branches`).
    pub params: Value,
    pub branches: Vec<Vec<FlowNode>>,
}

/// Branch names shown on nested blocks.
pub fn branch_names(tool: &str, n: usize) -> Vec<String> {
    match tool {
        "flow_if" => vec!["THEN".into(), "ELSE".into()],
        _ => (0..n).map(|i| format!("BRANCH {i}")).collect(),
    }
}

pub fn is_container(tool: &str) -> bool {
    matches!(tool, "flow_if" | "flow_split")
}

/// Path to a node: `[step, branch, step, branch, step, …]` (odd length).
pub type StepPath = Vec<usize>;

#[derive(Clone, Debug)]
pub struct FlowDoc {
    pub name: String,
    pub description: String,
    pub nodes: Vec<FlowNode>,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    next_uid: u64,
}

impl Default for FlowDoc {
    fn default() -> Self {
        Self {
            name: "untitled_flow".into(),
            description: String::new(),
            nodes: Vec::new(),
            path: None,
            dirty: false,
            next_uid: 1,
        }
    }
}

impl FlowDoc {
    pub fn from_spec(spec: &FlowSpec, path: Option<PathBuf>) -> Self {
        let mut doc = Self { name: spec.name.clone(), description: spec.description.clone(), path, ..Default::default() };
        let steps = spec.steps.clone();
        doc.nodes = steps.into_iter().map(|s| doc.lift(s)).collect();
        doc
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let is_toml = path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("toml"));
        let spec: FlowSpec = if is_toml {
            toml::from_str(&text).map_err(|e| e.to_string())?
        } else {
            serde_json::from_str(&text).map_err(|e| e.to_string())?
        };
        Ok(Self::from_spec(&spec, Some(path.to_path_buf())))
    }

    pub fn save(&mut self, path: &Path) -> Result<(), String> {
        let spec = self.to_spec();
        let is_toml = path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("toml"));
        let text = if is_toml {
            toml::to_string_pretty(&spec).map_err(|e| e.to_string())?
        } else {
            serde_json::to_string_pretty(&spec).map_err(|e| e.to_string())?
        };
        std::fs::write(path, text).map_err(|e| e.to_string())?;
        self.path = Some(path.to_path_buf());
        self.dirty = false;
        Ok(())
    }

    fn uid(&mut self) -> u64 {
        self.next_uid += 1;
        self.next_uid
    }

    pub fn new_node(&mut self, tool: &str, params: Value) -> FlowNode {
        let step = FlowStep { tool: tool.to_string(), enabled: true, label: None, params };
        self.lift(step)
    }

    fn lift(&mut self, step: FlowStep) -> FlowNode {
        let uid = self.uid();
        let mut params = step.params;
        let tool = step.tool.to_ascii_lowercase();
        let mut branches = Vec::new();
        match tool.as_str() {
            "flow_if" => {
                for key in ["then_steps", "else_steps"] {
                    let raw = params.get_mut(key).map(Value::take).unwrap_or(Value::Null);
                    let steps: Vec<FlowStep> = serde_json::from_value(raw).unwrap_or_default();
                    branches.push(steps.into_iter().map(|s| self.lift(s)).collect());
                }
                if let Some(o) = params.as_object_mut() {
                    o.remove("then_steps");
                    o.remove("else_steps");
                }
            }
            "flow_split" => {
                let raw = params.get_mut("branches").map(Value::take).unwrap_or(Value::Null);
                let bs: Vec<Vec<FlowStep>> = serde_json::from_value(raw).unwrap_or_default();
                for b in bs {
                    branches.push(b.into_iter().map(|s| self.lift(s)).collect());
                }
                if let Some(o) = params.as_object_mut() {
                    o.remove("branches");
                }
            }
            _ => {}
        }
        FlowNode { uid, tool: step.tool, enabled: step.enabled, label: step.label, params, branches }
    }

    pub fn to_spec(&self) -> FlowSpec {
        FlowSpec { name: self.name.clone(), description: self.description.clone(), steps: self.nodes.iter().map(lower).collect() }
    }

    pub fn get(&self, path: &[usize]) -> Option<&FlowNode> {
        let mut list = &self.nodes;
        let mut i = 0;
        loop {
            let node = list.get(*path.get(i)?)?;
            if i + 1 >= path.len() {
                return Some(node);
            }
            list = node.branches.get(path[i + 1])?;
            i += 2;
        }
    }

    pub fn get_mut(&mut self, path: &[usize]) -> Option<&mut FlowNode> {
        let mut list = &mut self.nodes;
        let mut i = 0;
        loop {
            let idx = *path.get(i)?;
            if i + 1 >= path.len() {
                return list.get_mut(idx);
            }
            let b = path[i + 1];
            list = list.get_mut(idx)?.branches.get_mut(b)?;
            i += 2;
        }
    }

    /// The list containing the node at `path` (path minus its last index).
    pub fn container_mut(&mut self, parent: &[usize]) -> Option<&mut Vec<FlowNode>> {
        if parent.is_empty() {
            return Some(&mut self.nodes);
        }
        let (node_path, b) = parent.split_at(parent.len() - 1);
        self.get_mut(node_path)?.branches.get_mut(b[0])
    }

    /// Insert after `after` (same container) or append to the root list.
    pub fn insert_after(&mut self, after: Option<&StepPath>, node: FlowNode) -> StepPath {
        match after {
            Some(p) if !p.is_empty() => {
                let (parent, last) = p.split_at(p.len() - 1);
                let idx = last[0] + 1;
                if let Some(list) = self.container_mut(parent) {
                    let idx = idx.min(list.len());
                    list.insert(idx, node);
                    self.dirty = true;
                    let mut out = parent.to_vec();
                    out.push(idx);
                    return out;
                }
                self.nodes.push(node);
                self.dirty = true;
                vec![self.nodes.len() - 1]
            }
            _ => {
                self.nodes.push(node);
                self.dirty = true;
                vec![self.nodes.len() - 1]
            }
        }
    }

    /// Append into a container branch of the node at `node_path`.
    pub fn insert_into_branch(&mut self, node_path: &[usize], branch: usize, node: FlowNode) -> Option<StepPath> {
        let n = self.get_mut(node_path)?;
        while n.branches.len() <= branch {
            n.branches.push(Vec::new());
        }
        n.branches[branch].push(node);
        let idx = n.branches[branch].len() - 1;
        self.dirty = true;
        let mut out = node_path.to_vec();
        out.push(branch);
        out.push(idx);
        Some(out)
    }

    pub fn remove(&mut self, path: &[usize]) -> Option<FlowNode> {
        let (parent, last) = path.split_at(path.len().checked_sub(1)?);
        let list = self.container_mut(parent)?;
        let removed = (last[0] < list.len()).then(|| list.remove(last[0]));
        if removed.is_some() {
            self.dirty = true;
        }
        removed
    }

    /// Move within the same container. Returns the new path.
    pub fn move_within(&mut self, path: &[usize], to_index: usize) -> Option<StepPath> {
        let (parent, last) = path.split_at(path.len().checked_sub(1)?);
        let list = self.container_mut(parent)?;
        let from = last[0];
        if from >= list.len() {
            return None;
        }
        let node = list.remove(from);
        let to = to_index.min(list.len());
        list.insert(to, node);
        self.dirty = true;
        let mut out = parent.to_vec();
        out.push(to);
        Some(out)
    }

    pub fn duplicate(&mut self, path: &[usize]) -> Option<StepPath> {
        let mut copy = self.get(path)?.clone();
        self.reuid(&mut copy);
        Some(self.insert_after(Some(&path.to_vec()), copy))
    }

    fn reuid(&mut self, n: &mut FlowNode) {
        n.uid = self.uid();
        for b in &mut n.branches {
            for c in b {
                self.reuid(c);
            }
        }
    }

    /// Paths of every node (depth-first, nested branches included) for which `pred` holds.
    pub fn find_paths(&self, pred: impl Fn(&FlowNode) -> bool) -> Vec<StepPath> {
        fn walk(list: &[FlowNode], prefix: &mut StepPath, pred: &dyn Fn(&FlowNode) -> bool, out: &mut Vec<StepPath>) {
            for (i, n) in list.iter().enumerate() {
                prefix.push(i);
                if pred(n) {
                    out.push(prefix.clone());
                }
                for (b, branch) in n.branches.iter().enumerate() {
                    prefix.push(b);
                    walk(branch, prefix, pred, out);
                    prefix.pop();
                }
                prefix.pop();
            }
        }
        let mut out = Vec::new();
        walk(&self.nodes, &mut Vec::new(), &pred, &mut out);
        out
    }

    pub fn count(&self) -> usize {
        fn c(list: &[FlowNode]) -> usize {
            list.iter().map(|n| 1 + n.branches.iter().map(|b| c(b)).sum::<usize>()).sum()
        }
        c(&self.nodes)
    }
}

/// A nested step as JSON without default fields (`enabled: true`, `label: null`).
fn compact(st: &FlowStep) -> serde_json::Value {
    let mut v = serde_json::to_value(st).unwrap_or(json!({}));
    if let Some(o) = v.as_object_mut() {
        if o.get("enabled") == Some(&json!(true)) {
            o.remove("enabled");
        }
        if o.get("label").is_some_and(|l| l.is_null()) {
            o.remove("label");
        }
    }
    v
}

pub fn lower(n: &FlowNode) -> FlowStep {
    let mut params = n.params.clone();
    if !params.is_object() {
        params = json!({});
    }
    match n.tool.to_ascii_lowercase().as_str() {
        "flow_if" => {
            let get = |i: usize| -> Vec<serde_json::Value> { n.branches.get(i).map(|b| b.iter().map(|c| compact(&lower(c))).collect()).unwrap_or_default() };
            params["then_steps"] = serde_json::Value::Array(get(0));
            params["else_steps"] = serde_json::Value::Array(get(1));
        }
        "flow_split" => {
            let bs: Vec<serde_json::Value> = n.branches.iter().map(|b| serde_json::Value::Array(b.iter().map(|c| compact(&lower(c))).collect())).collect();
            params["branches"] = serde_json::Value::Array(bs);
        }
        _ => {}
    }
    FlowStep { tool: n.tool.clone(), enabled: n.enabled, label: n.label.clone(), params }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> FlowSpec {
        serde_json::from_value(json!({
            "name": "t",
            "steps": [
                {"tool": "synth", "params": {}},
                {"tool": "flow_split", "params": {"output_branch": 1, "branches": [
                    [{"tool": "agc", "params": {"operator_ms": 500.0}}],
                    [{"tool": "flow_if", "params": {"header": "CDP", "min": 1, "max": 9,
                        "then_steps": [{"tool": "debias", "params": {}}], "else_steps": []}}]
                ]}},
                {"tool": "stack", "enabled": false, "params": {}}
            ]
        }))
        .unwrap()
    }

    #[test]
    fn nested_round_trip_is_lossless() {
        let spec = sample();
        let doc = FlowDoc::from_spec(&spec, None);
        assert_eq!(doc.count(), 6);
        assert!(doc.nodes[1].params.get("branches").is_none(), "children lifted out of params");
        let back = doc.to_spec();
        assert_eq!(serde_json::to_value(&back).unwrap(), serde_json::to_value(&spec).unwrap());
    }

    #[test]
    fn path_edits_insert_move_remove() {
        let mut doc = FlowDoc::from_spec(&sample(), None);
        let n = doc.get(&[1, 1, 0, 0, 0]).unwrap();
        assert_eq!(n.tool, "debias");
        let added = doc.new_node("agc", json!({}));
        let p = doc.insert_after(Some(&vec![1, 1, 0, 0, 0]), added);
        assert_eq!(p, vec![1, 1, 0, 0, 1]);
        let moved = doc.move_within(&p, 0).unwrap();
        assert_eq!(doc.get(&moved).unwrap().tool, "agc");
        doc.remove(&moved).unwrap();
        assert_eq!(doc.get(&[1, 1, 0, 0, 0]).unwrap().tool, "debias");
        let ins = doc.insert_into_branch(&[1, 1, 0], 1, doc.clone().new_node("tar", json!({}))).unwrap();
        assert_eq!(ins, vec![1, 1, 0, 1, 0]);
        assert!(doc.dirty);
    }

    #[test]
    fn find_paths_reaches_nested_steps() {
        let doc = FlowDoc::from_spec(&sample(), None);
        assert_eq!(doc.find_paths(|n| n.tool == "debias"), vec![vec![1, 1, 0, 0, 0]]);
        assert_eq!(doc.find_paths(|n| n.tool == "stack"), vec![vec![2]]);
        assert!(doc.find_paths(|n| n.tool == "nmo").is_empty());
    }
}
