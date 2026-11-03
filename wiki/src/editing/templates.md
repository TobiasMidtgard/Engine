# Page templates

```meta
summary: Ready-made pages to copy: creature, city, district, place, planet, faction and more.
```

Copy a template into a new file and fill it in (see [How to add a page](how-to-add-a-page.md)).
The templates live in `wiki/templates/` in the repository. Every page of a kind keeps the same
sections in the same order, so readers always know where to look. Leave out a section you have
nothing for.

## Creature

````markdown
{{#include ../../templates/creature.md}}
````

## City

````markdown
{{#include ../../templates/city.md}}
````

## District

For a city with its own folder: `cities/<city>/<district>.md`.

````markdown
{{#include ../../templates/district.md}}
````

## Region or landmark

````markdown
{{#include ../../templates/place.md}}
````

## Planet

````markdown
{{#include ../../templates/planet.md}}
````

## Faction

````markdown
{{#include ../../templates/faction.md}}
````

## Person

````markdown
{{#include ../../templates/person.md}}
````

## Item

````markdown
{{#include ../../templates/item.md}}
````

## Spell

````markdown
{{#include ../../templates/spell.md}}
````

## Vehicle

````markdown
{{#include ../../templates/vehicle.md}}
````

## Any other page

````markdown
{{#include ../../templates/page.md}}
````
