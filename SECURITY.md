# Security policy

## Supported versions

Partiworld is in early development. Only the latest code on the `main` branch gets fixes.

## Reporting a problem

Please don't report security problems in public issues.

Report them privately through GitHub instead: open the [Security tab](https://github.com/Fanaperana/partiworld/security) and choose **Report a vulnerability**. Only the maintainers can see the report.

Useful things to include:

- what the problem is and what someone could do with it
- the steps or a file (for example a save, mod or script) that shows it
- the version or commit you tested

You should hear back within a week. Once a fix is ready, we'll credit you in the release notes unless you'd rather stay anonymous.

## What counts

Partiworld loads mods, Rhai scripts and save files from disk, so the most likely problems are:

- a mod, script or save file that can read or write files outside the game's folders, run programs, or reach the network
- a mod, script or save file that crashes the game or hangs it in a way a player can't recover from by removing it

Bugs that only affect gameplay are welcome as normal [issues](https://github.com/Fanaperana/partiworld/issues).
