#!/usr/bin/env python3
"""The player wiki's helper: an mdBook preprocessor and a page checker. Python 3.8+, no packages.

    python3 tools/wiki.py              mdBook runs this as a preprocessor (book.toml)
    python3 tools/wiki.py supports X   answers mdBook: works with every renderer
    python3 tools/wiki.py check        warns about broken links, missing images and empty fields

What it adds to Markdown (src/editing/how-to-add-a-page.md explains it for editors):

    ```infobox            the "At a glance" panel beside the page: `Key: value` lines, `## Heading`
    ```meta               the same keys, shown nowhere: picture, summary and tags for the title
    {{draft}} {{stub}}    a Draft or Stub chip by the title (or `status: draft` in the infobox)
    {{cards}}             a picture card for every page in this section
    ```cards              cards for the listed pages (paths from src/, one per line)
    ```list               the same pages as rows with an icon
    {{table: A, B}}       a table of this section's pages with those infobox fields
    {{review}}            every draft and stub page (the editors' to-do list)
    {{recent}}            the latest changes to the wiki, from git
    ```flow               how a quest can go: start, paths side by side, join, endings
    ```spoiler            text hidden behind a "Show" cover (also: a `> [!WARNING]` that starts
                          with "Spoilers" hides the rest of its section)
    ```hero ```columns    the home page's big title and link columns

Every page is framed for theme/index.hbs: its title, summary and chips become the title block,
the infobox becomes the panel beside the text, and theme/codex.js adds tabs and the side panel.

A page saved in a section's folder joins the sidebar by itself, after the pages SUMMARY.md lists,
in alphabetical order. A folder with an index.md inside a section becomes a sub-section.
"""

import html
import json
import os
import posixpath
import re
import subprocess
import sys

RESERVED = {"title", "image", "caption", "summary", "status", "tags", "tabs", "facts"}
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


def smart(escaped):
    """Curly quotes and apostrophes in already-escaped text, like smart-punctuation in book.toml."""
    escaped = re.sub(r"&quot;(?=\S)(.+?)(?<=\S)&quot;", "\u201c\\1\u201d", escaped)
    return escaped.replace("&#x27;", "\u2019")


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
    out = smart(esc(text))

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
        # The home page (src/index.md) never gathers pages: every section is in SUMMARY.md.
        if posixpath.basename(path) in ("index.md", "README.md") and posixpath.dirname(path):
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
            # Alphabetical, ignoring a leading "The" so "The Gilded Ascent" sorts under G.
            found.sort(key=lambda c: re.sub(r"^the\s+", "", c["name"].lower()))
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
        "summary_explicit": False,
        "tags": [],
        "notabs": False,
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
                if meta.get("summary"):
                    info["summary_explicit"] = True
                if meta.get("tags") and not info["tags"]:
                    info["tags"] = [t.strip() for t in meta["tags"].split(",") if t.strip()]
                if meta.get("tabs", "").strip().lower() in ("no", "off", "false"):
                    info["notabs"] = True
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


def chip(label, cls="", title=""):
    return '<span class="cx-chip%s"%s>%s</span>' % (
        (" " + cls) if cls else "",
        ' title="%s"' % esc(title) if title else "",
        label,
    )


def status_chip(status):
    label, text = STATUSES[status]
    return chip(label, "cx-chip-%s" % status, text)


def status_note(status, note, page):
    """`{{draft: your own words}}`: the chip in the title says Draft; the words go under it."""
    label, _ = STATUSES[status]
    return '<p class="cx-note cx-note-%s"><span class="cx-note-label">%s</span> %s</p>' % (
        status,
        label,
        inline(note, page),
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


def image_src(img, page):
    return img if is_external(img) else rel(img, page)


def hero(h1_line, info, page):
    """The title block: emblem or picture, the title, its one-line summary and a few chips."""
    chips = []
    kind = info["fields"].get("type")
    if kind:
        chips.append(chip(inline(kind, page)))
    for tag in info["tags"]:
        chips.append(chip(inline(tag, page)))
    if info["status"]:
        chips.append(status_chip(info["status"]))
    art = ""
    if info["image"]:
        art = '<figure class="cx-hero-art"><img src="%s" alt="%s">%s</figure>' % (
            esc(image_src(info["image"], page)),
            esc(plain(info["title"])),
            "<figcaption>%s</figcaption>" % inline(info["caption"], page) if info["caption"] else "",
        )
    tagline = ""
    if info["summary"] and info["summary_explicit"]:
        tagline = '<p class="cx-tagline">%s</p>' % inline(info["summary"], page)
    chips_html = '<div class="cx-chips">%s</div>' % "".join(chips) if chips else ""
    return [
        '<header class="cx-hero%s">' % (" has-art" if art else ""),
        "" if art else '<span class="cx-emblem" aria-hidden="true" data-icon-for="%s" data-icon-class="cx-emblem-i"></span>'
        % esc(html_path(page)),
        '<div class="cx-hero-text">',
        "",
        h1_line,
        "",
        tagline + chips_html + "</div>" + art + "</header>",
        "",
    ]


def facts(lines, chapter):
    """The infobox, shown as the "At a glance" panel beside the page."""
    page = chapter["source_path"]
    meta, rows = parse_fields(lines)
    title = meta.get("facts") or "At a glance"
    out = [
        '<aside class="cx-facts" aria-label="%s">' % esc(plain(title)),
        # The title is drawn by CSS from data-title, so search results don't repeat it.
        '<div class="cx-panel-title" data-title="%s"></div>' % esc(plain(title)),
        '<table class="cx-facts-rows"><tbody>',
    ]
    for row in rows:
        if row[0] == "section":
            out.append('<tr class="cx-facts-section"><th colspan="2">%s</th></tr>' % inline(row[1], page))
        elif row[0] == "text":
            out.append('<tr><td colspan="2">%s</td></tr>' % inline(row[1], page))
        else:
            value = pips(row[2]) or inline(row[2], page) or '<span class="wiki-muted">Unknown</span>'
            out.append("<tr><th>%s</th><td>%s</td></tr>" % (inline(row[1], page), value))
    out.append("</tbody></table></aside>")
    return "".join(out)


def card(info, page):
    href = rel(html_path(info["path"]), page)
    img = info["image"]
    if img:
        picture = '<img class="wiki-card-img" src="%s" alt="" loading="lazy">' % esc(image_src(img, page))
    else:
        initial = (re.sub(r"^the\s+", "", plain(info["title"]), flags=re.I)[:1] or "?").upper()
        picture = (
            '<span class="wiki-card-img wiki-card-noimg" aria-hidden="true" data-icon-for="%s" '
            'data-icon-class="cx-card-i">%s</span>' % (esc(html_path(info["path"])), esc(initial))
        )
    badge = ""
    if info["status"]:
        badge = '<span class="wiki-badge wiki-card-badge wiki-badge-%s">%s</span>' % (
            info["status"],
            STATUSES[info["status"]][0],
        )
    summary = '<span class="wiki-card-text">%s</span>' % smart(esc(info["summary"])) if info["summary"] else ""
    return (
        '<a class="wiki-card" href="%s">%s%s<span class="wiki-card-body">'
        '<span class="wiki-card-title">%s</span>%s</span></a>'
        % (esc(href), picture, badge, smart(esc(plain(info["title"]))), summary)
    )


def cards(paths, pages, page):
    found = [pages[p] for p in paths if p in pages]
    if not found:
        return '<p class="wiki-muted">No pages here yet.</p>'
    return '<div class="wiki-cards">\n%s\n</div>' % "\n".join(card(i, page) for i in found)


def row_list(paths, pages, page):
    """```list: one row per page, with its section's icon, title and summary."""
    out = ['<ul class="cx-rowlist">']
    for p in paths:
        i = pages.get(p)
        if not i:
            continue
        out.append(
            '<li><a href="%s"><span class="cx-rowlist-icon" data-icon-for="%s"></span>'
            '<span class="cx-rowlist-text"><span class="cx-rowlist-title">%s</span>%s</span>'
            '<svg class="cx-i cx-rowlist-go" aria-hidden="true"><use href="#i-next"></use></svg></a></li>'
            % (
                esc(rel(html_path(i["path"]), page)),
                esc(html_path(i["path"])),
                smart(esc(plain(i["title"]))),
                '<span class="cx-rowlist-sub">%s</span>' % smart(esc(i["summary"])) if i["summary"] else "",
            )
        )
    out.append("</ul>")
    return "".join(out)


def table(columns, info, pages, page):
    rows = [pages[p] for p in info["children"] if p in pages]
    if not rows:
        return '<p class="wiki-muted">No pages here yet.</p>'
    head = "".join("<th>%s</th>" % esc(c) for c in ["Name"] + columns)
    body = []
    for r in rows:
        cells = ['<td><a href="%s">%s</a></td>' % (esc(rel(html_path(r["path"]), page)), smart(esc(plain(r["title"]))))]
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
        out.append('<ul class="wiki-review cx-nolist">')
        for i in items:
            where = " / ".join(i["parents"])
            out.append(
                '<li><a href="%s">%s</a>%s</li>'
                % (
                    esc(rel(html_path(i["path"]), page)),
                    smart(esc(plain(i["title"]))),
                    ' <span class="wiki-muted">in %s</span>' % esc(where) if where else "",
                )
            )
        out.append("</ul>")
    return "\n".join(out)


def home_hero(lines, page):
    """```hero on the home page: a big title over a picture, and buttons."""
    title, tagline, img, caption, buttons = "", "", None, "", []
    for line in lines:
        key, sep, value = line.strip().partition(":")
        key, value = key.strip().lower(), value.strip()
        if not sep:
            continue
        if key == "title":
            title = value
        elif key == "tagline":
            tagline = value
        elif key == "image":
            img = image_path(value, page)
        elif key == "caption":
            caption = value
        elif key == "button":
            label, _, target = value.partition("|")
            target = target.strip()
            buttons.append((label.strip(), target if is_external(target) else html_path(target)))
    out = ['<header class="cx-home-hero">']
    if img:
        out.append(
            '<figure class="cx-home-art"><img src="%s" alt="">%s</figure>'
            % (esc(image_src(img, page)), "<figcaption>%s</figcaption>" % inline(caption, page) if caption else "")
        )
    out.append('<div class="cx-home-text"><h1 class="cx-home-title">%s</h1>' % inline(title, page))
    if tagline:
        out.append('<p class="cx-tagline">%s</p>' % inline(tagline, page))
    if buttons:
        out.append('<div class="cx-buttons">')
        for n, (label, href) in enumerate(buttons):
            out.append(
                '<a class="cx-btn%s" href="%s">%s<svg class="cx-i" aria-hidden="true"><use href="#i-arrow"></use></svg></a>'
                % (" cx-btn-gold" if n == 0 else "", esc(href), inline(label, page))
            )
        out.append("</div>")
    out.append("</div></header>")
    return "".join(out)


def columns(lines, page):
    """```columns: `## Title | icon`, a line of text, then `- [link](page.md)` lines."""
    cols = []
    for line in lines:
        s = line.strip()
        if not s:
            continue
        if s.startswith("#"):
            title, _, ico = s.lstrip("#").strip().partition("|")
            cols.append({"title": title.strip(), "icon": ico.strip() or "dot", "text": [], "links": []})
        elif cols and s.startswith(("- ", "* ")):
            cols[-1]["links"].append(s[2:].strip())
        elif cols:
            cols[-1]["text"].append(s)
    out = ['<div class="cx-columns">']
    for c in cols:
        out.append(
            '<section class="cx-col"><div class="cx-col-head"><svg class="cx-i cx-col-icon" aria-hidden="true">'
            '<use href="#i-%s"></use></svg><span class="cx-col-title">%s</span></div>'
            % (esc(c["icon"]), inline(c["title"], page))
        )
        if c["text"]:
            out.append('<p class="cx-col-text">%s</p>' % inline(" ".join(c["text"]), page))
        out.append('<ul class="cx-col-links">')
        for link in c["links"]:
            out.append(
                "<li>%s</li>"
                % inline(link, page).replace(
                    "</a>", '<svg class="cx-i" aria-hidden="true"><use href="#i-arrow"></use></svg></a>', 1
                )
            )
        out.append("</ul></section>")
    out.append("</div>")
    return "".join(out)


def flow(lines, page):
    """```flow: how a quest can go. `start:`, then `path:` lines side by side, then `join:`,
    then `end:` lines side by side. Each value is `Title | one line`."""
    steps = []
    for line in lines:
        key, sep, value = line.strip().partition(":")
        key = key.strip().lower()
        if not sep or key not in ("start", "path", "join", "end"):
            continue
        title, _, text = value.partition("|")
        box = '<div class="cx-flow-box cx-flow-%s"><span class="cx-flow-title">%s</span>%s</div>' % (
            key,
            inline(title.strip(), page),
            '<span class="cx-flow-text">%s</span>' % inline(text.strip(), page) if text.strip() else "",
        )
        if steps and steps[-1][0] == key and key in ("path", "end"):
            steps[-1][1].append(box)
        else:
            steps.append((key, [box]))
    out = ['<div class="cx-flow">']
    for n, (key, boxes) in enumerate(steps):
        if n:
            out.append('<div class="cx-flow-arrow" aria-hidden="true"></div>')
        out.append(
            '<div class="cx-flow-row cx-flow-row-%s" style="--n:%d">%s</div>' % (key, len(boxes), "".join(boxes))
        )
    out.append("</div>")
    return "".join(out)


def nice_date(iso):
    try:
        y, m, d = (int(x) for x in iso.split("-"))
    except ValueError:
        return iso
    months = "Jan Feb Mar Apr May Jun Jul Aug Sep Oct Nov Dec".split()
    return "%d %s %d" % (d, months[m - 1], y)


def git_history(root, src):
    """Recent commits that touched the wiki's pages, newest first, and each page's last date.
    Empty when git or the history is missing (a download without .git still builds)."""
    try:
        prefix = subprocess.run(
            ["git", "rev-parse", "--show-prefix"], cwd=root, capture_output=True, text=True, timeout=10
        ).stdout.strip()
        log = subprocess.run(
            ["git", "log", "-n", "400", "--date=short", "--format=@@%ad|%s", "--name-only", "--", src],
            cwd=root,
            capture_output=True,
            text=True,
            timeout=30,
        )
    except (OSError, subprocess.SubprocessError):
        return [], {}
    if log.returncode != 0:
        return [], {}
    lead = prefix + src.rstrip("/") + "/"
    commits, updated = [], {}
    for line in log.stdout.split("\n"):
        if line.startswith("@@"):
            date, _, subject = line[2:].partition("|")
            commits.append({"date": date, "subject": subject.strip(), "pages": []})
        elif line.strip() and commits and line.startswith(lead) and line.endswith(".md"):
            path = line[len(lead):]
            commits[-1]["pages"].append(path)
            updated.setdefault(path, commits[-1]["date"])
    return [c for c in commits if c["pages"]], updated


def recent_list(commits, pages, page, limit=30):
    if not commits:
        return '<p class="wiki-muted">The list of changes appears on the published site.</p>'
    out, day = [], None
    for c in commits[:limit]:
        if c["date"] != day:
            if day is not None:
                out.append("</ul></section>")
            day = c["date"]
            out.append('<section class="cx-day"><div class="cx-day-date">%s</div><ul class="cx-changes cx-nolist">' % nice_date(day))
        subject = re.sub(r"^wiki:\s*", "", c["subject"], flags=re.I)
        links = []
        seen = set()
        for p in c["pages"]:
            if p in pages and p not in seen:
                seen.add(p)
                links.append('<a href="%s">%s</a>' % (esc(rel(html_path(p), page)), smart(esc(plain(pages[p]["title"])))))
        more = ""
        if len(links) > 8:
            more = ' <span class="wiki-muted">and %d more</span>' % (len(links) - 8)
            links = links[:8]
        out.append(
            '<li><span class="cx-change-what">%s</span><span class="cx-change-pages">%s%s</span></li>'
            % (smart(esc(subject[:1].upper() + subject[1:])), " · ".join(links) or '<span class="wiki-muted">Pages since moved</span>', more)
        )
    out.append("</ul></section>")
    return "\n".join(out)


SPOILER_START = re.compile(r"^\s{0,3}>\s*\[!WARNING\]\s*$", re.I)
SPOILER_TEXT = re.compile(r"^\s{0,3}>\s*(spoilers?\b.*?)\s*$", re.I)
SECTION_START = re.compile(r"^\s{0,3}#{1,2}\s")


def spoiler_note(text):
    note = re.sub(r"\s*\bbelow\b\.?$", "", plain(text).strip()).rstrip(".")
    return note or "Spoilers"


def transform(chapter, pages, history):
    page = chapter["source_path"]
    info = pages[page]
    blocks = list(split_blocks(chapter["content"]))
    out = []
    infobox_lines = None
    h1_done = False
    has_hero = False
    spoiler_open = False
    body_started = False
    home = any(b[0] == "fence" and b[1] == "hero" for b in blocks)

    def open_body():
        out.extend(
            [
                '<div class="cx-body"%s>' % (' data-rail="off"' if home else ""),
                "@@FACTS@@",
                '<article class="cx-main%s" id="cx-main">' % (" cx-notabs" if home or info["notabs"] else ""),
                "",
            ]
        )

    def close_spoiler():
        out.extend(["", "</div>", ""])

    i = 0
    while i < len(blocks):
        block = blocks[i]
        i += 1
        if block[0] == "fence":
            kind, body, raw = block[1], block[2], block[3]
            if kind == "infobox":
                if infobox_lines is None:
                    infobox_lines = body
                continue
            if kind == "meta":
                continue
            if kind == "hero":
                out.extend(["", home_hero(body, page), ""])
                has_hero = True
                if not body_started:
                    open_body()
                    body_started = True
                continue
            if kind in ("cards", "list"):
                paths = [l.strip() for l in body if l.strip() and not l.strip().startswith("#")]
                out.extend(["", cards(paths, pages, page) if kind == "cards" else row_list(paths, pages, page), ""])
                continue
            if kind == "columns":
                out.extend(["", columns(body, page), ""])
                continue
            if kind == "flow":
                out.extend(["", flow(body, page), ""])
                continue
            if kind == "spoiler":
                lines = list(body)
                note = "Spoilers"
                if lines and lines[0].strip().lower().startswith("note:"):
                    note = lines.pop(0).strip()[5:].strip() or note
                out.extend(['<div class="cx-spoiler" data-note="%s">' % esc(note), ""] + lines + ["", "</div>", ""])
                continue
            out.extend(raw)
            continue
        line = block[1]
        if not h1_done and H1.match(line):
            h1_done = True
            has_hero = True
            # A title with a picture spans the page; one without sits in the text column, so
            # the "At a glance" panel can stand beside it.
            if info["image"]:
                out.extend(hero(line, info, page))
            if not body_started:
                open_body()
                body_started = True
            if not info["image"]:
                out.extend(hero(line, info, page))
            continue
        if spoiler_open and SECTION_START.match(line):
            close_spoiler()
            spoiler_open = False
        # "> [!WARNING]" + "> Spoilers for … below." hides the rest of the section behind a cover.
        if SPOILER_START.match(line) and i < len(blocks) and blocks[i][0] == "line":
            m = SPOILER_TEXT.match(blocks[i][1])
            after = blocks[i + 1] if i + 1 < len(blocks) else ("line", "")
            if m and not (after[0] == "line" and after[1].lstrip().startswith(">")):
                if spoiler_open:
                    close_spoiler()
                out.extend(["", '<div class="cx-spoiler" data-note="%s">' % esc(spoiler_note(m.group(1))), ""])
                spoiler_open = True
                i += 1
                continue
        d = DIRECTIVE.match(line.strip())
        if d:
            name, arg = d.group(1).lower(), d.group(2)
            if name in STATUSES:
                if arg:
                    out.extend(["", status_note(name, arg, page), ""])
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
            if name == "recent":
                out.extend(["", recent_list(history[0], pages, page), ""])
                continue
        out.append(line)
    if spoiler_open:
        close_spoiler()
    if not body_started:
        # A page without a title: still give it the same frame.
        out[:0] = ['<div class="cx-body">', "@@FACTS@@", '<article class="cx-main" id="cx-main">', ""]
    out.extend(["", "</article>", "</div>", ""])
    facts_html = facts(infobox_lines, chapter) if infobox_lines is not None else ""
    out = [facts_html if l == "@@FACTS@@" else l for l in out]
    if not has_hero and info["status"]:
        out[:0] = ['<div class="cx-chips cx-chips-solo">%s</div>' % status_chip(info["status"]), ""]
    meta = {}
    if history[1].get(page):
        meta["updated"] = nice_date(history[1][page])
    if meta:
        out.extend(['<script type="application/json" id="cx-meta">%s</script>' % json.dumps(meta), ""])
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
    history = git_history(root, src)
    for ch in walk(book["items"]):
        if ch.get("source_path"):
            transform(ch, pages, history)
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
                    if block[1] in ("cards", "list"):
                        for l in block[2]:
                            p = l.strip()
                            if p and not p.startswith("#") and not os.path.isfile(os.path.join(src, p)):
                                warn(page, n + 1, "%s: no page at src/%s" % (block[1], p))
                    if block[1] == "hero":
                        for l in block[2]:
                            key, _, value = l.strip().partition(":")
                            key, value = key.strip().lower(), value.strip()
                            if key == "image":
                                img = image_path(value, page)
                                if img and not is_external(img) and not os.path.isfile(os.path.join(src, img)):
                                    warn(page, n + 1, "hero image not found: src/%s" % img)
                            elif key == "button":
                                target = resolve(value.partition("|")[2].strip(), page)
                                if target and not os.path.exists(os.path.join(src, target)):
                                    warn(page, n + 1, "hero button: no page at src/%s" % target)
                    if block[1] in ("columns", "flow"):
                        for l in block[2]:
                            for m in LINK.finditer(l):
                                target = resolve(m.group(2), page)
                                if target and not os.path.exists(os.path.join(src, target)):
                                    warn(page, n + 1, "broken link: %s" % m.group(2))
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
