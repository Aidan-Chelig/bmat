# edbmat

Run `cargo run --bin edbmat -- /path/to/material.bmat`. An existing file is
opened; a missing file starts a new material that will save at that path.
The `editor` feature is enabled by default. Runtime consumers can use
`default-features = false` to omit editor/converter dependencies.

The left inspector edits every input in the current BMAT material contract:

| Input | Sources |
| --- | --- |
| Base color | None, constant RGBA color or texture |
| Metallic, roughness, occlusion | None, constant from 0–1 or a texture's R/G/B/A channel |
| Emissive | None, constant RGB color or texture |
| Normal, data | Optional texture |
| Alpha mode | Default (Opaque), Opaque, Mask (cutoff 0.5), Blend |
| Clearcoat | Optional strength, roughness, strength/roughness/normal textures |
| Transmission | Optional diffuse/specular factors and textures, thickness and texture |
| Volume | Optional IOR, attenuation distance and linear RGB attenuation color |
| Anisotropy | Optional strength, rotation in radians, direction/strength texture |

The extended sections expose separate **None / Value** factors and optional
textures. Texture samples multiply their factors: a map alone does not enable
an effect whose default factor is zero. Clearcoat, diffuse/specular transmission,
thickness, and anisotropy strength default to 0; coat roughness defaults to 0.5,
IOR to 1.5, and attenuation distance to infinity. None preserves these defaults.
Advanced constants are stored as f32, without baking/quantizing them to textures.

Advanced maps use Bevy's linear channel layouts: clearcoat strength R,
coat roughness G, diffuse transmission A, specular transmission R, thickness G,
and anisotropy RG direction/B strength. Normal maps use tangent-space RGB.
Anisotropy requires mesh tangents; the preview cube generates them.
Refraction requires specular transmission, and thickness controls distortion.
The appearance also depends on the scene and camera's transmission settings.

These fields are stored in an optional `pbr` block in the runtime manifest.
Older bundles still load with unchanged defaults. Consumers must update to
this BMAT loader to render the new properties; older loaders/viewers may
ignore the block. This does not update the game's dependency or TrenchBroom.

New documents start with Opaque alpha mode and all other inputs unset. **None** omits the runtime binding
and removes its generated texture. Defaults are white base color, nonmetallic,
roughness 1, unoccluded, no emission, and no normal/data map. The Alpha section
has a single dropdown. Default (Opaque) writes Opaque explicitly so it also
works with older loaders. Existing files with omitted alpha retain their legacy Mask behavior.
Metallic and roughness share a packed texture: if either is set, the unset
channel contains its default. Embedded source images remain available for
editing even when they are not bound to a material input.

Use **File → Import texture** to read a PNG or uncompressed KTX2 image.
Choose an embedded name, R/RG/RGB/RGBA output, and the source of each output
channel (R/G/B/A, weighted RGB luma, zero, or one). The preview updates before
you confirm. Import creates an 8-bit-per-channel KTX2; R/RG are linear, while
RGB/RGBA can be tagged linear or sRGB. Tagging does not convert channel values.
Higher precision inputs are quantized to 8-bit; this is not a lossless import
for HDR textures. Embedded textures lists dimensions, stored layout/channel
count, and format, even when the GPU expands RGB to RGBA internally.

Select the named embedded texture in any material slot. For example, one
RG texture can hold coat strength in R and coat roughness in G and be selected
in both coat slots. Packed ORM textures can feed several
inputs using different channels. Inputs with differing dimensions are sampled
with nearest-neighbor filtering at the largest input width and height.
The editor currently limits decoded and packed images to 16,777,216 pixels.

The cube updates after edits. **View mask** displays a single channel in
grayscale; one-channel images use grayscale, two-channel images use red/green,
and alpha is composited over a checkerboard. Rotation can be paused in the
top bar. The inspector scrolls and resizes independently of the preview.

**File → New / Open / Save / Save As / Quit** manages documents; Ctrl+S saves.
New/Open/Save As/Import use native file pickers (the desktop's XDG portal on
Linux, native dialogs on Windows/macOS). Linux needs a working XDG desktop
portal with a file-chooser backend. Cancelling the picker leaves the document
unchanged. After selection, the editor confirms the operation. Save As confirms
overwrites, and Open/New/Quit prompt before discarding unsaved changes.
Save writes a temporary file beside the destination and atomically replaces it
only once the complete bundle has been written.

There are no external sidecars. A saved archive includes `editor.ron` with
editable settings, `sources/` with imported KTX2 images (or preserved legacy sources), and the standard
v1 `manifest.ron` and runtime KTX2 textures. Legacy BMATs are imported into this
document model automatically. Unrelated archive entries are preserved.
Constants retain their f32 values in editor metadata; runtime textures use
the existing 8-bit normalized BMAT encoding (including constants baked into
ORM). This is a material assembler, not a pixel-painting application.

The loader now uses a metallic multiplier of 1 when an ORM texture exists,
so its blue channel contributes metalness. Update runtime consumers to this
loader revision to see metallic maps correctly. No game dependency pin is
changed by editing this repository.
