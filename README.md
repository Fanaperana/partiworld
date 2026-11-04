# Partiworld

A top-down colony sim where everything is made of particles. Trees, rocks, walls and floors are groups of particles you can chop, mine, burn and build, cell by cell. You play it like RimWorld: give orders, and colonists do the work.

```bash
cargo run --release -- 1337
```

The number is the world seed (optional).

## Controls

| Input | Action |
|---|---|
| WASD / arrows, middle-drag | Move camera (Shift = faster) |
| Scroll, Q / E | Zoom |
| Space, 1 / 2 / 3 | Pause, game speed |
| Left drag | Use the selected tool on an area |
| Right click, Esc | Back to Inspect |
| R | Hide canopies and roofs |
| F5 / F9 | Quick save / load (`saves/quicksave.pws`) |

Tools (left panel): **Orders** (chop, cut, harvest, mine, break rocks, deconstruct), **Zones** (stockpiles), **Build** (walls drag as a room outline), **Plant** (saplings grow into trees), **God powers** (paint any material, drop any object, start fires, add colonists) and the **Object editor**.

## How it is built

Each crate is small and has one job. Only `pw_render`, `pw_ui` and the binary depend on Bevy; the simulation runs headless (that is how the tests run it).

| Crate | Job |
|---|---|
| `pw_core` | Coordinates, seeded RNG, data definitions, registries, `Content::link` (string ids to numeric ids) |
| `pw_mods` | Finds mods, sorts them by dependency, loads RON defs, applies JSON patches, compiles Rhai scripts |
| `pw_world` | 256x256-particle chunks with three layers (floor, solid, overlay), object instances, tile grid for pathing |
| `pw_objects` | Stamping particle-group objects, wearing them down, breaking, growth and regrowth |
| `pw_worldgen` | Seeded noise to biomes, floors, mountains and ore veins; deterministic object scatter across chunk borders; background generation threads |
| `pw_sim` | Particle reactions from data (fire, grass regrowth, ash) |
| `pw_colony` | Colonists, A* pathfinding, designations, jobs, items, stockpiles, blueprints |
| `pw_game` | `Game`: tick loop, chunk streaming, player tools, Rhai API, Rust plugin hooks |
| `pw_save` | Saves only changed chunks (lz4), content stored by id so saves survive mod changes |
| `pw_render` | Chunk textures and sprites, all generated from particle data |
| `pw_ui` | egui panels, mouse tools, object editor |

Scale: 8x8 particles make one tile; one colonist is one tile. The particle grid is what you see and what reacts; the tile grid is what colonists walk on.

## Modding

A mod is a folder in `assets/mods/` with a `mod.toml`. The base game is itself the `core` mod. Click **New world** in game to reload all mods from disk.

```
assets/mods/my_mod/
  mod.toml          id, name, version, depends = ["core"]
  materials/*.ron   colour, tags (flammable, mineable, soil...), blocks, walk_cost, yields
  items/*.ron
  objects/*.ron     painted Pattern or Procedural shape (Circle, Blob, Rect, Scatter)
  biomes/*.ron      climate ranges, floor mix, mountains, ore veins, object scatter
  reactions/*.ron   particle rules: input, neighbour, chance, effects
  jobs/*.ron        new orders = a built-in driver + target tags
  creatures/*.ron
  patches/*.json    change fields of other mods' defs ("set" / "append")
  scripts/*.rhai    hooks: fn name(ctx, x, y), named in an object's `hooks`
```

- Same id as an existing def = override it.
- `assets/mods/example_crystals` shows a full mod with no Rust: new material, item and tree, a patch that adds the tree to forests, and a script that drops shards when one is felled.
- Rust mods implement `pw_game::PwPlugin` to add per-tick systems and new script functions (see `DayAnnouncer` in `src/main.rs`).
- The in-game object editor saves to `assets/mods/user/objects/` and adds the object to the running game.

## Tests

```bash
cargo test --workspace
```

Covers mod loading and patching, dependency sorting, deterministic world generation, objects across chunk borders, pathfinding, chopping, harvesting, mining, building, hauling, fire, growth, scripts, and save/load.

`PW_AUTOSHOT=shot.png cargo run` runs a short scripted session, saves a screenshot and quits.
