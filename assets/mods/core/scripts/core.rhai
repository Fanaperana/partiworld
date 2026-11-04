// Core script hooks. A hook is fn name(ctx, x, y) and is named in an
// object's `hooks` map. See crates/pw_game/src/script.rs for the ctx API.

// A felled tree drops leaf litter around its stump.
fn tree_fall(ctx, x, y) {
    for i in 0..90 {
        let px = x + ctx.rand_int(-11, 11);
        let py = y + ctx.rand_int(-11, 11);
        if ctx.has_tag(px, py, "floor", "soil") && ctx.get(px, py, "solid") == "none" {
            ctx.set(px, py, "floor", "core:leaf_litter");
        }
    }
}

fn campfire_lit(ctx, x, y) {
    ctx.message("The campfire crackles to life.");
}
