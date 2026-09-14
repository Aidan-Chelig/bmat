//! Reversible authoring operations and portable external-texture workspaces.
use crate::editor::{ChannelSource, Document, ScalarInput, decode, mapped_ktx2};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::{Cursor, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Default)]
pub struct History {
    undo: Vec<Document>,
    redo: Vec<Document>,
}
impl History {
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn record(&mut self, previous: Document) {
        self.redo.clear();
        self.undo.push(previous);
        // Keep at least one undo for large documents, but bound retained history.
        while self.undo.len() > 1
            && (self.undo.len() > 32
                || self
                    .undo
                    .iter()
                    .map(|d| d.entries.values().map(Vec::len).sum::<usize>())
                    .sum::<usize>()
                    > 128 * 1024 * 1024)
        {
            self.undo.remove(0);
        }
    }
    pub fn undo(&mut self, current: &mut Document) -> bool {
        if let Some(old) = self.undo.pop() {
            self.redo.push(std::mem::replace(current, old));
            true
        } else {
            false
        }
    }
    pub fn redo(&mut self, current: &mut Document) -> bool {
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(current, next));
            true
        } else {
            false
        }
    }
}

impl Document {
    fn remap_reference(&mut self, old: &str, new: Option<&str>) {
        let s = &mut self.settings;
        let replace = |slot: &mut Option<String>| {
            if slot.as_deref() == Some(old) {
                *slot = new.map(str::to_owned);
                true
            } else {
                false
            }
        };
        if replace(&mut s.albedo) && new.is_none() {
            s.albedo_none = true;
        }
        if replace(&mut s.emissive) && new.is_none() {
            s.emissive_none = true;
        }
        replace(&mut s.normal);
        replace(&mut s.data);
        for input in [&mut s.metallic, &mut s.roughness, &mut s.occlusion] {
            if let ScalarInput::Texture { source, .. } = input {
                if source == old {
                    if let Some(n) = new {
                        *source = n.into();
                    } else {
                        *input = ScalarInput::None;
                    }
                }
            }
        }
        for (_, slot) in s.pbr.textures_mut() {
            replace(slot);
        }
    }
    pub fn rename_texture(&mut self, old: &str, name: &str) -> Result<String, String> {
        if name.is_empty()
            || name.len() > 60
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err("Use 1–60 letters, digits, hyphens, or underscores for the name".into());
        }
        let ext = Path::new(old)
            .extension()
            .and_then(|e| e.to_str())
            .ok_or("Texture has no extension")?;
        let new = format!("sources/{name}.{ext}");
        if old == new {
            return Ok(new);
        }
        if self.entries.contains_key(&new) {
            return Err("A texture with that name already exists".into());
        }
        let bytes = self.entries.remove(old).ok_or("Texture not found")?;
        self.entries.insert(new.clone(), bytes);
        self.remap_reference(old, Some(&new));
        Ok(new)
    }
    pub fn delete_texture(&mut self, name: &str) -> Result<(), String> {
        self.entries.remove(name).ok_or("Texture not found")?;
        self.remap_reference(name, None);
        Ok(())
    }
    /// PNG has no RG layout: use RGB with B=0, without display checkerboarding.
    pub fn texture_png(&self, name: &str) -> Result<Vec<u8>, String> {
        let p = self.pixels(name)?;
        let (bytes, color): (Vec<u8>, image::ColorType) = match p.channels {
            1 => (p.rgba.iter().map(|p| p[0]).collect(), image::ColorType::L8),
            2 => (
                p.rgba.iter().flat_map(|p| [p[0], p[1], 0]).collect(),
                image::ColorType::Rgb8,
            ),
            3 => (
                p.rgba.iter().flat_map(|p| [p[0], p[1], p[2]]).collect(),
                image::ColorType::Rgb8,
            ),
            _ => (
                p.rgba.iter().flatten().copied().collect(),
                image::ColorType::Rgba8,
            ),
        };
        let mut out = Cursor::new(Vec::new());
        image::write_buffer_with_format(
            &mut out,
            &bytes,
            p.width as u32,
            p.height as u32,
            color,
            image::ImageFormat::Png,
        )
        .map_err(|e| e.to_string())?;
        Ok(out.into_inner())
    }
}

pub fn write_export(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    temp.write_all(bytes).map_err(|e| e.to_string())?;
    temp.as_file().sync_all().map_err(|e| e.to_string())?;
    temp.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct TextureFile {
    key: String,
    file: String,
    channels: usize,
    srgb: bool,
    original_png_hash: u64,
}
#[derive(Serialize, Deserialize)]
struct Workspace {
    version: u32,
    textures: Vec<TextureFile>,
}
const INDEX: &str = "textures.ron";
const BASE: &str = "material.bmat";
fn hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |h, b| {
        (h ^ *b as u64).wrapping_mul(0x100000001b3)
    })
}

/// Export into an empty directory, refusing to overwrite unrelated work.
pub fn export_folder(doc: &Document, dir: &Path) -> Result<(), String> {
    if dir.exists()
        && fs::read_dir(dir)
            .map_err(|e| e.to_string())?
            .next()
            .is_some()
    {
        return Err("Choose an empty export directory".into());
    }
    let mut files = Vec::new();
    let mut textures = Vec::new();
    for key in doc.entries.keys().filter(|k| k.starts_with("sources/")) {
        let p = doc.pixels(key)?;
        let file = format!(
            "{:03}-{}.png",
            files.len(),
            Path::new(key)
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
        );
        // Flat output names only; never trust archive paths as filesystem paths.
        if !safe_file(&file) {
            return Err("Unsafe texture filename".into());
        }
        let srgb = if key.ends_with(".ktx2") {
            let r = ktx2::Reader::new(doc.entries[key].as_slice()).map_err(|e| e.to_string())?;
            matches!(
                r.header().format,
                Some(ktx2::Format::R8G8B8_SRGB | ktx2::Format::R8G8B8A8_SRGB)
            )
        } else {
            false
        };
        let png = doc.texture_png(key)?;
        textures.push(TextureFile {
            key: key.clone(),
            file: file.clone(),
            channels: p.channels as usize,
            srgb,
            original_png_hash: hash(&png),
        });
        files.push((file, png));
    }
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    doc.save(&dir.join(BASE))?;
    for (name, bytes) in files {
        write_export(&dir.join(name), &bytes)?;
    }
    write_export(
        &dir.join(INDEX),
        ron::ser::to_string_pretty(
            &Workspace {
                version: 1,
                textures,
            },
            ron::ser::PrettyConfig::default(),
        )
        .map_err(|e| e.to_string())?
        .as_bytes(),
    )
}
fn safe_file(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\']) && name != "." && name != ".."
}
fn inside(dir: &Path, file: &str) -> Result<PathBuf, String> {
    if !safe_file(file) {
        return Err("Unsafe workspace path".into());
    }
    let root = fs::canonicalize(dir).map_err(|e| e.to_string())?;
    let path = fs::canonicalize(root.join(file)).map_err(|e| e.to_string())?;
    if !path.starts_with(root) {
        return Err("Workspace symlink escapes directory".into());
    }
    Ok(path)
}
fn workspace(dir: &Path) -> Result<Workspace, String> {
    let w: Workspace =
        ron::de::from_bytes(&fs::read(inside(dir, INDEX)?).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    if w.version != 1 {
        return Err("Unsupported texture workspace version".into());
    }
    let mut keys = std::collections::BTreeSet::new();
    for t in &w.textures {
        if !t.key.starts_with("sources/") || !keys.insert(&t.key) || !(1..=4).contains(&t.channels)
        {
            return Err("Invalid workspace texture mapping".into());
        }
    }
    Ok(w)
}
pub fn import_folder(dir: &Path) -> Result<Document, String> {
    let mut doc = Document::open(&inside(dir, BASE)?)?;
    reload_folder(&mut doc, dir)?;
    Ok(doc)
}
/// Validate the entire batch before applying anything. Preserve current material settings.
pub fn reload_folder(doc: &mut Document, dir: &Path) -> Result<bool, String> {
    let w = workspace(dir)?;
    let mut updates = BTreeMap::new();
    let original = Document::open(&inside(dir, BASE)?)?;
    for t in w.textures {
        if !doc.entries.contains_key(&t.key) {
            return Err("Texture names changed; export a fresh workspace before watching".into());
        }
        let bytes = fs::read(inside(dir, &t.file)?).map_err(|e| e.to_string())?;
        if hash(&bytes) == t.original_png_hash {
            updates.insert(
                t.key.clone(),
                original
                    .entries
                    .get(&t.key)
                    .ok_or("Workspace texture missing from baseline")?
                    .clone(),
            );
            continue;
        }
        let p = decode("texture.png", &bytes)?;
        let mapping = [
            ChannelSource::R,
            ChannelSource::G,
            ChannelSource::B,
            ChannelSource::A,
        ];
        let encoded = if t.key.ends_with(".png") {
            bytes
        } else {
            mapped_ktx2(&p, &mapping[..t.channels], t.srgb)?
        };
        updates.insert(t.key, encoded);
    }
    let changed = updates
        .iter()
        .any(|(key, value)| doc.entries.get(key) != Some(value));
    if changed {
        doc.entries.extend(updates);
    }
    Ok(changed)
}
pub type Stamp = Vec<(String, u64, Option<SystemTime>)>;
#[cfg(test)]
mod tests {
    use super::*;
    use crate::converter::encode_ktx2;
    fn doc(channels: usize) -> Document {
        let mut d = Document::default();
        d.entries.insert(
            "sources/test.ktx2".into(),
            encode_ktx2(1, 1, &[10, 20, 30, 40][..channels], channels, false).unwrap(),
        );
        d
    }
    #[test]
    fn normalized_png_exports_have_expected_channels() {
        for channels in 1..=4 {
            let png = doc(channels).texture_png("sources/test.ktx2").unwrap();
            let p = image::load_from_memory(&png).unwrap();
            assert_eq!(
                p.color().channel_count(),
                if channels == 2 { 3 } else { channels as u8 }
            );
            let rgba = p.to_rgba8();
            assert_eq!(
                rgba.as_raw(),
                &match channels {
                    1 => vec![10, 10, 10, 255],
                    2 => vec![10, 20, 0, 255],
                    3 => vec![10, 20, 30, 255],
                    _ => vec![10, 20, 30, 40],
                }
            );
        }
    }
    #[test]
    fn rename_and_delete_update_all_references_and_undo() {
        let mut d = doc(4);
        let old = "sources/test.ktx2";
        d.settings.albedo_none = false;
        d.settings.albedo = Some(old.into());
        d.settings.normal = Some(old.into());
        d.settings.emissive_none = false;
        d.settings.emissive = Some(old.into());
        d.settings.data = Some(old.into());
        for slot in [
            &mut d.settings.metallic,
            &mut d.settings.roughness,
            &mut d.settings.occlusion,
        ] {
            *slot = ScalarInput::Texture {
                source: old.into(),
                channel: 0,
            };
        }
        for (_, slot) in d.settings.pbr.textures_mut() {
            *slot = Some(old.into());
        }
        let initial = d.clone();
        let mut history = History::default();
        history.record(d.clone());
        let new = d.rename_texture(old, "renamed").unwrap();
        assert_eq!(d.settings.albedo.as_deref(), Some(new.as_str()));
        assert!(!ron::ser::to_string(&d.settings).unwrap().contains(old));
        assert!(history.undo(&mut d));
        assert!(d == initial);
        assert!(history.redo(&mut d));
        history.record(d.clone());
        d.delete_texture(&new).unwrap();
        assert!(d.settings.albedo_none && d.settings.emissive_none);
        assert!(d.settings.normal.is_none() && d.settings.data.is_none());
        assert_eq!(d.settings.metallic, ScalarInput::None);
        assert!(
            d.settings
                .pbr
                .textures_mut()
                .iter()
                .all(|(_, slot)| slot.is_none())
        );
        assert!(history.undo(&mut d));
        assert!(d.entries.contains_key(&new));
        assert!(d.rename_texture(&new, "../escape").is_err());
        history.record(d.clone());
        assert!(!history.can_redo());
    }
    #[test]
    fn workspace_roundtrip_preserves_channels_settings_and_original_bytes() {
        for channels in 1..=4 {
            let d = doc(channels);
            let dir = tempfile::tempdir().unwrap();
            export_folder(&d, dir.path()).unwrap();
            let imported = import_folder(dir.path()).unwrap();
            assert_eq!(imported.settings, d.settings);
            assert_eq!(
                imported.entries["sources/test.ktx2"],
                d.entries["sources/test.ktx2"]
            );
            assert!(export_folder(&d, dir.path()).is_err());
        }
    }
    #[test]
    fn reload_is_atomic_and_preserves_live_material_edits() {
        let mut d = doc(2);
        d.entries.insert(
            "sources/other.ktx2".into(),
            d.entries["sources/test.ktx2"].clone(),
        );
        let dir = tempfile::tempdir().unwrap();
        export_folder(&d, dir.path()).unwrap();
        let initial_stamp = folder_stamp(dir.path()).unwrap();
        let w = workspace(dir.path()).unwrap();
        let mut edited = doc(2);
        edited.entries.insert(
            "sources/test.ktx2".into(),
            encode_ktx2(2, 1, &[70, 80, 90, 100], 2, false).unwrap(),
        );
        fs::write(
            dir.path().join(&w.textures[0].file),
            edited.texture_png("sources/test.ktx2").unwrap(),
        )
        .unwrap();
        let second_path = dir.path().join(&w.textures[1].file);
        let second_png = fs::read(&second_path).unwrap();
        fs::write(&second_path, b"partially written image").unwrap();
        let before = d.clone();
        assert!(reload_folder(&mut d, dir.path()).is_err());
        assert!(d == before);
        fs::write(&second_path, second_png).unwrap();
        d.settings.pbr.ior = Some(1.7);
        assert!(reload_folder(&mut d, dir.path()).unwrap());
        assert_eq!(d.settings.pbr.ior, Some(1.7));
        assert_eq!(d.pixels(&w.textures[0].key).unwrap().channels, 2);
        assert_eq!(d.pixels(&w.textures[0].key).unwrap().rgba[0][..2], [70, 80]);
        assert!(initial_stamp != folder_stamp(dir.path()).unwrap());
        assert!(!reload_folder(&mut d, dir.path()).unwrap());
    }
    #[test]
    fn workspace_rejects_path_traversal() {
        let d = doc(1);
        let dir = tempfile::tempdir().unwrap();
        export_folder(&d, dir.path()).unwrap();
        let mut w = workspace(dir.path()).unwrap();
        w.textures[0].file = "../outside.png".into();
        fs::write(dir.path().join(INDEX), ron::ser::to_string(&w).unwrap()).unwrap();
        assert!(import_folder(dir.path()).is_err());
    }
}
pub fn folder_stamp(dir: &Path) -> Result<Stamp, String> {
    let w = workspace(dir)?;
    let mut result = Vec::new();
    for file in std::iter::once(INDEX.to_string()).chain(w.textures.into_iter().map(|t| t.file)) {
        let m = fs::metadata(inside(dir, &file)?).map_err(|e| e.to_string())?;
        result.push((file, m.len(), m.modified().ok()));
    }
    Ok(result)
}
