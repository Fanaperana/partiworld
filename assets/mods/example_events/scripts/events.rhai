// World events: the game calls these by name. No Rust needed.
//   on_hour(ctx)            every in-game hour
//   on_day(ctx, day)        at the start of each day
//   on_season(ctx, season)  when the season changes
//   on_weather(ctx, id)     when the weather changes
//   on_new_species(ctx, name), on_extinct(ctx, name)
//   on_tribe_founded(ctx, name), on_discovery(ctx, tribe, tech), on_tribe_lost(ctx, name)
//   ctx.count("tribes") is how many tribes are alive.

// A random spot `min`..`max` particles away from the colony.
fn near_colony(ctx, min, max) {
    let a = ctx.rand() * 6.2831;
    let d = min + ctx.rand() * (max - min);
    [ctx.colony_x() + (d * a.cos()).to_int(), ctx.colony_y() + (d * a.sin()).to_int()]
}

fn on_hour(ctx) {
    // Thunderstorms throw real lightning; it can start wildfires in dry grass.
    if ctx.weather() == "core:storm" && ctx.rand() < 0.5 {
        let p = near_colony(ctx, 60, 320);
        ctx.power("core:lightning", p[0], p[1], 2);
    }
    // Clear summer nights: now and then a meteor comes down far away.
    let h = ctx.hour();
    if ctx.season() == "Summer" && ctx.weather() == "core:clear" && (h > 22.0 || h < 3.0) && ctx.rand() < 0.03 {
        let p = near_colony(ctx, 280, 420);
        ctx.power("core:meteor", p[0], p[1], 12);
        ctx.message("A meteor streaks across the night sky and crashes in the distance.");
    }
}

fn on_season(ctx, season) {
    if season == "Spring" { ctx.message("Spring: the ice breaks up and everything starts to grow."); }
    if season == "Winter" { ctx.message("Winter: water freezes and plants stop growing. Animals that can't take the cold will struggle."); }
}

fn on_weather(ctx, id) {
    if id == "core:storm" { ctx.message("A thunderstorm rolls in. Watch for lightning fires."); }
}

// Tribes: a word about each new village and discovery.
fn on_discovery(ctx, tribe, tech) {
    if tech == "Masonry" { ctx.message("The " + tribe + " have learned to build in stone. Their houses will stand through fire."); }
}

fn on_tribe_lost(ctx, name) {
    // When the last tribe dies out, new people wander in after a while.
    if ctx.count("tribes") == 0 && ctx.rand() < 0.5 {
        let p = near_colony(ctx, 360, 480);
        ctx.power("core:found_tribe", p[0], p[1], 4);
        ctx.message("Wanderers from far away settle where the " + name + " once lived.");
    }
}
