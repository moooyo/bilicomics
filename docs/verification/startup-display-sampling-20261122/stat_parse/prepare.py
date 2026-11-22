"""Generate a Linux-only parser experiment from the shipping source, without editing it."""

import argparse
from pathlib import Path
import re
import shutil
import tomllib


HERE = Path(__file__).resolve().parent


def between(source, start, stop):
    if source.count(start) != 1 or source.count(stop) != 1 or source.index(start) >= source.index(stop):
        raise ValueError("Shipping parser source boundary is no longer unique")
    return source[source.index(start):source.index(stop)].rstrip()


def main():
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("source_root", type=Path)
    cli.add_argument("output", type=Path)
    args = cli.parse_args()
    root = args.source_root
    writer = (root / "src/native_presentation/portrait_writer.rs").read_text()
    presentation = (root / "src/native_presentation.rs").read_text()
    pid = between(presentation, "fn parse_pid(value: &str)", "pub(crate) fn single_record(")
    limit = re.findall(r"^const MAX_STATUS_BYTES: usize = .*;$", presentation, flags=re.MULTILINE)
    if len(limit) != 1:
        raise ValueError("Missing unique shipping input bound")
    begin = "        ensure!(\n            bytes.len() <= MAX_STATUS_BYTES,"
    end = '        let executable = fs::read_link(path.join("exe"))?;'
    control = between(writer, begin, end).replace("from_utf8(&bytes)", "from_utf8(bytes)")
    field_begin = "        let fields = tail.split_ascii_whitespace().collect::<Vec<_>>();"
    field_end = "        let birth = fields[19].parse::<u64>()?;"
    stock_fields = between(control, field_begin, field_end) + "\n" + field_end
    proposal = control.replace(stock_fields, (HERE / "iterator.inc").read_text().rstrip())
    if control == proposal:
        raise ValueError("Expected one isolated field-extraction change")
    declarations = []
    for name, body in (("collect_all", control), ("first_twenty", proposal)):
        declarations.append(f"""#[inline(never)]
pub fn {name}(bytes: &[u8], pid: u32) -> Result<(u32, u32, u32, u64)> {{
{body}
    Ok((parent, group, session, birth))
}}""")
    packages = tomllib.loads((root / "Cargo.lock").read_text())["package"]
    version, = (entry["version"] for entry in packages if entry["name"] == "anyhow")
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        raise ValueError("Expected the pinned application error dependency")
    output = args.output
    output.mkdir(mode=0o700)
    (output / "Cargo.toml").write_text(f'''[package]
name = "capture-stat-parse-study"
version = "0.1.0"
edition = "2024"
publish = false

[dependencies]
anyhow = "={version}"

[profile.release]
lto = "thin"
codegen-units = 1
strip = true
''')
    src = output / "src"
    src.mkdir()
    (src / "parsers.rs").write_text("\n\n".join(("use anyhow::{ensure, Context, Result};", *limit, pid, *declarations)) + "\n")
    shutil.copyfile(HERE / "study.rs", src / "main.rs")


if __name__ == "__main__":
    main()
