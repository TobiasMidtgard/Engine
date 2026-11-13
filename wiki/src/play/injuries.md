# Damage, armour and injuries

```infobox
summary: Every blow lands on a part of the body. What it hits, what it is made of and what you are wearing decide whether it bruises, cuts, cripples or kills.
status: draft
tags: Combat, Numbers
Type: Mechanic
Damage types: 3 physical, 9 magical
Body zones: 10 on each side of the body
Treated by: Bandages, splints, healers, Vitalism, potions
See also: [Combat](combat.md), [Status effects](status-effects.md)
```

Combat in this world does not take health off a bar and nothing else. Every hit lands somewhere:
on a helmet, in the gap under an arm, across the back of a knee. Your health is how much punishment
you can take in total, but each part of the body can be hurt on its own, and a hurt part works
worse until it is treated.

## Damage types

| Type | Comes from | Good against | Poor against |
|---|---|---|---|
| **Cut** | Swords, axes, claws, glass | Unarmoured flesh, cloth, rope | Mail and plate |
| **Pierce** | Spears, daggers, arrows, bullets, bites | The gaps in armour, mail | Thick plate faced square on |
| **Crush** | Maces, hammers, shields, falls, fists | Plate (the blow goes through), bone, poise | Padding, soft targets |
| **Fire** | Evocation, torches, burning oil | Wood, cloth, the undead, beasts that fear it | Fire elementals, the Lady in the Furnace's own |
| **Frost** | Evocation, ice | Slowing anything, water creatures | The cold-blooded north |
| **Storm** | Evocation, Sky City's crowns | Metal armour (it conducts), anything wet | Stone, the grounded |
| **Blight** | Malefice, poisons, rot | The living | Undead, constructs |
| **Blood** | Malefice, vampires | Anyone who bleeds | Constructs, spirits |
| **Shadow** | Malefice, Pactbinding | The mind and the unwarded | Bright light |
| **Radiance** | Vitalism (Radiance), sunlight | The undead, vampires, demons | The living (it barely warms them) |
| **Force** | Shaping, falls, blasts | Shields, poise, things in the air | Anchored, heavy targets |
| **Mind** | Illusion, Domination | Concentration, morale | Constructs, the mindless dead |

## Armour

Armour has a **rating** for each zone it covers, and a **ward** if it was enchanted or blessed.
Rating reduces physical damage, and ward reduces magical damage. Neither works on what it doesn't
cover: a breastplate does nothing for an arrow in the thigh.

| Armour | Rating | Poise | Noise | Weight (full set) |
|---|---|---|---|---|
| Clothes | 0 to 2 | 0 | Silent | 2 to 4 kg |
| Padded and leather | 4 to 8 | +5 | Quiet | 6 to 10 kg |
| Mail | 10 to 16 | +15 | Jingles | 12 to 18 kg |
| Plate | 18 to 30 | +30 | Loud | 22 to 30 kg |
| Hellplate, force armour | 25 to 40 | +40 | Depends on the caster | None: it is a spell |

How armour treats each type:

- **Cut** loses the whole rating, so a sword against plate does very little.
- **Pierce** loses half the rating at the gaps (armpit, groin, visor, behind the knee) and the
  whole rating everywhere else. Hitting the gaps takes skill and Perception; see
  [look-to-aim](combat.md#aiming-at-the-body).
- **Crush** loses a quarter of the rating, but plate rings and the wearer still loses poise. This
  is why knights fear maces.

Armour wears: each hit takes a little condition off the piece that stopped it, and a smith or a
Mending spell restores it.

## Body zones

| Zone | Damage taken | When it is injured | When it is crippled |
|---|---|---|---|
| **Head** | ×2 | Blurred sight, ringing ears, spells harder to hold | Knocked out, or dazed and stumbling |
| **Neck** | ×2.5 | Bleeding fast | Lethal without help. Everyone guards it, and the best fighters guard it best. |
| **Chest** | ×1 | Stamina recovers slower | Short of breath: no sprinting |
| **Belly** | ×1.2 | Bleeding | Doubled over: heavy attacks impossible |
| **Upper arm** | ×0.7 | Weaker swings with that arm | Can't raise it: no shield, no two-handed grip |
| **Forearm** | ×0.6 | Slower attacks | Grip fails |
| **Hand** | ×0.5 | Fumbles: slower locks and reloads | Drops what it holds |
| **Thigh** | ×0.9 | Slower running | Limping |
| **Knee** | ×0.8 | Can't sprint | The joint locks: you walk stiff-legged and can't climb |
| **Shin and foot** | ×0.7 | Slower turning | Hobbling |

A zone is **injured** when the damage it has taken passes a quarter of your health, and
**crippled** when it passes half. A deep enough cut to a limb can **sever** it. A severed hand
can't be reattached by bandages; only master healing, Vitalism's Regrow, or a Necromancer's bone
graft gives it back.

### Losing a hand

You can fight with either hand, but the hand you trained fights best. Losing the use of your main
hand moves the weapon to the other one, at the skill your **Off hand** has reached (see
[Skills](skills.md#combat-skills)). Train the off hand before you need it.

## Bleeding and treatment

Cuts and pierces to the neck, belly and limbs **bleed**: a little health every second until the
wound is bound. Injuries don't heal on their own past a scratch. They need treatment:

| Treatment | What it does | Where |
|---|---|---|
| **Bandage** | Stops bleeding | Anyone with cloth |
| **Splint** | Unlocks a locked knee or a crippled limb for a day | Anyone with a splint kit |
| **Healer's visit** | Treats every injury, for a price | Healers in every city; the wound-houses of the Covenant of Ash in Orath |
| **Healing potions** | Restore health; the good ones close one injury | Alchemists, or brew your own |
| **Vitalism** | Mend closes injuries; Regrow restores lost limbs | Restoration branch (see [Vitalism](../magic/vitalism.md)) |
| **Necromancy** | Bone graft and drain-to-heal: fast, ugly and corrupting | See [Necromancy](../magic/necromancy.md) |

## Scars

An injury that heals without a healer, or a severed limb that was regrown, leaves a **scar**, which
you can keep or have removed. Some titles need them (see [Titles](titles.md)). A few people in the
world will read your scars and know where you have been.

## Your enemies have bodies too

Everything here works on the people and creatures you fight. Cripple a guard's knee and he can't
chase you up a ladder; break a swordsman's hand and his sword falls; blind a wolf and the pack
hesitates. On the harder [difficulties](difficulty.md), your enemies aim for your weak points as
carefully as you aim for theirs.
