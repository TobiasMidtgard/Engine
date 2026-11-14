# How to add a page

```meta
summary: Add or change a page in two minutes, from a phone or a computer.
```

Every page of this wiki is a short text file in the game's repository on GitHub, in the folder
`wiki/src/`. Change a file on the `main` branch and the website updates by itself about a
minute later.

## Change a page

1. Open the page on the wiki and tap the **pencil** at the top right, beside **History**. It
   opens the page's file on GitHub.
2. Tap the pencil on GitHub (**Edit this file**), make your change and tap
   **Commit changes**.

## Add a page

1. Pick the section it belongs in. Each section is a folder:

   | Section | Folder |
   |---|---|
   | Creatures | `wiki/src/creatures/` |
   | Cities and towns | `wiki/src/cities/` |
   | Factions | `wiki/src/factions/` |
   | Characters | `wiki/src/people/` |
   | Quests | `wiki/src/quests/` |
   | Planets | `wiki/src/world/planets/` |
   | Regions | `wiki/src/world/regions/` |
   | Landmarks | `wiki/src/world/landmarks/` |
   | Items | `wiki/src/items/` |
   | Vehicles | `wiki/src/vehicles/` |

2. On GitHub, open that folder and choose **Add file**, then **Create new file**.
3. Name the file in lowercase with dashes and `.md` at the end: `frost-wyrm.md`.
4. Copy the right [template](templates.md) into it, fill in what you know and delete the rest.
5. Tap **Commit changes**. The page appears in the sidebar under its section, in alphabetical
   order, and gets a card on the section's page. Nothing else to edit.

A big city can have a folder of its own: `cities/vantress/index.md` for the city and one file
per district beside it (`cities/vantress/knot-square.md`). The districts show up under the city.

## Pictures

Put pictures in `wiki/src/images/`, in the folder of their section (`images/creatures/`).
Landscape JPEGs about 1280 pixels wide and under 300 KB look best. Only use pictures made for
the game: screenshots, concept art and renders, never pictures from the internet.

In an infobox, write the picture's path from the `images` folder:

```text
image: creatures/frost-wyrm.jpg
```

Anywhere else in a page, use a normal Markdown picture, with a path from the page's folder:

```text
![A frost wyrm diving from the clouds](../images/creatures/frost-wyrm.jpg)
```

A `gallery` block puts several pictures side by side, each with a caption under it. Paths are
from the `images` folder, like the infobox; `wide: yes` gives each picture the full width (for
maps). Every picture opens full size when clicked.

````text
```gallery
cities/keth-veyra-dam.jpg | The dam face from below
creatures/gate-ox.jpg | A gate ox on a dam terrace
```
````

## The infobox

The **At a glance** panel beside the page (above the text on a phone). One `Key: value` per
line; a line starting with `##` starts a new group.

````text
```infobox
image: creatures/wolf.jpg
caption: A wolf stalking through the grass.
summary: One sentence for the card on the section page.
status: draft
tags: Pack hunter, Night
Type: Beast
Danger: 3/5
## Combat
Attacks: Bite, lunge
```
````

- `summary` is the line under the page's title, and the text on its card.
- `image` is the big picture behind the title, and the picture on its card. Leave it out and the
  title gets its section's emblem instead.
- `caption` is the small text on the picture.
- `Type` and `tags` (a comma-separated list) become the small chips under the title.
- `status` marks the page as a draft or a stub (see below).
- Any other key becomes a row of the panel, in the order you write them.
- A value like `3/5` shows as dots, for danger or difficulty.
- Links work inside values: `[Home world](../world/planets/home-world.md)`.
- `map: 48.5, 46` puts the place on the [world map](../world/index.md#map) (percent from the
  map's left and top edges) and shows a small map with a pin at the bottom of the panel. Cities
  show as pins, regions as names and dungeons as diamonds.

## Draft and stub

Mark a page that isn't settled yet, so readers know:

| Write | Shows |
|---|---|
| `status: draft` in the infobox, or `{{draft}}` on its own line | A **Draft** chip by the title: names and details may change |
| `status: stub`, or `{{stub}}` | A **Stub** chip: the page is only a start |
| `{{draft: your own note}}` | The chip, and your own words under the title |

Remove the line when the page is right. [Pages to review](review.md) lists every page still
marked.

## Easy to read

The wiki breaks long pages up by itself, so write plain sections and let it do the rest:

- **On this page**: every `##` section is listed in the panel beside the text, and clicking one
  jumps to it.
- **Cards**: a list where every line starts with a bold name, like `- **The Iron Anchor**:
  soldiers and engineers`, shows as a grid of small cards.
- **Quotes**: a one-paragraph `>` quote shows large, as a quote.
- **Spoilers**: start a section's secret part with these two lines, and the rest of the section
  is hidden behind a **Show** cover. Readers can show every spoiler at once with the switch beside
  the page.

  ```text
  > [!WARNING]
  > Spoilers for the guild questline below.
  ```

- **Quest routes**: a `flow` block draws how a quest can go: where it starts, the ways in side by
  side, where they meet and how it can end. The [quest template](templates.md#quest) has one.

## Charts

Numbers read faster as pictures. Each block below is drawn as a chart; the text stays in the
page, so search still finds it.

| Block | Lines | Draws |
|---|---|---|
| `levels` | `Name \| 10-25 \| note` | Bars on the 1 to 100 level scale, over the Human, Awakened and Ascendant bands. One level is a dot; `61+` runs off the end. |
| `bars` | `Name \| 45 \| note` | Horizontal bars from one baseline. `max:` fixes the scale, `unit:` follows each value, `scale: log` makes each tenfold step the same length, and `show: note` prints the note at the bar's tip instead of the number. |
| `stats` | `Health \| 900` | Big number tiles, for a boss or a creature; long values are listed under them |
| `tiers` | `Name \| what it means \| small label` | A staircase, lowest first: ranks, stages, coins |
| `timeline` | `When \| what happened` | Events down a line; `## Age \| when` starts an age |

Every block takes a `title:` line, and all but `stats` a `note:` line under the chart. A line
starting with `##` in `levels` and `bars` starts a group. `{{levels}}` on a section page charts
the section's pages by their infobox `Level` or `Levels` row; `{{worldmap}}` draws the world map
with every page that has a `map:` row.

````text
```bars
title: Range
unit: m
Short bow | 60 | 20 damage
Longbow | 120 | 30 damage
```
````

## Section pages

A section's own page (its `index.md`) can show its pages for you:

| Write | Shows |
|---|---|
| `{{cards}}` | A picture card for every page in the section |
| `{{table: Type, Habitat, Danger}}` | A table of the section's pages with those infobox rows |
| A `cards` block with one page path per line | Cards for just those pages, from anywhere in the wiki |
| A `list` block with one page path per line | The same pages as rows with an icon and their summary |

A `meta` block sets the picture and text of a page's own card without showing an infobox:

````text
```meta
image: world/valley-from-the-air.jpg
summary: Planets, regions and landmarks.
```
````

## Links

Link to another page with its path from the current file:
`[wolves](../creatures/wolf.md)`. Link the first time you mention something, not every time.

## On a computer

If you work from a clone of the repository, preview the wiki as you write:

```text
mdbook serve wiki --open
```

This needs [mdBook](https://rust-lang.github.io/mdBook/) 0.5 and Python 3. Running
`sh wiki/tools/fetch-fonts.sh` first gets the wiki's fonts; without them it uses your system's.
`python3 wiki/tools/wiki.py check` lists broken links and missing pictures. Push to `main` and the
site updates.
