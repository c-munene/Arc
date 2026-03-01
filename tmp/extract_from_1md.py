from __future__ import annotations

import os
import re
from pathlib import Path

ROOT = Path(r"C:\Users\Shuakami_Projects\CommunityProject\Arc")
SRC = ROOT / "1.md"
HEADER_RE = re.compile(r"^## FILE:\s*(.+?)\s*$")


def trim_blanks(lines: list[str]) -> list[str]:
    while lines and lines[0].strip() == "":
        lines.pop(0)
    while lines and lines[-1].strip() == "":
        lines.pop()
    return lines


def parse_sections(lines: list[str]) -> list[tuple[str, str]]:
    headers: list[tuple[int, str]] = []
    for i, line in enumerate(lines):
        m = HEADER_RE.match(line)
        if m:
            headers.append((i, m.group(1).strip()))

    if not headers:
        raise RuntimeError("未找到任何 ## FILE: 分段")

    out: list[tuple[str, str]] = []

    for idx, (line_no, rel_path) in enumerate(headers):
        section_start = line_no + 1
        section_end = headers[idx + 1][0] if idx + 1 < len(headers) else len(lines)
        chunk = lines[section_start:section_end]
        chunk = trim_blanks(chunk)

        # Drop trailing markdown separator between sections if present.
        while chunk and chunk[-1].strip() == "---":
            chunk.pop()
        chunk = trim_blanks(chunk)

        ext = Path(rel_path).suffix.lower()

        if ext == ".md":
            # For markdown targets we keep the whole section body, only peel one wrapper fence if it
            # exists at both boundaries.
            body = chunk[:]
            if body and body[0].startswith("```"):
                body = body[1:]
                body = trim_blanks(body)
            if body and body[-1].strip() == "```":
                body = body[:-1]
                body = trim_blanks(body)
            content_lines = body
        else:
            # For non-markdown files, take content inside the first fenced block only.
            open_idx = None
            for i, l in enumerate(chunk):
                if l.startswith("```"):
                    open_idx = i
                    break
            if open_idx is None:
                raise RuntimeError(f"分段缺少起始代码块: {rel_path}")

            close_idx = None
            for i in range(open_idx + 1, len(chunk)):
                if chunk[i].strip() == "```":
                    close_idx = i
                    break
            if close_idx is None:
                raise RuntimeError(f"分段缺少结束代码块: {rel_path}")

            content_lines = chunk[open_idx + 1 : close_idx]

        content = "\n".join(content_lines)
        if content and not content.endswith("\n"):
            content += "\n"

        out.append((rel_path.replace("/", os.sep), content))

    return out


def ensure_under_root(path: Path, root: Path) -> None:
    root_resolved = root.resolve()
    path_resolved = path.resolve()
    try:
        path_resolved.relative_to(root_resolved)
    except ValueError as exc:
        raise RuntimeError(f"越界路径: {path}") from exc


def main() -> int:
    lines = SRC.read_text(encoding="utf-8").splitlines()
    sections = parse_sections(lines)

    created = 0
    updated = 0
    unchanged = 0

    for rel, content in sections:
        dst = ROOT / rel
        ensure_under_root(dst, ROOT)
        dst.parent.mkdir(parents=True, exist_ok=True)

        old = dst.read_text(encoding="utf-8") if dst.exists() else None
        dst.write_text(content, encoding="utf-8", newline="\n")
        new = dst.read_text(encoding="utf-8")
        if new != content:
            raise RuntimeError(f"回读校验失败: {rel}")

        if old is None:
            created += 1
        elif old == new:
            unchanged += 1
        else:
            updated += 1

    print(f"SOURCE={SRC}")
    print(f"SECTIONS={len(sections)}")
    print(f"CREATED={created}")
    print(f"UPDATED={updated}")
    print(f"UNCHANGED={unchanged}")
    print("FILES_BEGIN")
    for rel, _ in sections:
        print(rel)
    print("FILES_END")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
