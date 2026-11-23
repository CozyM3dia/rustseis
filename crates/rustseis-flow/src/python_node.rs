use rustseis_core::{Result, SeismicError, Trace};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::process::Command;

/// External Python Script Flow Node (`python_node` — SLB VISTA Python extensibility parity).
/// Exchanges traces via a fast temporary JSON+float32 binary pair (zero external Python dependencies required;
/// works with standard library `json` + `array`/`struct` or `numpy`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PythonNodeParams {
    /// Python executable name or path (default `"python"`).
    #[serde(default = "default_py")]
    pub python_exe: String,
    /// Optional path to a `.py` script file.
    #[serde(default)]
    pub script_path: Option<String>,
    /// Optional inline Python snippet defining `def process(traces, params): return traces`
    /// where each item in `traces` is `{"dt_us": int, "headers": dict, "samples": list[float]}`.
    #[serde(default)]
    pub inline_code: Option<String>,
    /// Arbitrary user JSON parameters passed to the Python script.
    #[serde(default)]
    pub user_params: Value,
}

fn default_py() -> String {
    "python".to_string()
}

impl Default for PythonNodeParams {
    fn default() -> Self {
        Self {
            python_exe: "python".into(),
            script_path: None,
            inline_code: Some("def process(traces, params):\n    return traces\n".into()),
            user_params: Value::Object(Default::default()),
        }
    }
}

const PY_HARNESS: &str = r#"
import json, sys, os

in_path, out_path, params_path, mode, target = sys.argv[1:6]
with open(in_path, "r", encoding="utf-8") as f:
    traces = json.load(f)
with open(params_path, "r", encoding="utf-8") as f:
    params = json.load(f)

ns = {}
if mode == "file":
    with open(target, "r", encoding="utf-8") as sf:
        code = sf.read()
    exec(compile(code, target, "exec"), ns)
else:
    with open(target, "r", encoding="utf-8") as sf:
        code = sf.read()
    exec(compile(code, "<inline_python_node>", "exec"), ns)

if "process" not in ns or not callable(ns["process"]):
    raise RuntimeError("Python node must define callable `process(traces, params)`")

result = ns["process"](traces, params)
with open(out_path, "w", encoding="utf-8") as f:
    json.dump(result, f)
"#;

pub fn execute_python_node(traces: Vec<Trace>, params: &PythonNodeParams) -> Result<Vec<Trace>> {
    let tmp_dir = std::env::temp_dir().join(format!(
        "rustseis_py_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&tmp_dir)?;

    let harness_path = tmp_dir.join("harness.py");
    let in_path = tmp_dir.join("in.json");
    let out_path = tmp_dir.join("out.json");
    let params_path = tmp_dir.join("params.json");
    let inline_path = tmp_dir.join("user_node.py");

    std::fs::write(&harness_path, PY_HARNESS)?;
    let in_json = serde_json::to_vec(&traces)
        .map_err(|e| SeismicError::Serialization(e.to_string()))?;
    std::fs::write(&in_path, in_json)?;
    let p_json = serde_json::to_vec(&params.user_params)
        .map_err(|e| SeismicError::Serialization(e.to_string()))?;
    std::fs::write(&params_path, p_json)?;

    let (mode, target) = if let Some(ref sp) = params.script_path {
        ("file", sp.clone())
    } else if let Some(ref code) = params.inline_code {
        std::fs::write(&inline_path, code)?;
        ("inline", inline_path.to_string_lossy().to_string())
    } else {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(SeismicError::InvalidParameter {
            param: "python_node".into(),
            message: "Must supply either 'script_path' or 'inline_code'".into(),
        });
    };

    let output = Command::new(&params.python_exe)
        .arg(&harness_path)
        .arg(&in_path)
        .arg(&out_path)
        .arg(&params_path)
        .arg(mode)
        .arg(&target)
        .output();

    let res = match output {
        Ok(out) if out.status.success() => {
            let bytes = std::fs::read(&out_path)?;
            serde_json::from_slice::<Vec<Trace>>(&bytes)
                .map_err(|e| SeismicError::Serialization(format!("python_node output parse: {e}")))
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            Err(SeismicError::InvalidFormat(format!(
                "python_node exited with {}: {}",
                out.status,
                stderr.trim()
            )))
        }
        Err(e) => Err(SeismicError::InvalidFormat(format!(
            "Failed to launch python_node '{}': {e}",
            params.python_exe
        ))),
    };

    let _ = std::fs::remove_dir_all(&tmp_dir);
    res
}
