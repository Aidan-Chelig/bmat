# bmat

`bmat` provides a Bevy asset loader for BMAT material bundles and an
`ora_to_bmat` command for compiling layered OpenRaster (`.ora`) or OpenEXR
materials into those bundles.

Each BMAT file is a tar archive containing `manifest.ron` and KTX2 textures.
Recognized source layer names are `albedo`, `normal`, `roughness`, `metallic`,
`ao`, `emissive`, and `data`.

```sh
cargo run --bin ora_to_bmat -- material.ora output-directory
```

Add `--overwrite` (or `-f`) to replace an existing bundle. A sibling RON file
can select the alpha mode:

```ron
(alpha_mode: Blend)
```

In Bevy, install `BmatAssetPlugin`. The optional `install` helper also hooks
BMAT lookup into a `bevy_trenchbroom::TrenchBroomConfig`.
