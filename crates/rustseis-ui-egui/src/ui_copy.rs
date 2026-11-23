//! User-facing copy helpers. Engine descriptions are written for developers and sometimes
//! mention compatibility targets or internal tiers; the interface shows plain descriptions.

/// Phrases removed or rewritten before engine text reaches the screen, in order.
const REWRITES: &[(&str, &str)] = &[
    ("SLB VISTA parity ", ""),
    ("ProMAX/VISTA-compatible ", ""),
    ("ProMAX-compatible ", ""),
    ("ProMAX/VISTA ", ""),
    ("(Tier 1 + Tier 2, including ", "(including "),
    ("Tier 1 & Tier 2 ", ""),
    ("Tier 1 + Tier 2 ", ""),
    ("ProMAX header key", "header"),
    ("ProMAX header", "header"),
    ("ProMAX ", ""),
    ("ProMAX", ""),
    ("VISTA ", ""),
    ("VISTA", ""),
    ("Tier 2 ", ""),
    ("Tier 1 ", ""),
    ("Tier 2", ""),
    ("Tier 1", ""),
];

/// Plain-language version of an engine description.
pub fn clean(text: &str) -> String {
    let mut t = text.to_string();
    for (a, b) in REWRITES {
        if t.contains(a) {
            t = t.replace(a, b);
        }
    }
    while t.contains("  ") {
        t = t.replace("  ", " ");
    }
    t = t.replace("( ", "(").replace(" )", ")").replace("()", "");
    let t = t.trim();
    let mut c = t.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::clean;

    #[test]
    fn strips_compatibility_targets() {
        assert_eq!(clean("Sort traces by primary and optional secondary ProMAX header key."), "Sort traces by primary and optional secondary header.");
        assert_eq!(clean("SLB VISTA parity Parseval-tight multiscale denoising."), "Parseval-tight multiscale denoising.");
        assert_eq!(
            clean("Execute a declarative ProMAX-compatible JSON processing flow (Tier 1 + Tier 2, including IF/SPLIT and Python nodes)."),
            "Execute a declarative JSON processing flow (including IF/SPLIT and Python nodes)."
        );
        assert_eq!(clean("ProMAX growing/constant AGC."), "Growing/constant AGC.");
    }

    #[test]
    fn whole_catalog_is_clean() {
        for t in rustseis_flow::full_tool_catalog() {
            for s in [clean(&t.label), clean(&t.description), clean(&t.category)] {
                for banned in ["ProMAX", "VISTA", "Tier"] {
                    assert!(!s.contains(banned), "{}: {s}", t.menu_id);
                }
            }
        }
    }
}
