import json
import os
import re
from pathlib import Path

RAW = Path(r"C:\Kuliah\Metode Seismik 2025\rustseis\specs\raw")
GEN = Path(r"C:\Kuliah\Metode Seismik 2025\rustseis\specs\generated")
(GEN / "tools").mkdir(parents=True, exist_ok=True)


def parse_headers():
    headers = []
    with open(RAW / "header.list", encoding="utf-8", errors="ignore") as f:
        for line in f:
            if not line.startswith("CNAME="):
                continue
            name = line[6:15].strip()
            desc = line[15:48].strip()
            rest = line[48:].split()
            length = int(rest[0]) if len(rest) > 0 and rest[0].isdigit() else 1
            fmt = rest[1] if len(rest) > 1 else "IREAL4pz"
            pos = rest[2] if len(rest) > 2 else ""
            rust_type = (
                "i32" if "INT4" in fmt else ("f64" if "IREAL8" in fmt else "f32")
            )
            headers.append(
                {
                    "name": name,
                    "description": desc,
                    "length": length,
                    "promax_format": fmt,
                    "rust_type": rust_type,
                    "ui_priority": pos,
                }
            )
    with open(GEN / "headers.json", "w", encoding="utf-8") as f:
        json.dump({"count": len(headers), "headers": headers}, f, indent=2)
    return headers


def parse_menu(path: Path):
    text = path.read_text(encoding="utf-8", errors="ignore")
    # Remove lisp line comments
    lines = []
    for ln in text.splitlines():
        # strip ; comments outside strings
        in_str = False
        out = []
        for ch in ln:
            if ch == '"':
                in_str = not in_str
            if ch == ";" and not in_str:
                break
            out.append(ch)
        lines.append("".join(out))
    clean = "\n".join(lines)

    name_m = re.search(r'\bname:\s*(\S+)', clean)
    label_m = re.search(r'\blabel:\s*"([^"]*)"', clean)
    tool_name = name_m.group(1) if name_m else path.stem.upper()
    label = label_m.group(1) if label_m else path.stem

    # Split into parameter blocks
    parts = re.split(r'\bparameter:\s*', clean)
    params = []
    for blk in parts[1:]:
        pm = re.match(r'(\S+)', blk)
        if not pm:
            continue
        pname = pm.group(1)
        tm = re.search(r'\btext:\s*"([^"]*)"', blk)
        tym = re.search(r'\btype:\s*(:?[a-zA-Z0-9_-]+:?)', blk)
        vm = re.search(r'\bvalue:\s*("[^"]*"|\S+)', blk)
        mm = re.search(r'\bmouse_text:\s*"([^"]*)"', blk)
        choices = re.findall(r'\(\s*"([^"]+)"\s+([^\s\)]+)\s*(?:"([^"]*)")?\s*\)', blk)
        param = {
            "name": pname,
            "prompt": tm.group(1).strip() if tm else pname,
            "kind": tym.group(1).strip(":") if tym else "unknown",
            "default": vm.group(1).strip('"') if vm else None,
            "help": mm.group(1).strip() if mm else "",
        }
        if choices and param["kind"] in ("circular", "choose"):
            param["choices"] = [
                {"label": c[0].strip(), "value": c[1].strip('"'), "help": c[2].strip()}
                for c in choices
            ]
        params.append(param)

    return {
        "menu_id": path.stem,
        "tool_name": tool_name,
        "label": label,
        "parameter_count": len(params),
        "parameters": params,
    }


def main():
    headers = parse_headers()
    menus = sorted((RAW / "menus").glob("*.menu"))
    index = []
    for mp in menus:
        spec = parse_menu(mp)
        out_path = GEN / "tools" / f"{mp.stem}.json"
        out_path.write_text(json.dumps(spec, indent=2), encoding="utf-8")
        index.append(
            {
                "menu_id": spec["menu_id"],
                "tool_name": spec["tool_name"],
                "label": spec["label"],
                "parameter_count": spec["parameter_count"],
            }
        )
    (GEN / "tier1_index.json").write_text(
        json.dumps({"count": len(index), "tools": index}, indent=2), encoding="utf-8"
    )
    print(f"Generated {len(headers)} headers and {len(index)} tool JSON schemas.")


if __name__ == "__main__":
    main()
