use std::{
    collections::{BTreeMap, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::Arc,
};

use bevy::{
    asset::RenderAssetUsages,
    asset::{AssetLoader, AssetPath, LoadContext, io::Reader},
    image::{
        CompressedImageFormats, ImageAddressMode, ImageSampler, ImageSamplerDescriptor, ImageType,
    },
    prelude::*,
    render::render_resource::Face,
    tasks::BoxedFuture,
};
use bevy_trenchbroom::bevy_materialize::erased_material::ErasedMaterial;
use bevy_trenchbroom::bevy_materialize::prelude::GenericMaterial;
use serde::Deserialize;

#[cfg(feature = "converter")]
pub mod converter;

#[derive(Debug, Deserialize)]
pub struct BmatManifest {
    version: u32,
    base_color_texture: Option<String>,
    normal_map_texture: Option<String>,
    metallic_roughness_texture: Option<String>,
    occlusion_texture: Option<String>,
    emissive_texture: Option<String>,
    #[serde(default)]
    data_texture: Option<String>,
    #[serde(default)]
    alpha_mode: BmatAlphaMode,
}

/// Mirrors `ora_to_ktx2.rs`'s `TextureAlphaMode` — the two are independently
/// defined (this crate and that binary don't share a library) but must
/// agree on the RON variant names one writes and the other parses here.
/// `#[serde(default)]` on `BmatManifest::alpha_mode` means a `.bmat` built
/// before this field existed still loads fine, defaulting to `Mask` (the
/// previously-hardcoded behavior for every material).
#[derive(Debug, Default, Clone, Copy, Deserialize)]
pub enum BmatAlphaMode {
    Opaque,
    #[default]
    Mask,
    Blend,
}

impl BmatAlphaMode {
    fn into_alpha_mode(self) -> AlphaMode {
        match self {
            BmatAlphaMode::Opaque => AlphaMode::Opaque,
            BmatAlphaMode::Mask => AlphaMode::Mask(0.5),
            BmatAlphaMode::Blend => AlphaMode::Blend,
        }
    }
}

#[derive(Default, TypePath)]
struct BmatAssetLoader;

pub struct BmatAssetPlugin;

impl Plugin for BmatAssetPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset_loader::<BmatAssetLoader>();
    }
}

impl AssetLoader for BmatAssetLoader {
    type Asset = GenericMaterial;
    type Settings = ();
    type Error = std::io::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _: &Self::Settings,
        load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let revision = content_revision(&bytes);
        let entries = read_tar_entries(&bytes).map_err(std::io::Error::other)?;
        let manifest_bytes = entries
            .get("manifest.ron")
            .ok_or_else(|| std::io::Error::other("missing manifest.ron"))?;
        let manifest: BmatManifest = ron::de::from_bytes(manifest_bytes)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        let alpha_mode = manifest.alpha_mode.into_alpha_mode();
        let material = load_material_asset(load_context, &entries, &manifest, alpha_mode, revision)
            .map_err(std::io::Error::other)?;
        if let Some(path) = manifest.data_texture {
            let bytes = entries
                .get(&path)
                .ok_or_else(|| std::io::Error::other(format!("missing {path}")))?;
            let image = image_from_ktx2(bytes, false, ImageAddressMode::ClampToEdge)
                .map_err(std::io::Error::other)?;
            load_context.add_labeled_asset("data".to_owned(), image);
        }
        Ok(material)
    }

    fn extensions(&self) -> &[&str] {
        &["bmat"]
    }
}

pub fn install(config: &mut bevy_trenchbroom::prelude::TrenchBroomConfig) {
    *config = config.clone().load_loose_texture_fn(|fallback| {
        Arc::new(move |view| {
            let fallback = fallback.clone();
            Box::pin(async move {
                let source = view.load_context.path().source().clone_owned();

                // Quake sky brushes are geometry markers, not visible
                // surfaces. The actual background is supplied by the
                // worldspawn cubemap, so keep loose `sky*` textures
                // transparent instead of drawing the editor marker image.
                if view.name.starts_with("sky") {
                    let material = StandardMaterial {
                        base_color: Color::srgba(0.0, 0.0, 0.0, 0.0),
                        alpha_mode: AlphaMode::Blend,
                        unlit: true,
                        ..default()
                    };
                    let material_handle = Box::new(material)
                        .add_labeled_asset(view.load_context, format!("Material_{}", view.name));
                    let generic_material = GenericMaterial {
                        handle: material_handle.into(),
                        properties: default(),
                    };
                    return view.load_context.add_labeled_asset(
                        format!("GenericMaterial_{}", view.name),
                        generic_material,
                    );
                }

                let path = AssetPath::from_path_buf(
                    PathBuf::from("textures").join(format!("{}.bmat", view.name)),
                )
                .with_source(source);

                #[cfg(feature = "hot_reload")]
                let bmat_exists = source_bmat_exists(view.name);
                #[cfg(not(feature = "hot_reload"))]
                let bmat_exists = view
                    .load_context
                    .read_asset_bytes(path.clone())
                    .await
                    .is_ok();

                if !bmat_exists {
                    return fallback(view).await;
                }

                view.load_context.load(path)
            }) as BoxedFuture<'_, Handle<GenericMaterial>>
        })
    });
}

#[cfg(feature = "hot_reload")]
fn source_bmat_exists(name: &str) -> bool {
    let asset_root = std::env::var_os("BEVY_ASSET_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    asset_root
        .join("assets/textures")
        .join(format!("{name}.bmat"))
        .is_file()
}

fn load_material_asset(
    load_context: &mut LoadContext<'_>,
    entries: &BTreeMap<String, Vec<u8>>,
    manifest: &BmatManifest,
    alpha_mode: AlphaMode,
    revision: u64,
) -> Result<GenericMaterial, String> {
    if manifest.version != 1 {
        return Err(format!("unsupported manifest version {}", manifest.version));
    }

    let mut material = StandardMaterial {
        perceptual_roughness: 1.0,
        alpha_mode,
        // Map surfaces are single-sided by default. Per-brush two-sided
        // metadata can override this when it is available to the loader.
        cull_mode: Some(Face::Back),
        ..default()
    };

    if let Some(path) = &manifest.base_color_texture {
        material.base_color_texture = Some(add_image(load_context, entries, path, true, revision)?);
    }
    if let Some(path) = &manifest.normal_map_texture {
        material.normal_map_texture =
            Some(add_image(load_context, entries, path, false, revision)?);
    }
    if let Some(path) = &manifest.metallic_roughness_texture {
        let image = add_image(load_context, entries, path, false, revision)?;
        material.metallic_roughness_texture = Some(image.clone());
    }
    if let Some(path) = &manifest.occlusion_texture {
        material.occlusion_texture = Some(add_image(load_context, entries, path, false, revision)?);
    }
    if let Some(path) = &manifest.emissive_texture {
        // Bevy multiplies the emissive texture by this color. Its default is
        // black, which would suppress the texture entirely.
        material.emissive = LinearRgba::WHITE;
        material.emissive_texture = Some(add_image(load_context, entries, path, true, revision)?);
    }

    let material_handle =
        load_context.add_labeled_asset(format!("standard_material_{revision:016x}"), material);
    Ok(GenericMaterial::new(material_handle))
}

fn add_image(
    load_context: &mut LoadContext<'_>,
    entries: &BTreeMap<String, Vec<u8>>,
    path: &str,
    is_srgb: bool,
    revision: u64,
) -> Result<Handle<Image>, String> {
    let bytes = entries
        .get(path)
        .ok_or_else(|| format!("manifest references missing {path}"))?;
    let image = image_from_ktx2(bytes, is_srgb, ImageAddressMode::Repeat)
        .map_err(|error| format!("invalid KTX2 texture {path}: {error}"))?;
    Ok(load_context.add_labeled_asset(format!("image_{path}_{revision:016x}"), image))
}

fn content_revision(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

fn image_from_ktx2(
    bytes: &[u8],
    is_srgb: bool,
    address_mode: ImageAddressMode,
) -> Result<Image, String> {
    let mut sampler = ImageSamplerDescriptor::linear();
    sampler.set_address_mode(address_mode);
    Image::from_buffer(
        bytes,
        ImageType::Extension("ktx2"),
        CompressedImageFormats::NONE,
        is_srgb,
        ImageSampler::Descriptor(sampler),
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    )
    .map_err(|error| error.to_string())
}

/// Returns the dimensions of the primary texture in a BMAT bundle.
///
/// Map loaders can use this before the material asset and its dependent images
/// are available, ensuring authored texel-space UVs are normalized correctly.
pub fn texture_size(bytes: &[u8]) -> Result<UVec2, String> {
    let entries = read_tar_entries(bytes)?;
    let manifest_bytes = entries
        .get("manifest.ron")
        .ok_or_else(|| "missing manifest.ron".to_owned())?;
    let manifest: BmatManifest =
        ron::de::from_bytes(manifest_bytes).map_err(|error| error.to_string())?;
    if manifest.version != 1 {
        return Err(format!("unsupported manifest version {}", manifest.version));
    }
    let path = manifest
        .base_color_texture
        .as_ref()
        .or(manifest.data_texture.as_ref())
        .ok_or_else(|| "manifest has no texture suitable for sizing".to_owned())?;
    let ktx2 = entries
        .get(path)
        .ok_or_else(|| format!("manifest references missing {path}"))?;
    ktx2_size(ktx2).map_err(|error| format!("invalid KTX2 texture {path}: {error}"))
}

fn ktx2_size(bytes: &[u8]) -> Result<UVec2, String> {
    const IDENTIFIER: &[u8; 12] = b"\xABKTX 20\xBB\r\n\x1A\n";
    if bytes.len() < 28 || &bytes[..12] != IDENTIFIER {
        return Err("invalid identifier or truncated header".to_owned());
    }
    let width = u32::from_le_bytes(bytes[20..24].try_into().expect("four-byte slice"));
    let height = u32::from_le_bytes(bytes[24..28].try_into().expect("four-byte slice"));
    if width == 0 || height == 0 {
        return Err(format!("invalid dimensions {width}x{height}"));
    }
    Ok(UVec2::new(width, height))
}

fn read_tar_entries(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut entries = BTreeMap::new();
    let mut offset = 0;
    while offset + 512 <= bytes.len() {
        let header = &bytes[offset..offset + 512];
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        let name = tar_string(&header[..100]);
        let size = tar_octal(&header[124..136])?;
        let data_start = offset + 512;
        let data_end = data_start
            .checked_add(size)
            .ok_or_else(|| "BMAT entry size overflow".to_owned())?;
        if data_end > bytes.len() {
            return Err(format!("BMAT entry {name} extends past archive"));
        }
        entries.insert(name, bytes[data_start..data_end].to_vec());
        offset = data_start + size.div_ceil(512) * 512;
    }
    Ok(entries)
}

fn tar_string(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

fn tar_octal(bytes: &[u8]) -> Result<usize, String> {
    let value = tar_string(bytes);
    usize::from_str_radix(value.trim(), 8)
        .map_err(|error| format!("invalid BMAT tar size {value:?}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_bmat_tar_entries() {
        let mut archive = vec![0; 512];
        archive[..5].copy_from_slice(b"test\0");
        archive[124..136].copy_from_slice(b"00000000003\0");
        archive.extend_from_slice(b"abc");
        archive.resize(1024 + 3, 0);
        archive.extend_from_slice(&[0; 1024]);

        let entries = read_tar_entries(&archive).unwrap();
        assert_eq!(entries.get("test"), Some(&b"abc".to_vec()));
    }

    #[test]
    fn reads_primary_texture_size() {
        let manifest = b"(version:1,base_color_texture:Some(\"albedo.ktx2\"),normal_map_texture:None,metallic_roughness_texture:None,occlusion_texture:None,emissive_texture:None,data_texture:None,alpha_mode:Opaque)";
        let mut ktx2 = vec![0; 28];
        ktx2[..12].copy_from_slice(b"\xABKTX 20\xBB\r\n\x1A\n");
        ktx2[20..24].copy_from_slice(&512u32.to_le_bytes());
        ktx2[24..28].copy_from_slice(&256u32.to_le_bytes());
        let mut archive = Vec::new();
        for (name, data) in [
            ("manifest.ron", manifest.as_slice()),
            ("albedo.ktx2", &ktx2),
        ] {
            let mut header = [0; 512];
            header[..name.len()].copy_from_slice(name.as_bytes());
            let size = format!("{:011o}\0", data.len());
            header[124..136].copy_from_slice(size.as_bytes());
            archive.extend_from_slice(&header);
            archive.extend_from_slice(data);
            archive.resize(archive.len().div_ceil(512) * 512, 0);
        }
        archive.extend_from_slice(&[0; 1024]);
        assert_eq!(texture_size(&archive), Ok(UVec2::new(512, 256)));
    }
}
