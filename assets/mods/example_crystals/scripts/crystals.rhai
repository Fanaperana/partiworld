// Felling a crystal tree scatters glowing sparks on the ground and leaves shards.
fn shatter(ctx, x, y) {
    for i in 0..40 {
        let px = x + ctx.rand_int(-8, 8);
        let py = y + ctx.rand_int(-8, 8);
        if ctx.get(px, py, "solid") == "none" {
            ctx.set(px, py, "floor", "example_crystals:crystal_spark");
        }
    }
    let n = ctx.rand_int(2, 4);
    ctx.spawn_item(x, y, "example_crystals:shard", n);
    ctx.message("A crystal tree shatters into " + n + " shards.");
}
