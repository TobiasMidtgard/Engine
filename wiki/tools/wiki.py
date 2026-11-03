#!/usr/bin/env python3
"""The player wiki's helper: an mdBook preprocessor and a page checker. Python 3.8+, no packages.

    python3 tools/wiki.py              mdBook runs this as a preprocessor (book.toml)
    python3 tools/wiki.py supports X   answers mdBook: works with every renderer
    python3 tools/wiki.py check        warns about broken links, missing images and empty fields

What it adds to Markdown (src/editing/how-to-add-a-page.md explains it for editors):

    ```infobox            the box at the top right of a page: `Key: value` lines, `## Heading`
    ```meta               the same keys, shown nowhere: picture and summary for cards
    {{draft}} {{stub}}    a banner under the title (or `status: draft` in the infobox)
    {{cards}}             a picture card for every page in this section
    ```cards              cards for the listed pages (paths from src/, one per line)
    {{table: A, B}}       a table of this section's pages with those infobox fields
    {{review}}            every draft and stub page (the editors' to-do list)

A page saved in a section's folder joins the sidebar by itself, after the pages SUMMARY.md lists,
in alphabetical order. A folder with an index.md inside a section becomes a sub-section.
"""

import html
import json
import os
import posixpath
import re
import sys

RESERVED = {"title", "image", "caption", "summary", "status"}
STATUSES = {
    "draft": (
        "Draft",
        "Written from early design notes. Names and details may still change.",
    ),
    "stub": (
        "Stub",
        "This page is only a start. It needs more detail.",
    ),
}
EDIT_GUIDE = "editing/how-to-add-a-page.md"
FENCE = re.compile(r"^ {0,3}(`{3,}|~{3,})\s*([\w-]*)")
DIRECTIVE = re.compile(r"^\{\{\s*([\w-]+)\s*(?::\s*(.*?))?\s*\}\}$")
IMAGE = re.compile(r"!\[[^\]]*\]\(\s*<?([^)\s>]+)>?(?:\s+\"[^\"]*\")?\s*\)")
LINK = re.compile(r"(?<!!)\[([^\]]+)\]\(\s*<?([^)\s>]+)>?\s*\)")
H1 = re.compile(r"^#\s+(.+?)\s*#*\s*$")


# ---------------------------------------------------------------------------------------------
# Small helpers


def esc(text):
    return html.escape(text, quote=True)


def html_path(md_path):
    """creatures/wolf.md -> creatures/wolf.html, keeping any #anchor."""
    path, _, anchor = md_path.partition("#")
    if path.endswith(".md"):
        path = path[:-3] + ".html"
        if posixpath.basename(path) == "README.html":
            path = path[: -len("README.html")] + "index.html"
    return path + ("#" + anchor if anchor else "")


def rel(target, from_page):
    """Link from the page `from_page` (path under src/) to `target` (path under src/)."""
    start = posixpath.dirname(from_page) or "."
    return posixpath.relpath(target, start)


def is_external(url):
    return bool(re.match(r"^[a-z][a-z0-9+.-]*:", url, re.I)) or url.startswith(("/", "#"))


def resolve(url, from_page):
    """A relative link written in `from_page` -> its path under src/ (None if external)."""
    if is_external(url):
        return None
    path = url.split("#", 1)[0].split("?", 1)[0]
    if not path:
        return None
    return posixpath.normpath(posixpath.join(posixpath.dirname(from_page), path))


def image_path(value, from_page):
    """Infobox `image:` values are relative to src/images/ unless they start with ./ or ../."""
    value = value.strip()
    if not value or is_external(value):
        return value or None
    if value.startswith(("./", "../")):
        return resolve(value, from_page)
    return posixpath.normpath(posixpath.join("images", value))


def inline(text, page):
    """Inline Markdown for infobox values and card text: links, bold, italics and code."""
    out = esc(text)

    def link(m):
        label, url = m.group(1), html.unescape(m.group(2))
        if not is_external(url):
            url = html_path(url)
        return '<a href="%s">%s</a>' % (esc(url), label)

    out = re.sub(r"\[([^\]]+)\]\(([^)\s]+)\)", link, out)
    out = re.sub(r"`([^`]+)`", r"<code>\1</code>", out)
    out = re.sub(r"\*\*(.+?)\*\*", r"<strong>\1</strong>", out)
    out = re.sub(r"(?<![\w*])\*(?!\s)(.+?)(?<!\s)\*(?![\w*])", r"<em>\1</em>", out)
    out = re.sub(r"(?<![\w_])_(?!\s)(.+?)(?<!\s)_(?![\w_])", r"<em>\1</em>", out)
    return out


def plain(text):
    """Markdown -> plain text, for card summaries."""
    text = re.sub(r"!\[[^\]]*\]\([^)]*\)", "", text)
    text = re.sub(r"\[([^\]]+)\]\([^)]*\)", r"\1", text)
    text = re.sub(r"<[^>]+>", "", text)
    text = re.sub(r"[*_`]+", "", text)
    return re.sub(r"\s+", " ", text).strip()


def shorten(text, limit=150):
    if len(text) <= limit:
        return text
    cut = text[:limit].rsplit(" ", 1)[0].rstrip(",;:.")
    return cut + "…"


def split_blocks(content):
    """Yield ('fence', info, lines, raw_lines) for top-level fenced blocks and ('line', text) for
    everything else, so nothing inside an ordinary code block is ever touched."""
    lines = content.split("\n")
    i = 0
    while i < len(lines):
        m = FENCE.match(lines[i])
        if not m:
            yield ("line", lines[i])
            i += 1
            continue
        mark, info = m.group(1), m.group(2).lower()
        body, j = [], i + 1
        closed = False
        while j < len(lines):
            close = re.match(r"^ {0,3}(`{3,}|~{3,})\s*$", lines[j])
            if close and close.group(1)[0] == mark[0] and len(close.group(1)) >= len(mark):
                closed = True
                break
            body.append(lines[j])
            j += 1
        raw = lines[i : j + 1] if closed else lines[i:j]
        yield ("fence", info, body, raw)
        i = j + 1


def parse_fields(lines):
    """`Key: value` lines -> (reserved dict, ordered rows). `## Heading` lines become sections."""
    meta, rows = {}, []
    for line in lines:
        s = line.strip()
        if not s or s.startswith("<!--"):
            continue
        if s.startswith("#"):
            rows.append(("section", s.lstrip("#").strip()))
            continue
        key, sep, value = s.partition(":")
        if not sep:
            rows.append(("text", s))
            continue
        key, value = key.strip(), value.strip()
        if key.lower() in RESERVED:
            meta[key.lower()] = value
        else:
            rows.append(("row", key, value))
    return meta, rows


# ---------------------------------------------------------------------------------------------
# Reading the book


def walk(items):
    for item in items:
        if isinstance(item, dict) and "Chapter" in item:
            ch = item["Chapter"]
            yield ch
            yield from walk(ch.get("sub_items", []))


def first_heading(content):
    for line in content.split("\n"):
        m = H1.match(line)
        if m:
            return plain(m.group(1))
    return None


def title_from_file(path):
    name = posixpath.splitext(posixpath.basename(path))[0]
    if name in ("index", "README"):
        name = posixpath.basename(posixpath.dirname(path)) or name
    return name.replace("-", " ").replace("_", " ").capitalize()


def discover(book, src_dir):
    """Pages in a section's folder that SUMMARY.md doesn't list join that section."""
    listed = {ch["source_path"] for ch in walk(book["items"]) if ch.get("source_path")}

    def make_chapter(path, parent):
        with open(os.path.join(src_dir, path), encoding="utf-8") as f:
            content = f.read()
        listed.add(path)
        return {
            "name": first_heading(content) or title_from_file(path),
            "content": content,
            "number": None,
            "sub_items": [],
            "path": path,
            "source_path": path,
            "parent_names": parent["parent_names"] + [parent["name"]],
        }

    def fill(chapter):
        path = chapter.get("source_path") or ""
        if posixpath.basename(path) in ("index.md", "README.md"):
            folder = posixpath.dirname(path)
            full = os.path.join(src_dir, folder)
            found = []
            for name in sorted(os.listdir(full)) if os.path.isdir(full) else []:
                child = posixpath.join(folder, name) if folder else name
                if name.endswith(".md") and name not in ("index.md", "README.md", "SUMMARY.md"):
                    if child not in listed:
                        found.append(make_chapter(child, chapter))
                elif os.path.isdir(os.path.join(full, name)):
                    index = posixpath.join(child, "index.md")
                    if os.path.isfile(os.path.join(src_dir, index)) and index not in listed:
                        found.append(make_chapter(index, chapter))
            found.sort(key=lambda c: c["name"].lower())
            # The sidebar nests chapters by their section number, so new pages continue the
            # parent's numbering (no-section-label in book.toml keeps the numbers hidden).
            if chapter.get("number"):
                start = len(chapter["sub_items"])
                for i, c in enumerate(found):
                    c["number"] = chapter["number"] + [start + i + 1]
            chapter["sub_items"].extend({"Chapter": c} for c in found)
        for item in chapter["sub_items"]:
            if isinstance(item, dict) and "Chapter" in item:
                fill(item["Chapter"])

    for item in book["items"]:
        if isinstance(item, dict) and "Chapter" in item:
            fill(item["Chapter"])


def page_info(chapter):
    """What cards, tables and the review list need to know about a page."""
    path = chapter.get("source_path") or ""
    info = {
        "title": chapter["name"],
        "path": path,
        "image": None,
        "caption": None,
        "summary": None,
        "status": None,
        "fields": {},
        "parents": chapter.get("parent_names", []),
        "children": [
            c["Chapter"]["source_path"]
            for c in chapter.get("sub_items", [])
            if isinstance(c, dict) and "Chapter" in c and c["Chapter"].get("source_path")
        ],
    }
    paragraph, in_paragraph, seen_h1 = [], False, False
    for block in split_blocks(chapter["content"]):
        if block[0] == "fence":
            if block[1] in ("infobox", "meta"):
                meta, rows = parse_fields(block[2])
                for key in ("image", "caption", "summary", "status"):
                    if meta.get(key) and not info[key]:
                        info[key] = meta[key]
                if meta.get("image"):
                    info["image"] = image_path(meta["image"], path)
                if meta.get("title") and block[1] == "infobox":
                    info["title"] = meta["title"]
                for row in rows:
                    if row[0] == "row":
                        info["fields"].setdefault(row[1].lower(), row[2])
            if in_paragraph:
                in_paragraph = False
            continue
        line = block[1]
        s = line.strip()
        d = DIRECTIVE.match(s)
        if d and d.group(1).lower() in STATUSES and not info["status"]:
            info["status"] = d.group(1).lower()
        if not info["image"]:
            m = IMAGE.search(line)
            if m:
                info["image"] = resolve(m.group(1), path) if not is_external(m.group(1)) else m.group(1)
        if H1.match(line):
            seen_h1 = True
            continue
        if info["summary"] is None and not paragraph:
            starts_block = s.startswith(("#", "<", ">", "|", "-", "*", "+", "!", "{{")) or re.match(
                r"^\d+\.", s
            )
            if s and not starts_block and (seen_h1 or not s.startswith("#")):
                paragraph.append(s)
                in_paragraph = True
            continue
        if in_paragraph:
            if s:
                paragraph.append(s)
            else:
                in_paragraph = False
    if info["summary"] is None and paragraph:
        info["summary"] = shorten(plain(" ".join(paragraph)))
    if info["status"]:
        info["status"] = info["status"].strip().lower()
        if info["status"] not in STATUSES:
            info["status"] = None
    return info


# ---------------------------------------------------------------------------------------------
# Rendering the additions


def status_banner(status, note, page):
    label, text = STATUSES[status]
    if note:
        text = note
    guide = rel(html_path(EDIT_GUIDE), page)
    return (
        '<div class="wiki-status wiki-status-%s" role="note"><span class="wiki-status-label">%s</span>'
        '<span class="wiki-status-text">%s <a href="%s">How to edit</a></span></div>'
        % (status, label, inline(text, page), esc(guide))
    )


def pips(value):
    """`2/5` -> five pips with two filled, for danger and difficulty ratings."""
    m = re.match(r"^\s*(\d)\s*/\s*(\d)\s*(.*)$", value)
    if not m or int(m.group(2)) > 10 or int(m.group(1)) > int(m.group(2)):
        return None
    filled, total, rest = int(m.group(1)), int(m.group(2)), m.group(3).strip()
    dots = "".join(
        '<span class="wiki-pip%s"></span>' % (" on" if i < filled else "") for i in range(total)
    )
    label = "%d of %d" % (filled, total)
    tail = (" " + esc(rest.lstrip("-–— ").strip())) if rest else ""
    return '<span class="wiki-pips" title="%s" aria-label="%s">%s</span>%s' % (
        label,
        label,
        dots,
        tail,
    )


def infobox(lines, chapter):
    page = chapter["source_path"]
    meta, rows = parse_fields(lines)
    title = meta.get("title") or chapter["name"]
    out = ['<aside class="infobox">', '<div class="infobox-title">%s</div>' % inline(title, page)]
    img = image_path(meta.get("image", ""), page)
    if img:
        src = img if is_external(img) else rel(img, page)
        out.append(
            '<figure class="infobox-image"><img src="%s" alt="%s" loading="lazy">%s</figure>'
            % (
                esc(src),
                esc(plain(title)),
                "<figcaption>%s</figcaption>" % inline(meta["caption"], page)
                if meta.get("caption")
                else "",
            )
        )
    else:
        out.append(
            '<div class="infobox-image infobox-noimage"><span>No image yet</span></div>'
        )
    out.append('<table class="infobox-rows"><tbody>')
    for row in rows:
        if row[0] == "section":
            out.append('<tr class="infobox-section"><th colspan="2">%s</th></tr>' % inline(row[1], page))
        elif row[0] == "text":
            out.append('<tr><td colspan="2">%s</td></tr>' % inline(row[1], page))
        else:
            value = pips(row[2]) or inline(row[2], page) or '<span class="wiki-muted">Unknown</span>'
            out.append("<tr><th>%s</th><td>%s</td></tr>" % (inline(row[1], page), value))
    out.append("</tbody></table></aside>")
    return "\n".join(out)


def card(info, page):
    href = rel(html_path(info["path"]), page)
    img = info["image"]
    if img:
        src = img if is_external(img) else rel(img, page)
        picture = '<img class="wiki-card-img" src="%s" alt="" loading="lazy">' % esc(src)
    else:
        initial = (plain(info["title"])[:1] or "?").upper()
        picture = '<span class="wiki-card-img wiki-card-noimg" aria-hidden="true">%s</span>' % esc(
            initial
        )
    badge = ""
    if info["status"]:
        badge = '<span class="wiki-badge wiki-card-badge wiki-badge-%s">%s</span>' % (
            info["status"],
            STATUSES[info["status"]][0],
        )
    summary = '<span class="wiki-card-text">%s</span>' % esc(info["summary"]) if info["summary"] else ""
    return (
        '<a class="wiki-card" href="%s">%s%s<span class="wiki-card-body">'
        '<span class="wiki-card-title">%s</span>%s</span></a>'
        % (esc(href), picture, badge, esc(plain(info["title"])), summary)
    )


def cards(paths, pages, page):
    found = [pages[p] for p in paths if p in pages]
    if not found:
        return '<p class="wiki-muted">No pages here yet.</p>'
    return '<div class="wiki-cards">\n%s\n</div>' % "\n".join(card(i, page) for i in found)


def table(columns, info, pages, page):
    rows = [pages[p] for p in info["children"] if p in pages]
    if not rows:
        return '<p class="wiki-muted">No pages here yet.</p>'
    head = "".join("<th>%s</th>" % esc(c) for c in ["Name"] + columns)
    body = []
    for r in rows:
        cells = ['<td><a href="%s">%s</a></td>' % (esc(rel(html_path(r["path"]), page)), esc(plain(r["title"])))]
        for c in columns:
            v = r["fields"].get(c.lower(), "")
            cells.append("<td>%s</td>" % (pips(v) or inline(v, page) or "—"))
        body.append("<tr>%s</tr>" % "".join(cells))
    return '<div class="wiki-table"><table><thead><tr>%s</tr></thead><tbody>%s</tbody></table></div>' % (
        head,
        "".join(body),
    )


def review_list(pages, page):
    out = []
    for status in ("draft", "stub"):
        items = sorted(
            (i for i in pages.values() if i["status"] == status),
            key=lambda i: (" / ".join(i["parents"]), i["title"].lower()),
        )
        out.append("<h2>%ss <span class=\"wiki-muted\">(%d)</span></h2>" % (STATUSES[status][0], len(items)))
        if not items:
            out.append('<p class="wiki-muted">None.</p>')
            continue
        out.append('<ul class="wiki-review">')
        for i in items:
            where = " / ".join(i["parents"])
            out.append(
                '<li><a href="%s">%s</a>%s</li>'
                % (
                    esc(rel(html_path(i["path"]), page)),
                    esc(plain(i["title"])),
                    ' <span class="wiki-muted">in %s</span>' % esc(where) if where else "",
                )
            )
        out.append("</ul>")
    return "\n".join(out)


def transform(chapter, pages):
    page = chapter["source_path"]
    info = pages[page]
    out, banner_done, pending_banner = [], False, None
    explicit = any(
        b[0] == "line"
        and DIRECTIVE.match(b[1].strip())
        and DIRECTIVE.match(b[1].strip()).group(1).lower() in STATUSES
        for b in split_blocks(chapter["content"])
    )
    if info["status"] and not explicit:
        pending_banner = status_banner(info["status"], None, page)
    h1_seen = False
    for block in split_blocks(chapter["content"]):
        if block[0] == "fence":
            kind, body, raw = block[1], block[2], block[3]
            if kind == "infobox":
                out.extend(["", infobox(body, chapter), ""])
            elif kind == "meta":
                pass
            elif kind == "cards":
                paths = [l.strip() for l in body if l.strip() and not l.strip().startswith("#")]
                out.extend(["", cards(paths, pages, page), ""])
            else:
                out.extend(raw)
            continue
        line = block[1]
        d = DIRECTIVE.match(line.strip())
        if d:
            name, arg = d.group(1).lower(), d.group(2)
            if name in STATUSES:
                out.extend(["", status_banner(name, arg, page), ""])
                banner_done = True
                continue
            if name == "cards":
                out.extend(["", cards(info["children"], pages, page), ""])
                continue
            if name == "table":
                cols = [c.strip() for c in (arg or "").split(",") if c.strip()]
                out.extend(["", table(cols, info, pages, page), ""])
                continue
            if name == "review":
                out.extend(["", review_list(pages, page), ""])
                continue
        out.append(line)
        if pending_banner and not banner_done and not h1_seen and H1.match(line):
            h1_seen = True
            out.extend(["", pending_banner, ""])
            banner_done = True
    if pending_banner and not banner_done:
        out[:0] = [pending_banner, ""]
    chapter["content"] = "\n".join(out)


def preprocess():
    context, book = json.load(sys.stdin)
    root = context["root"]
    src = context["config"].get("book", {}).get("src", "src")
    discover(book, os.path.join(root, src))
    pages = {}
    for ch in walk(book["items"]):
        if ch.get("source_path"):
            pages[ch["source_path"]] = page_info(ch)
    for ch in walk(book["items"]):
        if ch.get("source_path"):
            transform(ch, pages)
    json.dump(book, sys.stdout)


# ---------------------------------------------------------------------------------------------
# Checking pages (warnings only: a broken link never stops the wiki from publishing)


def check():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    src = os.path.join(root, "src")
    warnings = []
    in_actions = os.environ.get("GITHUB_ACTIONS") == "true"

    def warn(path, line, message):
        warnings.append((path, line, message))

    for folder, _, files in os.walk(src):
        for name in files:
            if not name.endswith(".md"):
                continue
            full = os.path.join(folder, name)
            page = os.path.relpath(full, src).replace(os.sep, "/")
            with open(full, encoding="utf-8") as f:
                content = f.read()
            n = 0
            for block in split_blocks(content):
                if block[0] == "fence":
                    if block[1] in ("infobox", "meta"):
                        meta, _ = parse_fields(block[2])
                        img = image_path(meta.get("image", ""), page)
                        if img and not is_external(img) and not os.path.isfile(os.path.join(src, img)):
                            warn(page, n + 1, "infobox image not found: src/%s" % img)
                        if meta.get("status") and meta["status"].lower() not in STATUSES:
                            warn(page, n + 1, "unknown status %r (use draft or stub)" % meta["status"])
                    if block[1] == "cards":
                        for l in block[2]:
                            p = l.strip()
                            if p and not p.startswith("#") and not os.path.isfile(os.path.join(src, p)):
                                warn(page, n + 1, "cards: no page at src/%s" % p)
                    n += len(block[3])
                    continue
                n += 1
                for m in IMAGE.finditer(block[1]):
                    target = resolve(m.group(1), page)
                    if target and not os.path.isfile(os.path.join(src, target)):
                        warn(page, n, "image not found: %s" % m.group(1))
                for m in LINK.finditer(block[1]):
                    target = resolve(m.group(2), page)
                    if target and not os.path.exists(os.path.join(src, target)):
                        warn(page, n, "broken link: %s" % m.group(2))
    for path, line, message in warnings:
        if in_actions:
            print("::warning file=wiki/src/%s,line=%d::%s" % (path, line, message))
        else:
            print("wiki/src/%s:%d: %s" % (path, line, message))
    print("wiki check: %d warning(s)" % len(warnings))


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "supports":
        sys.exit(0)
    if len(sys.argv) > 1 and sys.argv[1] == "check":
        check()
        sys.exit(0)
    preprocess()
