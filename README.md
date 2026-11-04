# Partiworld

**A top-down colony sim where the whole world is made of particles.**

Every tree, rock, wall, roof and patch of grass is a group of tiny particles. You can chop a tree and watch it wear away bit by bit, mine a hillside cell by cell, or set a dry meadow alight and watch the wind carry the fire. You play it like RimWorld: mark the work, plan the buildings, and your colonists get on with it. The world is generated from a seed, it goes on forever, and it changes with the seasons.

Built in Rust with [Bevy](https://bevyengine.org/). Everything is moddable with data files and [Rhai](https://rhai.rs/) scripts.

![Autumn evening in the forest](docs/screenshots/autumn.png)

<table>
  <tr>
    <td><img src="docs/screenshots/winter.png" alt="Snow, bare trees and evergreens in winter"></td>
    <td><img src="docs/screenshots/storm.png" alt="A thunderstorm at dawn"></td>
  </tr>
  <tr>
    <td align="center"><sub>Winter: snow cover, bare oaks, green pines</sub></td>
    <td align="center"><sub>A thunderstorm rolling in at dawn</sub></td>
  </tr>
  <tr>
    <td><img src="docs/screenshots/building.png" alt="Colonists building a wooden house"></td>
    <td><img src="docs/screenshots/see-through-roof.png" alt="A roof fading to show the colonist underneath"></td>
  </tr>
  <tr>
    <td align="center"><sub>Colonists hauling wood and raising walls</sub></td>
    <td align="center"><sub>Roofs fade so you can see who's inside</sub></td>
  </tr>
</table>

## Play

You need the latest stable [Rust](https://rustup.rs/). Then:

```bash
git clone https://github.com/Fanaperana/partiworld
cd partiworld
cargo run --release -- 1337
```

The number is the world seed; leave it out for the default world. The first build takes a few minutes.

## What you can do

**Run a colony**
- Mark trees to chop, rocks to break, berries to harvest and hills to mine. Colonists walk there, work the particles away, and carry the results to your stockpiles.
- Plan walls, floors, doors and roofs. Builders fetch a full load of materials and drop it at several blueprints in one trip.
- Set each colonist's work priorities (1 = first, 4 = last, blank = never) so one is a builder and another a woodcutter.
- Plant saplings and watch them grow into trees.

**A world that follows its own rules**
- **Roofs need support.** A roof must be within 6 tiles of a wall or natural rock. Knock out the wall and the roof caves in.
- **Fire is physics, not an animation.** It spreads particle by particle through anything flammable. Dry grass goes up fast, green lawns barely burn. Wind pushes it downwind and throws embers ahead. Rain and snow slow it down and put it out, except under a roof.
- **Seasons change the world.** Spring, summer, autumn and winter each have their own temperature, day length and weather. Leaves turn orange and fall, pines stay green, snow piles up and melts, ponds freeze.
- **Weather:** clear skies, overcast, wind, fog, rain, thunderstorms, snow and blizzards, with temperature and wind shown on the top bar.
- **Day and night:** dawn glow, bright days, orange dusks, dark nights.
- **Plants follow the climate.** Growth speeds up in spring and stops in the frost. Grass only spreads when it's warm.

**Play god**
- Paint any material, drop any object, start fires, add colonists.
- Draw your own trees, props and buildings in the **object editor**. They're saved as a mod and work in the game right away.

## Controls

| Input | Action |
|---|---|
| WASD / arrows, middle-drag | Move the camera (Shift = faster) |
| Scroll, Q / E | Zoom |
| Space, 1 / 2 / 3 | Pause, game speed |
| Left drag | Use the selected tool on an area |
| Right click, Esc | Back to Inspect |
| C / M / G / X | Chop, mine, harvest, cancel orders |
| Z / B / T | Stockpile zone, build (last pick), plant (last pick) |
| Click a colonist, F | Select, follow with the camera |
| P | Work priorities |
| R / V | Hide roofs and tree tops / see-through roofs |
| F5 / F9 | Quick save / load |
| H | Help |

## How it works

The world is a grid of particles, 8×8 per tile, in 256×256-particle chunks. Each cell has three layers:

| Layer | Holds | Example |
|---|---|---|
| **Floor** | what you walk on | grass, sand, water, wooden floor |
| **Solid** | what blocks you | tree trunks, rock, walls |
| **Overlay** | what's above you | tree canopies, roofs |

Objects (trees, bushes, walls) are groups of particles that remember which object they belong to. Remove enough of them and the object breaks and drops its items. Colonists don't walk on particles; they walk on a coarse tile grid that's rebuilt from the particles whenever something changes.

The simulation has no dependency on the game engine. It runs headless, which is how the tests drive whole colonies. The code is split into small crates:

| Crate | Job |
|---|---|
| `pw_core` | Coordinates, seeded RNG, data definitions, registries, linking string ids to fast numeric ids |
| `pw_mods` | Finds mods, sorts them by dependency, loads RON data, applies JSON patches, compiles Rhai scripts |
| `pw_world` | Chunks, the three particle layers, object instances, the tile grid |
| `pw_objects` | Stamping objects, wearing them down, breaking, growth, roof support |
| `pw_worldgen` | Seeded noise to biomes, ground, mountains, ore veins and scattered plants, generated on background threads |
| `pw_sim` | Particle reactions from data (fire, grass, ash), affected by wind, rain and temperature |
| `pw_colony` | Colonists, pathfinding, orders, jobs, hauling, stockpiles, construction |
| `pw_game` | The tick loop, seasons and weather, chunk streaming, player tools, script API, Rust plugin hooks |
| `pw_save` | Saves only the chunks you changed; content is saved by id so saves survive mod changes |
| `pw_render` | Chunk textures, sprites, lighting and weather effects, all drawn from particle data (no sprite files) |
| `pw_ui` | Panels, tools and the object editor ([egui](https://github.com/emilk/egui)) |

## Modding

The base game is itself a mod (`assets/mods/core`). A mod is a folder in `assets/mods/` with a `mod.toml`; click **New world** in game to reload every mod from disk.

```
assets/mods/my_mod/
  mod.toml          id, name, version, depends = ["core"]
  materials/*.ron   colour, tags (flammable, support, leaves...), blocks, walk cost, yields
  items/*.ron
  objects/*.ron     painted patterns or procedural shapes (circle, blob, rect, scatter)
  biomes/*.ron      climate ranges, ground mix, mountains, ore veins, what grows there
  reactions/*.ron   particle rules: input, neighbour, chance, effects, weather
  jobs/*.ron        new orders = a built-in behaviour + target tags
  creatures/*.ron
  climate.ron       seasons, day length, temperatures, weather
  patches/*.json    change other mods' data ("set" / "append")
  scripts/*.rhai    hooks like fn on_destroyed(ctx, x, y)
```

A whole new material and how it burns, with no Rust:

```ron
// materials/peat.ron
MaterialDef(id: "my_mod:peat", name: "Peat", color: "#4a3a28", variance: 0.2,
    tags: ["soil", "flammable"])

// reactions/peat.ron: smoulders slowly, follows the wind, rain barely stops it
ReactionDef(id: "my_mod:peat_smoulder", layer: Floor, input: "core:fire",
    windy: true, wet: 0.7, chance: 0.05,
    neighbor: NeighborDef(layer: Floor, matches: "my_mod:peat"),
    effects: [EffectDef(target: Other, set: "core:fire")])
```

- Using an existing id replaces that definition.
- `assets/mods/example_crystals` is a complete mod: a new material, item and glowing tree, a patch that adds the tree to forests, and a script that scatters crystal shards when one is felled.
- Rust mods implement `pw_game::PwPlugin` to add per-tick systems and new script functions (see `DayAnnouncer` in `src/main.rs`).

## Tests

```bash
cargo test --workspace
```

The tests cover:
- mod loading, patching and dependency order
- deterministic world generation, and objects across chunk borders
- pathfinding
- chopping, harvesting, mining, hauling and building a room
- work priorities
- fire, rain and wind
- roof support and collapse
- seasons and weather
- plant growth, scripts and save/load

Tools for testing:
- **Busy-colony benchmark:** `cargo test --release -p pw_game --test game bench -- --ignored --nocapture` times a busy colony.
- **Scripted screenshot:** `PW_AUTOSHOT=shot.png cargo run --release` plays a short scripted session, saves a screenshot and quits. You can set it up with these variables:
  - `PW_AUTOSHOT_SECS`: when to take the screenshot
  - `PW_AUTOSHOT_DAY` and `PW_AUTOSHOT_HOUR`: when the game starts
  - `PW_AUTOSHOT_WEATHER`: the weather, e.g. `core:snow`
  - `PW_AUTOSHOT_ROOF=1`: a finished roof with a colonist under it
  - `PW_AUTOSHOT_SPEED`: game speed
- **Frame times:** `PW_FRAMES=1` prints frame timings.

## Roadmap

Next on the list: colonist needs (food, sleep, warmth), crafting, farming zones, flowing water, animals.

## License

Dual-licensed under either of

- [MIT license](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

at your option. Unless you say otherwise, any contribution you submit is dual-licensed the same way.
