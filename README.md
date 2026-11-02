# <img src="assets/pot.ico" width="50" /> `stove` - an editor for cooked unreal engine maps

> a fork of [bananaturtlesandwich/stove](https://github.com/bananaturtlesandwich/stove) that adds support for unreal engine 5.3 to 5.8. test builds are on the [releases page](https://github.com/Marduc666/stove/releases) or you can [build it yourself](#building) - the [original releases](https://github.com/bananaturtlesandwich/stove/releases) stop at 5.2

<h1 align="center"><img width=900 src=https://github.com/user-attachments/assets/a480a151-c6c1-4d27-a78a-af0bdccdd754></h1>

# features
- visualise maps as they would be in-game
- edit actor properties and transforms
- duplicate and delete actors
- transplant actors from other maps (including your own!)

<details>
<summary><h1>roadmap</h1></summary>

### basic functionality
- [x] save and open unreal map files of any version
- [x] display a selectable list of actors
- [x] allow editing all of an actor's transforms
- [x] render each actor as a cube/sprite in a 3d scene
- [x] walk around the scene with an unreal-editor-style camera
- [x] duplicate actors in the same map
- [x] transplant actors from a different map
- [x] edit the properties of actors and their components
### convenience
- [ ] undo and redo
- [x] actor deletion
- [x] move actors in the viewport
- [x] searching
### advanced functionality
- [ ] insert default values (properties left as default are cut from the map)
- [ ] delete excess exports left after actor removal
- [x] duplicate and transplant all actor types (not sure why some don't work)
### aesthetic
- [x] load assets from pak folders
- [x] retrieve and display static meshes
- [ ] retrieve and display skeletal meshes
- [ ] get meshes for all types of actor
- [x] retrieve and display textures
- [ ] parse materials properly
- [x] discord RPC (show your internet friends what you're doing)
</details>

# unreal engine 5.3 and newer

cooked ue5 games need a little setup as their maps don't contain any type information:

1. extract the game with [retoc](https://github.com/trumank/retoc) `to-legacy` if it uses iostore (`.utoc`/`.ucas`), keeping the folder structure
2. select the engine version in stove
3. select the game's `.usmap` next to the engine version
4. add the extracted folder under `content` and select it, meshes and textures are read from there
5. open a map from the extracted folder

blueprints and textures missing from the map are read from their own packages, so the whole game should be extracted and not only its maps.

some 5.8 games like the last caretaker use an expanded import layout, which stove detects by itself but needs [ihsoft's retoc fork](https://github.com/ihsoft/retoc) with `--version UE5_8` to extract.

### limitations
- only unversioned packages are supported from 5.4 onwards, which is what games ship
- exports that can't be read are kept as they are when saving but won't show up in stove
- when a map is opened it's also test saved in memory, and you'll get a warning if the result isn't identical to the file. keep a backup of maps that warn about this
- static meshes use their most detailed lod that has any data, and nanite data isn't read
- each material shows one colour texture without lighting from the map, glass is a flat colour and materials whose textures can't be found show a grid
- textures are shown at 1024 pixels at most, and virtual textures use the first layer only
- an edited map packed back with retoc has loaded in the last caretaker once, so keep backups
- invisible meshes like collision volumes show up as they do in the unreal editor

# updates and settings

builds from the [releases page](https://github.com/Marduc666/stove/releases) check for newer releases when they start and offer them in a window. `update` downloads the new build and puts it in place of the running one, and `restart now` starts it. test builds are offered newer test builds and stable builds only stable ones. this can be turned off with `options` → `check for updates`, and builds you make yourself don't check.

settings are kept in `settings.ron` in stove's config folder (`%APPDATA%\stove` on windows), so they carry over to new builds.

# building

```
cargo build -r
```

this needs [rust](https://rustup.rs) and on windows the msvc build tools. the result is the single file `target/release/stove.exe`.

on windows the build fails with `LNK1104` if the path to the target folder is too long, point `CARGO_TARGET_DIR` somewhere shorter if that happens.

oodle compressed paks are read with the `oo2core_*.dll` found in the game's folder, so no oodle sdk is needed.

# credits

- [atenfyr](https://github.com/atenfyr) for creating the extensive [UAssetAPI](https://github.com/atenfyr/UAssetAPI) which made this project possible ❤️
- [localcc](https://github.com/localcc) for rewriting it as [unreal_asset](https://github.com/AstroTechies/unrealmodding/tree/main/unreal_asset), allowing me to program this in [rust <img src="https://raw.githubusercontent.com/Tarikul-Islam-Anik/Animated-Fluent-Emojis/master/Emojis/Food/Crab.png" width="20" />](https://www.rust-lang.org/)
- [LongerWarrior](https://github.com/LongerWarrior) for pointing out everything I was missing in the actor duplication code
- [ASEAN Motor Club](https://github.com/ASEAN-Motor-Club/unrealmodding) for the usmap and unversioned property fixes this fork's [unreal_asset](https://github.com/Marduc666/unrealmodding) builds on
- [ihsoft](https://github.com/ihsoft) for working out the ue 5.8 cooked layouts in their [UAssetAPI](https://github.com/ihsoft/UAssetAPI) and [retoc](https://github.com/ihsoft/retoc) forks
- the [CUE4Parse](https://github.com/FabianFG/CUE4Parse) contributors for documenting what changed between engine versions
