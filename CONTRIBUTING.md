# Contributing to Partiworld

Thanks for wanting to help. Partiworld is a colony sim where the whole world is made of particles, and there is room for all kinds of help: code, new content and mods, pixel art, bug reports, ideas and docs.

By taking part you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md).

## Ways to help

- **Report a bug.** Open an issue with the *Bug report* form. The world seed and the steps that cause it help the most.
- **Suggest an idea.** Open an issue with the *Feature idea* form. Say what you would do in the game, not only what to build.
- **Add content.** New materials, plants, animals, reactions, god powers, tribes and biomes are mostly data files. See [Content and mods](#content-and-mods).
- **Draw.** Sprites are plain text grids, so pixel art can be added without touching Rust. See [Art](#art).
- **Write code.** Look for issues labelled `good first issue` or `help wanted`, or pick something from the [roadmap](README.md#roadmap).

For anything bigger than a small fix, please open an issue first (or comment on an existing one) so we can agree on the approach before you spend time on it.

## Getting set up

You need the latest stable [Rust](https://rustup.rs/).

On Linux, Bevy also needs a few system packages. On Debian or Ubuntu:

```bash
sudo apt install g++ pkg-config libx11-dev libasound2-dev libudev-dev libxkbcommon-x11-0 libwayland-dev libxkbcommon-dev
```

Then build and run:

```bash
git clone https://github.com/Fanaperana/partiworld
cd partiworld
cargo run --release -- 1337     # the number is the world seed
```

Run the tests before you open a pull request:

```bash
cargo test --workspace
```

The first build takes a few minutes.

## How the code is laid out

The [README](README.md#how-it-works) has a table of the crates. Two rules keep the project healthy:

- **The simulation doesn't depend on the game engine.** Only `pw_render` and `pw_ui` use Bevy. Everything else runs headless, which is how the tests play whole colonies. Keep game rules out of the render and UI crates.
- **Content is data.** If something could be a material, object, creature, reaction, power or tribe defined in `assets/mods/core`, make it data rather than hard-coding it in Rust. That way mods can change it too.

The simulation is deterministic for a given seed. Use the seeded RNG in `pw_core::rng`, never thread-local or time-based randomness, in anything that affects the world.

## Code style

- **Don't run `cargo fmt`.** The code isn't rustfmt-formatted, and running it rewraps thousands of lines, which buries your change in noise. Match the style of the code around your change by hand: long lines are fine (about 150 to 160 columns).
- Write short, plain comments that explain *why*, not *what*. Doc comments (`///`) on anything public.
- Keep functions focused, and prefer small, readable code over clever code.
- Add or update a test when you change behaviour. Tests in `crates/pw_game/tests/game.rs` show how to drive a whole colony headless.
- Fix any warnings your change introduces (`cargo build` should be clean).

## Content and mods

The base game is itself a mod in `assets/mods/core`. The [Modding section of the README](README.md#modding) explains every kind of file.

- Ids are namespaced: `core:` for the base game, your mod's id for yours.
- Prefer adding to the data over adding special cases in Rust. If the data format can't express what you need, a small, general addition to the format is better than a one-off.
- `assets/mods/example_crystals` and `assets/mods/example_events` are complete small mods to copy from.
- With live reload on (Tools > World), you can edit `.ron` and `.rhai` files while the game runs and see the change straight away.

## Art

All art is drawn from particle data; there are no image files in the game. Characters and animals are text grids in `assets/mods/core/creatures/*.ron`:

- `palette` maps a character to a colour (`"#rrggbb"`) or to a colour slot like `"$skin"` that each individual fills in. Add `-` or `+` for a shadow or highlight tone of a slot: `"$skin-"`, `"$hair+"`.
- `rows` is the front view. `views` holds the other facings (`"se"`, `"e"`, `"ne"`, `"n"`, `"nw"`, `"w"`, `"sw"`); west-facing views are mirrored from east ones when left out.
- `poses` holds action poses (`idle`, `run`, `leap`, `work`, `carry`, `eat`, `graze`, `sleep`, `attack`, `hurt` and more), drawn facing east.
- `outline: false` turns off the automatic outline for art that draws its own.

Concept sheets the current sprites were made from live in `docs/art/`. Lighting comes from the top-left, so keep highlights on that side.

Trees, bushes and rocks are particles too, shaped by the `Procedural` primitives in `objects/*.ron` and shaded in `pw_render`.

To check how something looks, `PW_AUTOSHOT=shot.png cargo run --release` plays a short scripted session and saves a screenshot. The README lists the [settings](README.md#tests) for time of day, weather and more.

## Pull requests

1. Fork the repository and create a branch from `main`.
2. Keep each pull request to one change. Several small pull requests are easier to review than one big one.
3. Write commit messages with a short summary line in the present tense ("Add peat that smoulders in the rain"), then a blank line and a few lines on why, if it isn't obvious.
4. Run `cargo test --workspace` and make sure it passes.
5. Add a screenshot or short clip to the pull request if your change shows on screen.
6. Update the README if you change controls, add a feature players will see, or change the modding format.

A maintainer will review your pull request. Please be patient; this is a project run in spare time.

## Security

Please don't report security problems in public issues. See [SECURITY.md](SECURITY.md).

## License

Partiworld is dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE). Unless you say otherwise, anything you contribute is dual-licensed the same way.
