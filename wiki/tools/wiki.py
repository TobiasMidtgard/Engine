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
    ```levels ```bars     charts: level ranges on the 1-100 scale, and horizontal bars
    ```stats ```tiers     number tiles for a boss, and a staircase of ranks or stages
    ```timeline           events down a line, oldest first
    {{levels}}            this section's pages on the level scale, from their Level rows
    ```gallery            pictures side by side with captions (`picture | caption` lines)
    `map: x, y`           (infobox) a pin on the world map and a small map in the panel
    {{worldmap}}          the world map with every page that has `map:`

Every page is framed for theme/index.hbs: its title, summary and chips become the title block,
the infobox becomes the panel beside the text, and theme/codex.js adds the side panel.

A page saved in a section's folder joins the sidebar by itself, after the pages SUMMARY.md lists,
in alphabetical order. A folder with an index.md inside a section becomes a sub-section.
"""

import html
import json
import math
import os
import posixpath
import re
import subprocess
import sys

RESERVED = {"title", "image", "caption", "summary", "status", "tags", "tabs", "facts", "map"}
STATUSES = {
    "draft": (
        "Draft",
        "Names and details on this page may still change.",
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
        "fields": {},
        "map": None,
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
                if meta.get("map") and not info["map"]:
                    info["map"] = map_point(meta["map"])
                if meta.get("summary"):
                    info["summary_explicit"] = True
                if meta.get("tags") and not info["tags"]:
                    info["tags"] = [t.strip() for t in meta["tags"].split(",") if t.strip()]
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
    out.append("</tbody></table>")
    point = map_point(meta.get("map", ""))
    if point:
        out.append(locator(point, page))
    out.append("</aside>")
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


# ---------------------------------------------------------------------------------------------
# Charts: ```levels ```bars ```stats ```tiers ```timeline (plain HTML; theme/wiki.css draws them)

LEVEL_BANDS = (("Human", 1, 60), ("Awakened", 61, 80), ("Ascendant", 81, 100))
CHART_OPTIONS = ("title", "max", "unit", "note", "scale", "show")


def chart_rows(lines):
    """A chart block's lines -> (options, rows). `title:`, `max:`, `unit:` and `note:` lines are
    options, `## Heading` starts a group and every other line is a row of `|`-separated cells."""
    opts, rows = {}, []
    for line in lines:
        s = line.strip()
        if not s or s.startswith("<!--"):
            continue
        if s.startswith("#"):
            rows.append(("group", [p.strip() for p in s.lstrip("#").strip().split("|")]))
            continue
        key, sep, value = s.partition(":")
        if sep and "|" not in s and key.strip().lower() in CHART_OPTIONS:
            opts[key.strip().lower()] = value.strip()
            continue
        rows.append(("row", [p.strip() for p in s.split("|")]))
    return opts, rows


def number(text):
    m = re.match(r"^\s*[^\d-]{0,2}(-?\d[\d,]*(?:\.\d+)?)", text or "")
    return float(m.group(1).replace(",", "")) if m else None


def level_range(text):
    """`1-10`, `1 to 10`, `55`, `61+` -> (low, high, open-ended) or None."""
    t = (text or "").replace("–", "-").replace("—", "-").strip()
    m = re.match(r"^(\d+)\s*(?:-|to)\s*(\d+)$", t)
    if m:
        a, b = int(m.group(1)), int(m.group(2))
        return min(a, b), max(a, b), False
    m = re.match(r"^(\d+)\s*\+$", t)
    if m:
        return int(m.group(1)), 100, True
    m = re.match(r"^(\d+)$", t)
    if m:
        return int(m.group(1)), int(m.group(1)), False
    return None


def level_span(lo, hi):
    """Each level is one slot 1% wide: level 1 fills 0 to 1%, level 100 fills 99 to 100%."""
    return max(0.0, lo - 1.0), min(100.0, float(hi))


def chart_title(opts, page):
    out = ""
    if opts.get("title"):
        out += '<figcaption class="cx-chart-title">%s</figcaption>' % inline(opts["title"], page)
    return out


def chart_note(opts, page):
    return '<p class="cx-chart-note">%s</p>' % inline(opts["note"], page) if opts.get("note") else ""


def levels_chart(lines, page):
    """```levels: `Name | 1-10 | note` rows on the 1 to 100 level scale, over the Human, Awakened
    and Ascendant bands. A single level draws a dot, `61+` a bar that runs off the end."""
    opts, rows = chart_rows(lines)
    out = ['<figure class="cx-chart cx-levels" style="--h:60%;--w:80%">', chart_title(opts, page)]
    out.append('<div class="cx-lv-row cx-lv-bands" aria-hidden="true"><span></span><span class="cx-lv-track">')
    for name, lo, hi in LEVEL_BANDS:
        out.append(
            '<span class="cx-lv-band" style="--a:%.2f%%;--b:%.2f%%">%s <small>%d–%d</small></span>'
            % (level_span(lo, hi) + (name, lo, hi))
        )
    out.append("</span></div>")
    for kind, cells in rows:
        if kind == "group":
            out.append('<div class="cx-lv-group">%s</div>' % inline(cells[0], page))
            continue
        label = inline(cells[0], page)
        value = cells[1] if len(cells) > 1 else ""
        note = cells[2] if len(cells) > 2 else ""
        rng = level_range(value)
        tip = "%s: %s" % (plain(cells[0]), ("level " if rng and rng[0] == rng[1] else "levels ") + value if value else "")
        out.append('<div class="cx-lv-row" title="%s">' % esc(tip))
        out.append(
            '<span class="cx-lv-label">%s%s</span>'
            % (label, '<small class="cx-lv-note">%s</small>' % inline(note, page) if note else "")
        )
        out.append('<span class="cx-lv-track">')
        if rng:
            lo, hi, open_end = rng
            a, b = level_span(lo, hi)
            shape = "cx-lv-dot" if lo == hi else "cx-lv-bar" + (" open" if open_end else "")
            text = esc(value.replace("-", "–").replace(" to ", "–"))
            # The value sits after the mark, before it near the right edge, or inside a long bar
            # that reaches from one edge almost to the other.
            side = "after" if b <= 68 else ("before" if a > 12 else "inside")
            if side == "inside" and lo != hi:
                shape += " has-val"
            elif side == "inside":
                side = "after"
            out.append('<span class="%s" style="--a:%.2f%%;--b:%.2f%%"></span>' % (shape, a, b))
            out.append('<span class="cx-lv-val %s" style="--a:%.2f%%;--b:%.2f%%">%s</span>' % (side, a, b, text))
        else:
            out.append('<span class="cx-lv-val cx-lv-none">%s</span>' % (inline(value, page) or "Any level"))
        out.append("</span></div>")
    out.append('<div class="cx-lv-row cx-lv-axis" aria-hidden="true"><span></span><span class="cx-lv-track">')
    for tick in (1, 20, 40, 60, 80, 100):
        out.append('<span style="--a:%d%%">%d</span>' % (0 if tick == 1 else tick, tick))
    out.append("</span></div>")
    out.append(
        '<div class="cx-lv-key" aria-hidden="true">%s</div>'
        % "".join("<span><i></i>%s %d\u2013%d</span>" % band for band in LEVEL_BANDS)
    )
    out.append(chart_note(opts, page))
    out.append("</figure>")
    return "".join(out)


def section_levels(arg, info, pages, page):
    """{{levels}}: this section's pages on the level scale, from their infobox Level (or Levels)
    field, lowest first. `{{levels: Title}}` gives the chart a title."""
    rows = []
    for path in info["children"]:
        child = pages.get(path)
        if not child or child["fields"].get("type", "").lower() == "list":
            continue
        raw = child["fields"].get("level") or child["fields"].get("levels") or ""
        m = re.search(r"(\d+)(?:\s*(?:to|-|\u2013)\s*(\d+))?", raw)
        if not m:
            continue
        lo, hi = int(m.group(1)), int(m.group(2) or m.group(1))
        label = "[%s](%s)" % (plain(child["title"]), rel(path, page))
        rows.append((lo, hi, "%s | %s | %s" % (label, "%d-%d" % (lo, hi) if hi != lo else str(lo), child["fields"].get("type", ""))))
    rows.sort(key=lambda r: (r[0], r[1]))
    lines = (["title: " + arg] if arg else []) + [r[2] for r in rows]
    return levels_chart(lines, page) if rows else ""


def bars_chart(lines, page):
    """```bars: `Name | 45 | note` rows as horizontal bars from one baseline, the value at the
    tip. `max:` fixes the scale (default: the largest value), `unit:` follows each value."""
    opts, rows = chart_rows(lines)
    values = [number(c[1]) for k, c in rows if k == "row" and len(c) > 1]
    values = [v for v in values if v is not None]
    top = number(opts.get("max", "")) or (max(values) if values else 1) or 1
    unit = opts.get("unit", "")
    log = opts.get("scale", "").lower() == "log"
    show_note = opts.get("show", "").lower() == "note"
    out = ['<figure class="cx-chart cx-bars%s">' % (" cx-bars-log" if log else ""), chart_title(opts, page)]
    for kind, cells in rows:
        if kind == "group":
            out.append('<div class="cx-bar-group">%s</div>' % inline(cells[0], page))
            continue
        value = cells[1] if len(cells) > 1 else ""
        note = cells[2] if len(cells) > 2 else ""
        v = number(value)
        shown = esc(value) + ((" " + esc(unit)) if unit and v is not None else "")
        if show_note and note:
            # `show: note`: the bar is drawn from the number, the third cell is what it says.
            shown, value, note = inline(note, page), plain(note), ""
        out.append('<div class="cx-bar-row" title="%s">' % esc("%s: %s" % (plain(cells[0]), plain(value + (" " + unit if unit else "")))))
        out.append(
            '<span class="cx-bar-label">%s%s</span>'
            % (inline(cells[0], page), '<small class="cx-bar-note">%s</small>' % inline(note, page) if note else "")
        )
        if v is None:
            out.append('<span class="cx-bar-track"><span class="cx-bar-val cx-bar-text">%s</span></span></div>' % inline(value, page))
            continue
        if log:
            # A log scale from a tenth of one unit: every tenfold step is the same length.
            pct = max(0.6, min(100.0, (math.log10(max(v, 0.1)) + 1) / (math.log10(top) + 1) * 100.0))
        else:
            pct = max(0.6, min(100.0, v / top * 100.0))
        inside = " in" if pct > 82 else ""
        out.append(
            '<span class="cx-bar-track"><span class="cx-bar" style="--v:%.2f%%"></span>'
            '<span class="cx-bar-val%s" style="--v:%.2f%%">%s</span></span></div>' % (pct, inside, pct, shown)
        )
    out.append(chart_note(opts, page))
    out.append("</figure>")
    return "".join(out)


STAT_NUMBER = re.compile(r"^((?:\d[\d,.]*)(?:\s*(?:to|-|\u2013)\s*\d[\d,.]*)?\+?(?:\s*(?:%|kg|m|s))?)(?:\s*[,;(]\s*(.*?)\)?)?$")


def stats_tiles(lines, page):
    """```stats: `Label | value | note` rows. Short values and numbers become big tiles (a number
    followed by a remark keeps the remark as a note); long values are listed under the tiles."""
    opts, rows = chart_rows(lines)
    tiles, wide = [], []
    for kind, cells in rows:
        if kind != "row" or len(cells) < 2:
            continue
        label, raw = cells[0], cells[1]
        note = cells[2] if len(cells) > 2 else ""
        m = STAT_NUMBER.match(raw)
        if pips(raw):
            tiles.append((label, pips(raw), note, ""))
        elif m and len(m.group(2) or "") + len(note) <= 44:
            tiles.append((label, inline(m.group(1), page), note or (m.group(2) or ""), ""))
        elif len(plain(raw)) <= 16:
            tiles.append((label, inline(raw, page), note, " word"))
        else:
            wide.append((label, raw))
    out = ['<div class="cx-statblock">', chart_title(opts, page).replace("figcaption", "div")]
    if tiles:
        out.append('<div class="cx-stats">')
        for label, value, note, cls in tiles:
            out.append(
                '<div class="cx-stat"><span class="cx-stat-label">%s</span><span class="cx-stat-value%s">%s</span>%s</div>'
                % (inline(label, page), cls, value, '<span class="cx-stat-note">%s</span>' % inline(note, page) if note else "")
            )
        out.append("</div>")
    if wide:
        out.append('<dl class="cx-stat-rows">')
        for label, raw in wide:
            out.append("<div><dt>%s</dt><dd>%s</dd></div>" % (inline(label, page), inline(raw, page)))
        out.append("</dl>")
    out.append("</div>")
    return "".join(out)


def tiers_chart(lines, page):
    """```tiers: `Name | what it means | small label` rows as a staircase, lowest first."""
    opts, rows = chart_rows(lines)
    tiers = [c for k, c in rows if k == "row"]
    n = max(1, len(tiers))
    out = ['<figure class="cx-chart cx-tiers-fig">', chart_title(opts, page), '<div class="cx-tiers" style="--n:%d">' % n]
    for i, cells in enumerate(tiers):
        tone = 0.28 + 0.72 * (i / (n - 1) if n > 1 else 1)
        out.append(
            '<div class="cx-tier" style="--i:%d;--t:%.2f;--w:%.0f%%">'
            '<div class="cx-tier-head">%s<span class="cx-tier-name">%s</span></div>'
            '<div class="cx-tier-step" aria-hidden="true"></div>'
            '<div class="cx-tier-text">%s</div></div>'
            % (
                i,
                tone,
                30 + 70 * (i / (n - 1) if n > 1 else 1),
                '<span class="cx-tier-sub">%s</span>' % inline(cells[2], page) if len(cells) > 2 and cells[2] else "",
                inline(cells[0], page),
                inline(cells[1], page) if len(cells) > 1 else "",
            )
        )
    out.append("</div>" + chart_note(opts, page) + "</figure>")
    return "".join(out)


def timeline(lines, page):
    """```timeline: `## Age | when` headings and `When | what happened` events, oldest first."""
    opts, rows = chart_rows(lines)
    out = ['<figure class="cx-chart cx-tl-fig">', chart_title(opts, page), '<ol class="cx-tl">']
    for kind, cells in rows:
        if kind == "group":
            out.append(
                '<li class="cx-tl-era"><span class="cx-tl-era-name">%s</span>%s</li>'
                % (inline(cells[0], page), '<span class="cx-tl-era-when">%s</span>' % inline(cells[1], page) if len(cells) > 1 else "")
            )
            continue
        when, text = (cells[0], " | ".join(cells[1:])) if len(cells) > 1 else ("", cells[0])
        out.append(
            '<li class="cx-tl-ev"><span class="cx-tl-when">%s</span><span class="cx-tl-dot" aria-hidden="true"></span>'
            '<div class="cx-tl-text">%s</div></li>' % (inline(when, page), inline(text, page))
        )
    out.append("</ol>" + chart_note(opts, page) + "</figure>")
    return "".join(out)


# ---------------------------------------------------------------------------------------------
# Pictures and the world map

WORLD_MAP = "images/world/world-map.jpg"
WORLD_MAP_RATIO = 1536 / 1024
WORLD_PAGE = "world/index.md"
MAP_KINDS = (("cities/", "city"), ("world/regions/", "region"), ("world/dungeons/", "dungeon"), ("world/landmarks/", "landmark"))


def map_point(text):
    """`map: 48.5, 46` in an infobox: where the place is on the world map, in percent from the
    map's left and top edges."""
    m = re.match(r"^\s*(-?\d+(?:\.\d+)?)\s*[, ]\s*(-?\d+(?:\.\d+)?)\s*$", text or "")
    if not m:
        return None
    x, y = float(m.group(1)), float(m.group(2))
    return (x, y) if 0 <= x <= 100 and 0 <= y <= 100 else None


def map_kind(path):
    for prefix, kind in MAP_KINDS:
        if path.startswith(prefix):
            return kind
    return "place"


def locator(point, page, zoom=2.4, window=0.62):
    """The small map in the "At a glance" panel: the world map, zoomed in on this place, with a pin.
    `window` is the panel's height as a share of its width."""
    x, y = point
    map_h = zoom / WORLD_MAP_RATIO  # the zoomed map's height, in panel widths
    off_x = min(max(zoom * x - 50.0, 0.0), (zoom - 1) * 100.0)  # percent of the panel width
    pin_y = map_h * y / 100.0
    off_y = min(max(pin_y - window / 2, 0.0), map_h - window)  # panel widths
    style = "width:%.1f%%;left:%.2f%%;top:%.2f%%" % (zoom * 100, -off_x, -off_y / window * 100)
    pin = "left:%.2f%%;top:%.2f%%" % (zoom * x - off_x, (pin_y - off_y) / window * 100)
    return (
        '<a class="cx-locator" href="%s" style="--win:%.0f%%" title="Open the world map">'
        '<img src="%s" alt="" loading="lazy" style="%s"><span class="cx-locator-pin" style="%s"></span>'
        '<span class="cx-locator-label">World map</span></a>'
        % (esc(rel(html_path(WORLD_PAGE), page) + "#map"), window * 100, esc(rel(WORLD_MAP, page)), style, pin)
    )


def world_map(pages, page):
    """{{worldmap}}: the drawn map with a marker for every page that has `map:` in its infobox.
    Cities are pins, regions are names, dungeons are small diamonds; each opens its page."""
    marks = {"city": [], "region": [], "dungeon": [], "landmark": []}
    for path, info in pages.items():
        if info.get("map"):
            marks.setdefault(map_kind(path), []).append((path, info))
    out = [
        '<div class="cx-worldmap" id="map">',
        '<div class="cx-map-layers" role="group" aria-label="Show on the map">',
    ]
    labels = (("city", "Cities"), ("region", "Regions"), ("dungeon", "Dungeons"))
    for kind, label in labels:
        if marks.get(kind):
            out.append(
                '<label class="cx-map-layer cx-map-layer-%s"><input type="checkbox" checked data-layer="%s">'
                '<span class="cx-map-key" aria-hidden="true"></span>%s <small>%d</small></label>'
                % (kind, kind, label, len(marks[kind]))
            )
    out.append("</div>")
    out.append('<div class="cx-map-scroll"><figure class="cx-map">')
    out.append('<img class="cx-map-img" src="%s" alt="The drawn map of the great continent">' % esc(rel(WORLD_MAP, page)))
    order = ("region", "dungeon", "landmark", "city")
    for kind in order:
        for path, info in sorted(marks.get(kind, []), key=lambda pi: pi[1]["map"][1]):
            x, y = info["map"]
            fields = info["fields"]
            facts_line = []
            for key in ("levels", "region"):
                if fields.get(key) and not (kind == "region" and key == "region"):
                    facts_line.append(plain(fields[key]) if key == "region" else "Levels " + plain(fields[key]))
            tip = '<span class="cx-map-tip"><b>%s</b>%s%s</span>' % (
                esc(plain(info["title"])),
                "<em>%s</em>" % esc(" · ".join(facts_line)) if facts_line else "",
                "<span>%s</span>" % esc(shorten(plain(info["summary"] or ""), 120)) if info["summary"] else "",
            )
            side = " cx-tip-left" if x > 70 else (" cx-tip-right" if x < 30 else "")
            below = " cx-tip-below" if y < 22 else ""
            name = '<span class="cx-map-name">%s</span>' % esc(re.sub(r"^The\s+", "", plain(info["title"]))) if kind == "region" else ""
            out.append(
                '<a class="cx-mark cx-mark-%s%s%s" data-layer="%s" href="%s" style="left:%.2f%%;top:%.2f%%">%s'
                '<span class="cx-mark-dot" aria-hidden="true"></span>%s</a>'
                % (kind, side, below, kind, esc(rel(html_path(path), page)), x, y, name, tip)
            )
    out.append("</figure></div></div>")
    return "".join(out)


def zoomable(src, alt):
    """A picture that opens full size when clicked (the same markup mdBook gives Markdown pictures)."""
    return (
        '<label class="checkbox-label"><input class="checkbox-img" type="checkbox">'
        '<img src="%s" alt="%s" loading="lazy"><span class="img-wrapper"><img src="%s" alt="%s" loading="lazy"></span></label>'
        % (esc(src), esc(alt), esc(src), esc(alt))
    )


def gallery(lines, page):
    """```gallery: `picture | caption` lines (paths from src/images/, like the infobox), and an
    optional `wide: yes` to give each picture the full width (maps)."""
    opts, items = {}, []
    for line in lines:
        s = line.strip()
        if not s or s.startswith("#"):
            continue
        key, sep, value = s.partition(":")
        if sep and "|" not in s and key.strip().lower() in ("wide", "title"):
            opts[key.strip().lower()] = value.strip()
            continue
        src, _, caption = s.partition("|")
        items.append((image_path(src.strip(), page), caption.strip()))
    wide = opts.get("wide", "").lower() in ("yes", "true", "1") or len(items) == 1
    cols = 2 if len(items) in (2, 4) else 3
    out = ['<div class="cx-gallery%s" style="--n:%d">' % (" cx-gallery-wide" if wide else "", cols)]
    if opts.get("title"):
        out.append('<div class="cx-chart-title">%s</div>' % inline(opts["title"], page))
    for img, caption in items:
        out.append(
            '<figure class="cx-shot">%s%s</figure>'
            % (zoomable(image_src(img, page), plain(caption)), "<figcaption>%s</figcaption>" % inline(caption, page) if caption else "")
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


CHARTS = {"levels": levels_chart, "bars": bars_chart, "stats": stats_tiles, "tiers": tiers_chart, "timeline": timeline}


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
                '<article class="cx-main" id="cx-main">',
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
            if kind in CHARTS:
                out.extend(["", CHARTS[kind](body, page), ""])
                continue
            if kind == "gallery":
                out.extend(["", gallery(body, page), ""])
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
            if name == "levels":
                out.extend(["", section_levels(arg, info, pages, page), ""])
                continue
            if name == "worldmap":
                out.extend(["", world_map(pages, page), ""])
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
                        if meta.get("map") and not map_point(meta["map"]):
                            warn(page, n + 1, "map: %r is not two numbers from 0 to 100" % meta["map"])
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
                    if block[1] == "gallery":
                        for l in block[2]:
                            p = l.strip()
                            if not p or p.startswith("#") or ("|" not in p and ":" in p):
                                continue
                            img = image_path(p.partition("|")[0].strip(), page)
                            if not os.path.isfile(os.path.join(src, img)):
                                warn(page, n + 1, "gallery picture not found: src/%s" % img)
                    if block[1] in ("columns", "flow") or block[1] in CHARTS:
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
