//! Editable inputs live inside the bundle; baked v1 textures remain readable
//! by existing game and TrenchBroom loaders.
use crate::{BmatAlphaMode, BmatManifest, converter::encode_ktx2, read_tar_entries};
use bevy::{
    asset::RenderAssetUsages,
    image::{CompressedImageFormats, ImageSampler, ImageType},
    prelude::*,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, io::Write, path::Path};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ScalarInput {
    None,
    Constant(f32),
    Texture { source: String, channel: usize },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    #[serde(default)]
    pub alpha_default: bool,
    #[serde(default)]
    pub pbr: crate::BmatPbr,
    #[serde(default)]
    pub albedo_none: bool,
    #[serde(default)]
    pub emissive_none: bool,
    #[serde(default)]
    pub alpha_none: bool,
    pub version: u32,
    pub alpha: BmatAlphaMode,
    pub albedo: Option<String>,
    pub color: [f32; 4],
    pub normal: Option<String>,
    pub metallic: ScalarInput,
    pub roughness: ScalarInput,
    pub occlusion: ScalarInput,
    pub emissive: Option<String>,
    pub emission_color: [f32; 3],
    pub data: Option<String>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            alpha_default: true,
            pbr: crate::BmatPbr::default(),
            albedo_none: true,
            emissive_none: true,
            alpha_none: false,
            version: 1,
            alpha: BmatAlphaMode::Opaque,
            albedo: None,
            color: [1.; 4],
            normal: None,
            metallic: ScalarInput::None,
            roughness: ScalarInput::None,
            occlusion: ScalarInput::None,
            emissive: None,
            emission_color: [0.; 3],
            data: None,
        }
    }
}
#[derive(Clone, Default, PartialEq)]
pub struct Document {
    pub settings: Settings,
    pub entries: BTreeMap<String, Vec<u8>>,
}
pub struct Pixels {
    pub format: String,
    pub width: usize,
    pub height: usize,
    pub channels: u8,
    pub rgba: Vec<[u8; 4]>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ChannelSource {
    R,
    G,
    B,
    A,
    Luma,
    Zero,
    One,
}
impl ChannelSource {
    pub fn sample(self, p: [u8; 4]) -> u8 {
        match self {
            Self::R => p[0],
            Self::G => p[1],
            Self::B => p[2],
            Self::A => p[3],
            Self::Luma => {
                (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32).round() as u8
            }
            Self::Zero => 0,
            Self::One => 255,
        }
    }
}

pub fn mapped_ktx2(
    pixels: &Pixels,
    mapping: &[ChannelSource],
    srgb: bool,
) -> Result<Vec<u8>, String> {
    if !(1..=4).contains(&mapping.len()) {
        return Err("Choose 1–4 channels".into());
    }
    if srgb && mapping.len() < 3 {
        return Err("R and RG maps must be linear".into());
    }
    let bytes: Vec<u8> = pixels
        .rgba
        .iter()
        .flat_map(|p| mapping.iter().map(|s| s.sample(*p)))
        .collect();
    encode_ktx2(pixels.width, pixels.height, &bytes, mapping.len(), srgb).map_err(|e| e.to_string())
}

pub fn decode(name: &str, bytes: &[u8]) -> Result<Pixels, String> {
    let ext = Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let mut image = Image::from_buffer(
        bytes,
        ImageType::Extension(ext),
        CompressedImageFormats::NONE,
        false,
        ImageSampler::default(),
        RenderAssetUsages::MAIN_WORLD,
    )
    .map_err(|e| e.to_string())?;
    if image.texture_descriptor.dimension == bevy::render::render_resource::TextureDimension::D1 {
        image.texture_descriptor.dimension = bevy::render::render_resource::TextureDimension::D2;
    }
    if image.texture_descriptor.dimension != bevy::render::render_resource::TextureDimension::D2
        || image.texture_descriptor.size.depth_or_array_layers != 1
    {
        return Err("Material inputs must be single 2D textures".into());
    }
    let width = image.width() as usize;
    let height = image.height() as usize;
    if width * height > 16_777_216 {
        return Err("Texture exceeds the editor's 4096² pixel budget".into());
    }
    let mut rgba = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let c = image
                .get_color_at(x as u32, y as u32)
                .map_err(|e| e.to_string())?
                .to_linear();
            rgba.push(
                [c.red, c.green, c.blue, c.alpha].map(|f| (f.clamp(0., 1.) * 255.).round() as u8),
            );
        }
    }
    // GPU uploads can expand RGB to RGBA. Report the stored layout, not that expansion.
    let (channels, format) = if ext.eq_ignore_ascii_case("ktx2") {
        let reader = ktx2::Reader::new(bytes).map_err(|e| e.to_string())?;
        let fmt = reader.header().format;
        let channels = match fmt {
            Some(ktx2::Format::R8_UNORM) => 1,
            Some(ktx2::Format::R8G8_UNORM) => 2,
            Some(ktx2::Format::R8G8B8_UNORM | ktx2::Format::R8G8B8_SRGB) => 3,
            _ => image.texture_descriptor.format.components(),
        };
        (channels, format!("{fmt:?}"))
    } else {
        let decoded = ::image::load_from_memory(bytes).map_err(|e| e.to_string())?;
        (
            decoded.color().channel_count(),
            format!("PNG {:?}", decoded.color()),
        )
    };
    Ok(Pixels {
        format,
        width,
        height,
        channels,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mapped_import_preserves_stored_layout_and_channel_values() {
        let bytes = encode_ktx2(2, 1, &[10, 20, 30, 40, 100, 150, 200, 250], 4, false).unwrap();
        let pixels = decode("source.ktx2", &bytes).unwrap();
        let mapping = [
            ChannelSource::G,
            ChannelSource::A,
            ChannelSource::Zero,
            ChannelSource::One,
        ];
        for channels in 1..=4 {
            let mut doc = Document::default();
            let key = doc
                .import_mapped("packed_mask", &pixels, &mapping[..channels], false)
                .unwrap();
            let encoded = &doc.entries[&key];
            let reader = ktx2::Reader::new(encoded.as_slice()).unwrap();
            let expected: Vec<u8> = [[20, 40, 0, 255], [150, 250, 0, 255]]
                .iter()
                .flat_map(|p| p[..channels].iter().copied())
                .collect();
            assert_eq!(reader.levels().next().unwrap().data, expected);
            let decoded = doc.pixels(&key).unwrap();
            assert_eq!(decoded.channels as usize, channels);
            assert_eq!(decoded.rgba[0][0], 20);
            if channels > 1 {
                assert_eq!(decoded.rgba[0][1], 40);
            }
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("packed.bmat");
            doc.save(&file).unwrap();
            assert_eq!(Document::open(&file).unwrap().entries[&key], *encoded);
        }
        assert!(mapped_ktx2(&pixels, &mapping[..2], true).is_err());
        assert!(mapped_ktx2(&pixels, &[], false).is_err());
    }
    #[test]
    fn new_material_defaults_to_explicit_opaque() {
        let doc = Document::default();
        assert!(!doc.settings.alpha_none);
        assert!(doc.settings.alpha_default);
        let baked = doc.bake().unwrap();
        let manifest: BmatManifest = ron::de::from_bytes(&baked["manifest.ron"]).unwrap();
        assert_eq!(manifest.alpha_mode, BmatAlphaMode::Opaque);
    }
    #[test]
    fn extended_pbr_roundtrips_and_clears_generated_textures() {
        let mut doc = Document::default();
        let original = encode_ktx2(1, 1, &[32, 64, 128, 255], 4, false).unwrap();
        doc.entries
            .insert("sources/extended.ktx2".into(), original.clone());
        doc.settings.pbr.clearcoat = Some(0.7);
        doc.settings.pbr.ior = Some(1.333);
        doc.settings.pbr.specular_transmission = Some(0.9);
        doc.settings.pbr.anisotropy_rotation = Some(-0.6);
        for (_, texture) in doc.settings.pbr.textures_mut() {
            *texture = Some("sources/extended.ktx2".into());
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("advanced.bmat");
        doc.save(&path).unwrap();
        let mut reopened = Document::open(&path).unwrap();
        assert_eq!(doc.settings, reopened.settings);
        let mut manifest: BmatManifest =
            ron::de::from_bytes(&reopened.entries["manifest.ron"]).unwrap();
        assert_eq!(manifest.pbr.ior, Some(1.333));
        for (_, texture) in manifest.pbr.textures_mut() {
            assert_eq!(reopened.entries[texture.as_ref().unwrap()], original);
        }
        reopened.settings.pbr = crate::BmatPbr::default();
        reopened.save(&path).unwrap();
        let cleared = Document::open(&path).unwrap();
        assert!(!cleared.entries.keys().any(|k| k.starts_with("pbr-")));
        assert_eq!(cleared.entries["sources/extended.ktx2"], original);
        let m: BmatManifest = ron::de::from_bytes(&cleared.entries["manifest.ron"]).unwrap();
        assert_eq!(m.pbr, crate::BmatPbr::default());
    }
    #[test]
    fn none_omits_outputs_and_uses_defaults() {
        let mut doc = Document::default();
        doc.settings.alpha_none = true;
        doc.settings.alpha_default = false;
        doc.entries.insert("orm.ktx2".into(), vec![1]);
        doc.entries.insert("albedo.ktx2".into(), vec![1]);
        let baked = doc.bake().unwrap();
        assert!(!baked.keys().any(|k| k.ends_with(".ktx2")));
        let m: BmatManifest = ron::de::from_bytes(&baked["manifest.ron"]).unwrap();
        assert!(m.base_color_texture.is_none());
        assert!(m.metallic_roughness_texture.is_none());
        assert!(m.occlusion_texture.is_none());
        assert!(m.emissive_texture.is_none());
        assert_eq!(m.alpha_mode, BmatAlphaMode::default());
        doc.settings.roughness = ScalarInput::Constant(0.5);
        let baked = doc.bake().unwrap();
        assert_eq!(
            decode("orm.ktx2", &baked["orm.ktx2"]).unwrap().rgba,
            vec![[255, 128, 0, 255]]
        );
        let m: BmatManifest = ron::de::from_bytes(&baked["manifest.ron"]).unwrap();
        assert!(m.metallic_roughness_texture.is_some());
        assert!(m.occlusion_texture.is_none());
    }
    #[test]
    fn constants_and_mask_channels_survive_save_and_reopen() {
        let mut doc = Document::default();
        doc.settings.albedo_none = false;
        doc.entries.insert(
            "sources/mask.ktx2".into(),
            encode_ktx2(2, 1, &[10, 20, 30, 255, 100, 150, 200, 255], 4, false).unwrap(),
        );
        doc.settings.metallic = ScalarInput::Constant(0.25);
        doc.settings.roughness = ScalarInput::Texture {
            source: "sources/mask.ktx2".into(),
            channel: 1,
        };
        doc.settings.occlusion = ScalarInput::Texture {
            source: "sources/mask.ktx2".into(),
            channel: 0,
        };
        doc.entries.insert("custom.bin".into(), vec![42]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("material.bmat");
        doc.save(&path).unwrap();
        let reopened = Document::open(&path).unwrap();
        assert_eq!(reopened.settings, doc.settings);
        assert_eq!(reopened.entries["custom.bin"], vec![42]);
        let baked = reopened.bake().unwrap();
        let orm = decode("orm.ktx2", &baked["orm.ktx2"]).unwrap();
        assert_eq!(orm.rgba, vec![[10, 20, 64, 255], [100, 150, 64, 255]]);
        let material: BmatManifest = ron::de::from_bytes(&baked["manifest.ron"]).unwrap();
        assert_eq!(material.version, 1);
        crate::image_from_ktx2(
            &baked["albedo.ktx2"],
            true,
            bevy::image::ImageAddressMode::Repeat,
        )
        .unwrap();
    }
    #[test]
    fn invalid_inputs_do_not_replace_existing_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("material.bmat");
        let mut doc = Document::default();
        doc.save(&path).unwrap();
        let original = fs::read(&path).unwrap();
        doc.settings.metallic = ScalarInput::Constant(f32::NAN);
        assert!(doc.save(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
    }
    #[test]
    fn legacy_bundle_migrates_without_overwriting_its_sources() {
        let mut doc = Document::default();
        doc.settings.albedo_none = false;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.bmat");
        let mut baked = doc.bake().unwrap();
        baked.remove("editor.ron");
        let mut archive = tar::Builder::new(Vec::new());
        for (name, data) in baked {
            let mut h = tar::Header::new_ustar();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            archive.append_data(&mut h, name, data.as_slice()).unwrap();
        }
        let mut migrated = Document::from_bytes(&archive.into_inner().unwrap()).unwrap();
        let old = migrated.settings.albedo.clone().unwrap();
        let bytes = migrated.entries[&old].clone();
        migrated.settings.albedo = None;
        migrated.settings.color = [0.2, 0.4, 0.6, 1.];
        migrated.save(&path).unwrap();
        let reopened = Document::open(&path).unwrap();
        assert_eq!(reopened.entries[&old], bytes);
        assert_eq!(reopened.settings.color, [0.2, 0.4, 0.6, 1.]);
    }
}
impl Document {
    pub fn open(path: &Path) -> Result<Self, String> {
        Self::from_bytes(&fs::read(path).map_err(|e| e.to_string())?)
    }
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let mut entries = read_tar_entries(bytes)?;
        let manifest: BmatManifest =
            ron::de::from_bytes(entries.get("manifest.ron").ok_or("Missing manifest.ron")?)
                .map_err(|e| e.to_string())?;
        if manifest.version != 1 {
            return Err(format!("Unsupported BMAT version {}", manifest.version));
        }
        if let Some(meta) = entries.get("editor.ron") {
            let settings: Settings =
                ron::de::from_bytes(meta).map_err(|e| format!("Invalid editor metadata: {e}"))?;
            if settings.version != 1 {
                return Err("Unsupported editor metadata version".into());
            }
            let doc = Self { entries, settings };
            doc.bake()?;
            return Ok(doc);
        }
        // Copy original texture sources so rebaking never overwrites an input.
        let mut source = |name: Option<String>| -> Result<Option<String>, String> {
            name.map(|name| {
                let data = entries
                    .get(&name)
                    .ok_or_else(|| format!("Missing {name}"))?
                    .clone();
                let mut n = 0;
                let ext = Path::new(&name)
                    .extension()
                    .and_then(|v| v.to_str())
                    .unwrap_or("ktx2");
                loop {
                    let key = format!("sources/original-{n}.{ext}");
                    n += 1;
                    if !entries.contains_key(&key) {
                        entries.insert(key.clone(), data);
                        break Ok(key);
                    }
                }
            })
            .transpose()
        };
        let albedo = source(manifest.base_color_texture)?;
        let normal = source(manifest.normal_map_texture)?;
        let orm = source(manifest.metallic_roughness_texture)?;
        let ao = source(manifest.occlusion_texture)?;
        let emissive = source(manifest.emissive_texture)?;
        let data = source(manifest.data_texture)?;
        let mut pbr = manifest.pbr;
        for (_, texture) in pbr.textures_mut() {
            *texture = source(texture.take())?;
        }
        let settings = Settings {
            alpha_default: false,
            pbr,
            albedo_none: albedo.is_none(),
            emissive_none: emissive.is_none(),
            alpha_none: false,
            alpha: manifest.alpha_mode,
            albedo,
            normal,
            emissive,
            data,
            metallic: orm
                .clone()
                .map(|source| ScalarInput::Texture { source, channel: 2 })
                .unwrap_or(ScalarInput::None),
            roughness: orm
                .map(|source| ScalarInput::Texture { source, channel: 1 })
                .unwrap_or(ScalarInput::None),
            occlusion: ao
                .map(|source| ScalarInput::Texture { source, channel: 0 })
                .unwrap_or(ScalarInput::None),
            ..Settings::default()
        };
        let doc = Self { settings, entries };
        doc.bake()?;
        Ok(doc)
    }
    pub fn import(&mut self, path: &Path) -> Result<String, String> {
        let bytes = fs::read(path).map_err(|e| e.to_string())?;
        let ext = path
            .extension()
            .and_then(|x| x.to_str())
            .ok_or("Missing image extension")?
            .to_ascii_lowercase();
        decode(&format!("image.{ext}"), &bytes)?;
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("image")
            .chars()
            .take(40)
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>();
        for n in 0.. {
            let name = format!("sources/{stem}-{n}.{ext}");
            if !self.entries.contains_key(&name) {
                self.entries.insert(name.clone(), bytes);
                return Ok(name);
            }
        }
        unreachable!()
    }
    pub fn import_mapped(
        &mut self,
        name: &str,
        pixels: &Pixels,
        mapping: &[ChannelSource],
        srgb: bool,
    ) -> Result<String, String> {
        let bytes = mapped_ktx2(pixels, mapping, srgb)?;
        let stem: String = name
            .chars()
            .take(60)
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        if stem.is_empty() {
            return Err("Texture name cannot be empty".into());
        }
        for n in 0.. {
            let key = format!("sources/{stem}-{n}.ktx2");
            if !self.entries.contains_key(&key) {
                self.entries.insert(key.clone(), bytes);
                return Ok(key);
            }
        }
        unreachable!()
    }
    pub fn pixels(&self, name: &str) -> Result<Pixels, String> {
        decode(
            name,
            self.entries
                .get(name)
                .ok_or_else(|| format!("Missing texture {name}"))?,
        )
    }
    fn texture(&self, source: &str, srgb: bool) -> Result<Vec<u8>, String> {
        if source.ends_with(".ktx2") {
            return self
                .entries
                .get(source)
                .cloned()
                .ok_or("Missing KTX2 source".into());
        }
        let p = self.pixels(source)?;
        encode_ktx2(
            p.width,
            p.height,
            &p.rgba.into_iter().flatten().collect::<Vec<_>>(),
            4,
            srgb,
        )
        .map_err(|e| e.to_string())
    }
    /// Compile independent constants/maps to the existing v1 material contract.
    pub fn bake(&self) -> Result<BTreeMap<String, Vec<u8>>, String> {
        let s = &self.settings;
        s.pbr.validate()?;
        let mut out = self.entries.clone();
        // Remove previously generated outputs when an input is cleared.
        // Embedded sources are kept for editing, but are not runtime bindings.
        for key in ["albedo", "emissive", "orm", "normal", "data"] {
            out.remove(&format!("{key}.ktx2"));
        }
        let constant = |values: &[f32], srgb| -> Result<Vec<u8>, String> {
            if values
                .iter()
                .any(|v| !v.is_finite() || !(0. ..=1.).contains(v))
            {
                return Err("Color constants must be finite and between 0 and 1".into());
            }
            encode_ktx2(
                1,
                1,
                &values
                    .iter()
                    .map(|v| (v * 255.).round() as u8)
                    .collect::<Vec<_>>(),
                values.len(),
                srgb,
            )
            .map_err(|e| e.to_string())
        };
        if !s.albedo_none {
            out.insert(
                "albedo.ktx2".into(),
                if let Some(src) = &s.albedo {
                    self.texture(src, true)?
                } else {
                    constant(&s.color, true)?
                },
            );
        }
        if !s.emissive_none {
            out.insert(
                "emissive.ktx2".into(),
                if let Some(src) = &s.emissive {
                    self.texture(src, true)?
                } else {
                    constant(&s.emission_color, true)?
                },
            );
        }
        let mut maps = Vec::new();
        for input in [&s.occlusion, &s.roughness, &s.metallic] {
            maps.push(match input {
                ScalarInput::None => None,
                ScalarInput::Constant(v) => {
                    if !v.is_finite() || !(0. ..=1.).contains(v) {
                        return Err("Scalar constants must be finite and between 0 and 1".into());
                    }
                    None
                }
                ScalarInput::Texture { source, channel } => {
                    if *channel > 3 {
                        return Err("Invalid texture channel".into());
                    }
                    Some(self.pixels(source)?)
                }
            });
        }
        let w = maps.iter().flatten().map(|p| p.width).max().unwrap_or(1);
        let h = maps.iter().flatten().map(|p| p.height).max().unwrap_or(1);
        if w * h > 16_777_216 {
            return Err("Combined ORM texture exceeds pixel budget".into());
        }
        let inputs = [&s.occlusion, &s.roughness, &s.metallic];
        let mut orm = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                for (i, input) in inputs.iter().enumerate() {
                    orm.push(match input {
                        ScalarInput::None => {
                            if i == 2 {
                                0
                            } else {
                                255
                            }
                        }
                        ScalarInput::Constant(v) => (v * 255.).round() as u8,
                        ScalarInput::Texture { channel, .. } => {
                            let p = maps[i].as_ref().unwrap();
                            p.rgba[(y * p.height / h) * p.width + x * p.width / w][*channel]
                        }
                    });
                }
                orm.push(255);
            }
        }
        let mr = s.metallic != ScalarInput::None || s.roughness != ScalarInput::None;
        let ao = s.occlusion != ScalarInput::None;
        if mr || ao {
            out.insert(
                "orm.ktx2".into(),
                encode_ktx2(w, h, &orm, 4, false).map_err(|e| e.to_string())?,
            );
        }
        for (key, src) in [("normal", &s.normal), ("data", &s.data)] {
            if let Some(src) = src {
                out.insert(format!("{key}.ktx2"), self.texture(src, false)?);
            }
        }
        let opt = |key: &str, present: bool| {
            if present {
                format!("Some(\"{key}.ktx2\")")
            } else {
                "None".into()
            }
        };
        let mut pbr = s.pbr.clone();
        for (name, texture) in pbr.textures_mut() {
            let key = format!("pbr-{name}.ktx2");
            out.remove(&key);
            if let Some(src) = texture {
                out.insert(key.clone(), self.texture(src, false)?);
                *src = key;
            }
        }
        let mut alpha = if s.alpha_default {
            ",alpha_mode:Opaque".into()
        } else if s.alpha_none {
            String::new()
        } else {
            format!(",alpha_mode:{:?}", s.alpha)
        };
        if pbr != crate::BmatPbr::default() {
            alpha.push_str(&format!(
                ",pbr:{}",
                ron::ser::to_string(&pbr).map_err(|e| e.to_string())?
            ));
        }
        out.insert("manifest.ron".into(),format!("(version:1,base_color_texture:{},normal_map_texture:{},metallic_roughness_texture:{},occlusion_texture:{},emissive_texture:{},data_texture:{}{alpha})",opt("albedo",!s.albedo_none),opt("normal",s.normal.is_some()),opt("orm",mr),opt("orm",ao),opt("emissive",!s.emissive_none),opt("data",s.data.is_some())).into_bytes());
        out.insert(
            "editor.ron".into(),
            ron::ser::to_string_pretty(s, ron::ser::PrettyConfig::default())
                .map_err(|e| e.to_string())?
                .into_bytes(),
        );
        Ok(out)
    }
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let entries = self.bake()?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        let mut archive = tar::Builder::new(&mut temporary);
        for (name, data) in entries {
            let mut header = tar::Header::new_ustar();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            archive
                .append_data(&mut header, name, data.as_slice())
                .map_err(|e| e.to_string())?;
        }
        archive.finish().map_err(|e| e.to_string())?;
        drop(archive);
        temporary.flush().map_err(|e| e.to_string())?;
        temporary.as_file().sync_all().map_err(|e| e.to_string())?;
        temporary.persist(path).map_err(|e| e.to_string())?;
        Ok(())
    }
}
