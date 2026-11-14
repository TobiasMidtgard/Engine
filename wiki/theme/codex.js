/* The Codex: everything the player wiki does in the browser. No libraries.
 *
 *   - the grouped sidebar with icons, built from mdBook's chapter list (toc.js)
 *   - breadcrumbs, and the Read / History / Edit links above every page
 *   - the side panel: spoiler switch, "On this page" (the section list) and "More in this section"
 *   - spoiler blur, card lists for "**Name**: text" lists, scrollable tables
 *   - the search window (Ctrl K, / or S), the dark/light switch and the phone menu
 *
 * Pages still read top to bottom without it: the side panel and the sidebar are extras.
 */
(function () {
    'use strict';

    var ROOT = typeof path_to_root === 'string' ? path_to_root : '';
    var html = document.documentElement;
    var body = document.body;
    var pagePath = body.getAttribute('data-path') || '';
    var pageHtml = pagePath.replace(/\.md$/, '.html').replace(/(^|\/)README\.html$/, '$1index.html');

    function load(key) { try { return localStorage.getItem(key); } catch (e) { return null; } }
    function save(key, value) { try { localStorage.setItem(key, value); } catch (e) { } }
    function el(tag, cls, text) {
        var e = document.createElement(tag);
        if (cls) { e.className = cls; }
        if (text) { e.textContent = text; }
        return e;
    }
    function icon(name, cls) {
        return '<svg class="' + (cls || 'cx-i') + '" aria-hidden="true"><use href="#i-' + name + '"></use></svg>';
    }

    // ---- Icons per section -----------------------------------------------------------------
    var ICONS = [
        ['world/planets/', 'planet'], ['world/regions/', 'mountain'], ['world/landmarks/', 'tower'],
        ['world/', 'map'], ['cities/', 'castle'], ['people/', 'person'], ['factions/', 'shield'],
        ['quests/', 'scroll'], ['lore/', 'book'], ['creatures/', 'paw'], ['items/', 'sword'], ['crafting/', 'hammer'],
        ['magic/', 'spark'], ['play/progression', 'compass'], ['play/thievery', 'key'],
        ['play/', 'guide'], ['vehicles/', 'rocket'], ['recent', 'clock'], ['editing/', 'help'],
        ['index.html', 'home']
    ];
    function iconFor(path) {
        for (var i = 0; i < ICONS.length; i++) {
            if (path.indexOf(ICONS[i][0]) === 0) { return ICONS[i][1]; }
        }
        return 'dot';
    }
    // A sidebar link's href is relative to this page; strip that back to a path in the book.
    function bookPath(a) {
        var href = a.getAttribute('href') || '';
        if (ROOT && href.indexOf(ROOT) === 0) { href = href.slice(ROOT.length); }
        return href.split('#')[0];
    }

    // ---- The chapter list ------------------------------------------------------------------
    var toc = document.getElementById('cx-toc');
    var tocList = toc && toc.querySelector('ol.chapter');
    var groups = [];
    var activeLink = null;
    if (tocList) {
        var current = { title: '', items: [] };
        groups.push(current);
        Array.prototype.forEach.call(tocList.children, function (li) {
            if (li.classList.contains('part-title')) {
                current = { title: li.textContent.trim(), items: [] };
                groups.push(current);
            } else if (li.classList.contains('chapter-item')) {
                var a = li.querySelector(':scope > span > a:not(.chapter-fold-toggle)');
                if (a) { current.items.push(li); }
            }
        });
        groups = groups.filter(function (g) { return g.items.length; });
        activeLink = tocList.querySelector('a.active');
        if (!activeLink && pageHtml) {
            activeLink = Array.prototype.filter.call(tocList.querySelectorAll('a'), function (a) {
                return bookPath(a) === pageHtml;
            })[0] || null;
        }
    }
    function linkOf(li) { return li.querySelector(':scope > span > a:not(.chapter-fold-toggle)'); }
    function childrenOf(li) {
        var ol = li.querySelector(':scope > ol.section');
        if (!ol) { return []; }
        return Array.prototype.filter.call(ol.children, function (c) {
            return c.classList.contains('chapter-item') && linkOf(c);
        });
    }
    function ancestors(link) {
        var chain = [];
        var li = link && link.closest('li.chapter-item');
        while (li) {
            chain.unshift(li);
            li = li.parentElement && li.parentElement.closest('li.chapter-item');
        }
        return chain;
    }
    var activeChain = ancestors(activeLink);
    function partOf(topLi) {
        for (var i = 0; i < groups.length; i++) {
            if (groups[i].items.indexOf(topLi) >= 0) { return groups[i].title; }
        }
        return '';
    }

    // ---- Sidebar -----------------------------------------------------------------------------
    var nav = document.getElementById('cx-nav');
    var collapsed = {};
    try { collapsed = JSON.parse(load('codex-nav-closed') || '{}') || {}; } catch (e) { collapsed = {}; }

    function buildItem(li, depth) {
        var a = linkOf(li);
        var item = el('li', 'cx-item');
        var row = el('div', 'cx-row');
        var link = el('a', 'cx-link');
        link.href = a.getAttribute('href');
        if (depth === 0) {
            link.innerHTML = icon(iconFor(bookPath(a))) + '<span></span>';
            link.lastChild.textContent = a.textContent.trim();
        } else {
            link.textContent = a.textContent.trim();
        }
        var inChain = activeChain.indexOf(li) >= 0;
        if (a === activeLink) { link.classList.add('is-current'); link.setAttribute('aria-current', 'page'); }
        if (inChain) { item.classList.add('in-trail'); }
        row.appendChild(link);
        item.appendChild(row);
        var kids = childrenOf(li);
        if (kids.length) {
            var open = inChain;
            var sub = el('ul', 'cx-sub');
            kids.forEach(function (k) { sub.appendChild(buildItem(k, depth + 1)); });
            var toggle = el('button', 'cx-fold');
            toggle.type = 'button';
            toggle.setAttribute('aria-label', 'Show or hide the pages in ' + a.textContent.trim());
            toggle.setAttribute('aria-expanded', open ? 'true' : 'false');
            toggle.innerHTML = icon('chev');
            toggle.addEventListener('click', function () {
                var now = !item.classList.contains('is-open');
                item.classList.toggle('is-open', now);
                toggle.setAttribute('aria-expanded', now ? 'true' : 'false');
            });
            row.appendChild(toggle);
            item.appendChild(sub);
            if (open) { item.classList.add('is-open'); }
        }
        return item;
    }

    if (nav && groups.length) {
        groups.forEach(function (g) {
            var group = el('div', 'cx-group');
            var hasActive = g.items.some(function (li) { return activeChain.indexOf(li) >= 0; });
            if (g.title) {
                var head = el('button', 'cx-group-head');
                head.type = 'button';
                head.innerHTML = '<span></span>' + icon('chev');
                head.firstChild.textContent = g.title;
                var closed = !!collapsed[g.title] && !hasActive;
                group.classList.toggle('is-closed', closed);
                head.setAttribute('aria-expanded', closed ? 'false' : 'true');
                head.addEventListener('click', function () {
                    var nowClosed = !group.classList.contains('is-closed');
                    group.classList.toggle('is-closed', nowClosed);
                    head.setAttribute('aria-expanded', nowClosed ? 'false' : 'true');
                    if (nowClosed) { collapsed[g.title] = 1; } else { delete collapsed[g.title]; }
                    save('codex-nav-closed', JSON.stringify(collapsed));
                });
                group.appendChild(head);
            }
            var list = el('ul', 'cx-group-list');
            g.items.forEach(function (li) { list.appendChild(buildItem(li, 0)); });
            group.appendChild(list);
            nav.appendChild(group);
        });
        var current = nav.querySelector('.is-current');
        if (current) {
            var side = document.getElementById('cx-side');
            var top = current.getBoundingClientRect().top - side.getBoundingClientRect().top;
            if (top > side.clientHeight - 80) { side.scrollTop = top - side.clientHeight / 2; }
        }
    }

    // ---- Page head: breadcrumbs and Read / History / Edit -------------------------------------
    var main = document.getElementById('cx-page-main');
    var article = document.getElementById('cx-main');
    var bodyGrid = main && main.querySelector('.cx-body');

    if (main && activeChain.length) {
        var head = el('div', 'cx-pagehead');
        var crumbs = el('nav', 'cx-crumbs');
        crumbs.setAttribute('aria-label', 'You are here');
        var part = partOf(activeChain[0]);
        if (part) { crumbs.appendChild(el('span', 'cx-crumb', part)); }
        activeChain.forEach(function (li, i) {
            var a = linkOf(li);
            if (i === activeChain.length - 1) {
                var here = el('span', 'cx-crumb cx-crumb-here', a.textContent.trim());
                here.setAttribute('aria-current', 'page');
                crumbs.appendChild(here);
            } else {
                var c = el('a', 'cx-crumb', a.textContent.trim());
                c.href = a.getAttribute('href');
                crumbs.appendChild(c);
            }
        });
        head.appendChild(crumbs);
        var actions = el('div', 'cx-actions');
        actions.appendChild(el('span', 'cx-action is-on', 'Read'));
        if (pagePath) {
            var hist = el('a', 'cx-action', 'History');
            hist.href = 'https://github.com/TobiasMidtgard/Engine/commits/main/wiki/src/' + pagePath;
            hist.title = 'Every change to this page';
            actions.appendChild(hist);
        }
        var editUrl = body.getAttribute('data-edit');
        if (editUrl) {
            var edit = el('a', 'cx-iconbtn cx-edit');
            edit.href = editUrl;
            edit.title = 'Edit this page';
            edit.setAttribute('aria-label', 'Edit this page');
            edit.innerHTML = icon('pen');
            actions.appendChild(edit);
        }
        var meta = document.getElementById('cx-meta');
        if (meta) {
            try {
                var info = JSON.parse(meta.textContent);
                if (info.updated) {
                    var upd = el('span', 'cx-updated', 'Updated ' + info.updated);
                    actions.insertBefore(upd, actions.firstChild);
                }
            } catch (e) { }
        }
        head.appendChild(actions);
        main.insertBefore(head, main.firstChild);
    }

    // Section icons: the emblem beside a title with no picture, cards without one, row lists.
    Array.prototype.forEach.call(document.querySelectorAll('[data-icon-for]'), function (e) {
        e.innerHTML = icon(iconFor(e.getAttribute('data-icon-for')), e.getAttribute('data-icon-class') || 'cx-i');
    });

    // ---- Content helpers ------------------------------------------------------------------------
    if (article) {
        // "- **Name**: text" lists become a grid of small cards.
        Array.prototype.forEach.call(article.querySelectorAll('ul'), function (ul) {
            if (ul.closest('li') || ul.closest('.wiki-cards, .cx-nolist')) { return; }
            var items = Array.prototype.filter.call(ul.children, function (c) { return c.tagName === 'LI'; });
            if (items.length < 2) { return; }
            var lead = function (li) {
                var n = li.firstChild;
                while (n && n.nodeType === 3 && !n.textContent.trim()) { n = n.nextSibling; }
                if (n && n.tagName === 'P') {
                    n = n.firstChild;
                    while (n && n.nodeType === 3 && !n.textContent.trim()) { n = n.nextSibling; }
                }
                return n && n.tagName === 'STRONG' ? n : null;
            };
            if (!items.every(lead)) { return; }
            ul.classList.add('cx-terms');
            items.forEach(function (li) {
                var strong = lead(li);
                strong.classList.add('cx-term');
                var next = strong.nextSibling;
                if (next && next.nodeType === 3) {
                    // "**Name**: text", "**Name**, which ..." and "**Name** does ..." all lose
                    // their joining mark, and the card text starts with a capital.
                    var rest = next.textContent.replace(/^\s*[:;,—–-]?\s*/, '');
                    next.textContent = rest.charAt(0).toUpperCase() + rest.slice(1);
                }
            });
        });
        // Wide tables scroll sideways on a phone instead of squashing.
        Array.prototype.forEach.call(article.querySelectorAll('table'), function (t) {
            if (t.closest('.wiki-table, .cx-scroll, .cx-facts')) { return; }
            var wrap = el('div', 'cx-scroll');
            t.parentNode.insertBefore(wrap, t);
            wrap.appendChild(t);
        });
    }

    // ---- Spoilers --------------------------------------------------------------------------------
    var spoilers = document.querySelectorAll('.cx-spoiler');
    function spoilersShown() { return html.classList.contains('cx-spoilers-shown'); }
    function setSpoilers(shown) {
        html.classList.toggle('cx-spoilers-shown', shown);
        save('codex-spoilers', shown ? 'shown' : 'hidden');
        Array.prototype.forEach.call(document.querySelectorAll('.cx-spoiler-switch'), function (s) {
            s.setAttribute('aria-checked', shown ? 'true' : 'false');
            var label = s.querySelector('.cx-switch-state');
            if (label) { label.textContent = shown ? 'shown' : 'hidden'; }
        });
        if (!shown) {
            Array.prototype.forEach.call(spoilers, function (s) { s.classList.remove('is-revealed'); });
        }
    }
    Array.prototype.forEach.call(spoilers, function (s) {
        var note = s.getAttribute('data-note') || 'Spoilers';
        var cover = el('button', 'cx-spoiler-cover');
        cover.type = 'button';
        cover.innerHTML = icon('eye') + '<span class="cx-spoiler-note"></span><span class="cx-spoiler-show">Show</span>';
        cover.querySelector('.cx-spoiler-note').textContent = note;
        cover.addEventListener('click', function () { s.classList.add('is-revealed'); });
        s.insertBefore(cover, s.firstChild);
    });

    // ---- Pages read top to bottom; "On this page" in the side panel jumps between sections. ----
    var otpLinks = [];

    function textOf(h) { return (h.textContent || '').replace(/\s+/g, ' ').trim(); }
    function topOffset() {
        var top = document.querySelector('.cx-top');
        return top ? top.offsetHeight : 0;
    }

    // ---- Side panel -----------------------------------------------------------------------------
    var rail = null;
    if (bodyGrid && bodyGrid.getAttribute('data-rail') !== 'off') {
        rail = el('aside', 'cx-rail');
        rail.setAttribute('aria-label', 'About this page');

        if (spoilers.length) {
            var sp = el('div', 'cx-rail-block cx-rail-spoilers');
            var sw = el('button', 'cx-spoiler-switch');
            sw.type = 'button';
            sw.setAttribute('role', 'switch');
            sw.innerHTML = '<span class="cx-switch-label">Spoilers: <span class="cx-switch-state"></span></span><span class="cx-switch-track"><span class="cx-switch-knob"></span></span>';
            sw.addEventListener('click', function () { setSpoilers(!spoilersShown()); });
            sp.appendChild(sw);
            rail.appendChild(sp);
        }

        var heads = article ? Array.prototype.filter.call(article.querySelectorAll('h2'), function (h) {
            return h.id && !h.closest('.wiki-cards');
        }) : [];
        if (heads.length >= 2) {
            var otp = el('div', 'cx-rail-block cx-otp');
            otp.appendChild(el('div', 'cx-rail-title', 'On this page'));
            var ol = el('ol', 'cx-otp-list');
            heads.forEach(function (h) {
                var li = el('li');
                var a = el('a', '', textOf(h));
                a.href = '#' + h.id;
                a.addEventListener('click', function (e) {
                    e.preventDefault();
                    try { history.replaceState(null, '', '#' + h.id); } catch (err) { }
                    var y = h.getBoundingClientRect().top + window.pageYOffset - topOffset() - 16;
                    window.scrollTo(0, Math.max(0, y));
                    markOnThisPage(h);
                });
                li.appendChild(a);
                ol.appendChild(li);
                otpLinks.push({ a: a, h: h });
            });
            otp.appendChild(ol);
            rail.appendChild(otp);
        }

        // More in this section: the pages beside this one (or below it, for a section's front page).
        var here = activeChain[activeChain.length - 1];
        if (here) {
            var parent = activeChain.length > 1 ? activeChain[activeChain.length - 2] : null;
            var mine = childrenOf(here);
            var source = mine.length ? here : parent;
            var pages = source ? childrenOf(source) : [];
            if (pages.length) {
                var more = el('div', 'cx-rail-block cx-more');
                var title = el('div', 'cx-rail-title', 'In ' + linkOf(source).textContent.trim());
                more.appendChild(title);
                var ul = el('ul', 'cx-more-list');
                var shown = 0;
                var limit = 12;
                pages.forEach(function (li) {
                    var a = linkOf(li);
                    var item = el('li');
                    var link = el('a', '', a.textContent.trim());
                    link.href = a.getAttribute('href');
                    if (a === activeLink) { link.classList.add('is-current'); link.setAttribute('aria-current', 'page'); }
                    item.appendChild(link);
                    if (shown >= limit && a !== activeLink) { item.hidden = true; item.className = 'cx-more-extra'; }
                    ul.appendChild(item);
                    shown++;
                });
                more.appendChild(ul);
                if (pages.length > limit) {
                    var btn = el('button', 'cx-more-btn', 'Show all ' + pages.length);
                    btn.type = 'button';
                    btn.addEventListener('click', function () {
                        Array.prototype.forEach.call(ul.querySelectorAll('.cx-more-extra'), function (x) { x.hidden = false; });
                        btn.remove();
                    });
                    more.appendChild(btn);
                }
                if (source !== here) {
                    var up = el('a', 'cx-more-up');
                    up.href = linkOf(source).getAttribute('href');
                    up.innerHTML = '<span></span>' + icon('arrow');
                    up.firstChild.textContent = 'All of ' + linkOf(source).textContent.trim();
                    more.appendChild(up);
                }
                rail.appendChild(more);
            }
        }
        if (rail.children.length) { bodyGrid.appendChild(rail); bodyGrid.classList.add('has-rail'); }
    }
    // On a phone the side panel sits below the text, so the section list opens above it instead.
    if (article && otpLinks.length) {
        var box = el('details', 'cx-sections');
        var sum = el('summary', 'cx-sections-head');
        sum.innerHTML = icon('list') + '<span>On this page</span><span class="cx-sections-count">' + otpLinks.length + ' sections</span>';
        box.appendChild(sum);
        var list2 = el('ol', 'cx-sections-list');
        otpLinks.forEach(function (o) {
            var li = el('li');
            var a = el('a', '', textOf(o.h));
            a.href = '#' + o.h.id;
            a.addEventListener('click', function (e) {
                e.preventDefault();
                box.open = false;
                try { history.replaceState(null, '', '#' + o.h.id); } catch (err) { }
                var y = o.h.getBoundingClientRect().top + window.pageYOffset - topOffset() - 16;
                window.scrollTo(0, Math.max(0, y));
            });
            li.appendChild(a);
            list2.appendChild(li);
        });
        box.appendChild(list2);
        var heroIn = article.querySelector(':scope > .cx-hero');
        article.insertBefore(box, heroIn ? heroIn.nextSibling : article.firstChild);
    }
    if (spoilers.length) { setSpoilers(spoilersShown()); }

    function markOnThisPage(forced) {
        if (!otpLinks.length) { return; }
        var on = forced || null;
        if (!on) {
            var line = topOffset() + 120;
            otpLinks.forEach(function (o) {
                if (o.h.offsetParent !== null && o.h.getBoundingClientRect().top < line) { on = o.h; }
            });
        }
        otpLinks.forEach(function (o) { o.a.classList.toggle('is-on', o.h === on); });
    }
    var ticking = false;
    window.addEventListener('scroll', function () {
        if (ticking) { return; }
        ticking = true;
        window.requestAnimationFrame(function () { ticking = false; markOnThisPage(); });
    }, { passive: true });
    markOnThisPage();

    // ---- Dark and light ------------------------------------------------------------------------
    var themeBtn = document.getElementById('cx-theme');
    if (themeBtn) {
        themeBtn.addEventListener('click', function () {
            var light = !html.classList.contains('cx-light');
            html.classList.toggle('cx-light', light);
            html.classList.toggle('cx-dark', !light);
            save('codex-theme', light ? 'light' : 'dark');
        });
    }

    // ---- Phone menu -------------------------------------------------------------------------------
    var menuBtn = document.getElementById('cx-menu');
    function setNav(open) {
        html.classList.toggle('cx-nav-open', open);
        if (menuBtn) { menuBtn.setAttribute('aria-expanded', open ? 'true' : 'false'); }
    }
    if (menuBtn) { menuBtn.addEventListener('click', function () { setNav(!html.classList.contains('cx-nav-open')); }); }
    Array.prototype.forEach.call(document.querySelectorAll('[data-close-nav]'), function (b) {
        b.addEventListener('click', function () { setNav(false); });
    });
    document.addEventListener('keydown', function (e) {
        if (e.key === 'Escape' && html.classList.contains('cx-nav-open')) { setNav(false); }
        // A picture opened full size closes on Escape too.
        if (e.key === 'Escape') {
            Array.prototype.forEach.call(document.querySelectorAll('input.checkbox-img:checked'), function (c) { c.checked = false; });
        }
    });

    // ---- Search window --------------------------------------------------------------------------
    // mdBook's searcher.js does the searching; this opens it as a window instead of a bar.
    window.mdbook_something_else_has_focus = function (e) {
        var t = (e && e.target) || document.activeElement;
        return !!t && /^(?:input|select|textarea)$/i.test(t.nodeName);
    };
    var searchWrap = document.getElementById('mdbook-search-wrapper');
    var searchBtn = document.getElementById('mdbook-search-toggle');
    var searchBar = document.getElementById('mdbook-searchbar');
    if (searchWrap && searchBtn) {
        // searcher.js jumps to the top of the page when it opens; keep the reader's place.
        var realScrollTo = window.scrollTo.bind(window);
        var keepPlace = false;
        window.scrollTo = function (x, y) {
            if (keepPlace && x === 0 && y === 0) { keepPlace = false; return; }
            return realScrollTo.apply(null, arguments);
        };
        searchBtn.addEventListener('click', function () { keepPlace = true; setTimeout(function () { keepPlace = false; }, 0); }, true);
        document.addEventListener('keydown', function (e) {
            var open = !searchWrap.classList.contains('hidden');
            if ((e.ctrlKey || e.metaKey) && !e.altKey && (e.key === 'k' || e.key === 'K')) {
                e.preventDefault();
                if (!open) { searchBtn.click(); } else if (searchBar) { searchBar.focus(); searchBar.select(); }
                return;
            }
            if (!open && (e.key === 's' || e.key === '/') && !window.mdbook_something_else_has_focus(e)) {
                keepPlace = true;
                setTimeout(function () { keepPlace = false; }, 0);
            }
        }, true);
        Array.prototype.forEach.call(searchWrap.querySelectorAll('[data-close-search]'), function (b) {
            b.addEventListener('click', function () {
                if (!searchWrap.classList.contains('hidden')) { searchBtn.click(); }
            });
        });
        new MutationObserver(function () {
            var open = !searchWrap.classList.contains('hidden');
            html.classList.toggle('cx-search-open', open);
        }).observe(searchWrap, { attributes: true, attributeFilter: ['class'] });
    }
})();
