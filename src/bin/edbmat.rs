use bevy::{
    camera::{CameraOutputMode, Viewport, visibility::RenderLayers},
    core_pipeline::Skybox,
    image::ImageAddressMode,
    prelude::*,
    render::render_resource::BlendState,
    window::WindowCloseRequested,
};
use bevy_egui::{
    EguiContexts, EguiGlobalSettings, EguiPlugin, EguiPrimaryContextPass, PrimaryEguiContext, egui,
};
use bmat::editor_workflow::{self as workflow, History, Stamp};
use bmat::{
    BmatAlphaMode,
    editor::{ChannelSource, Document, Pixels, ScalarInput, decode},
    image_from_ktx2,
};
use std::time::{Duration, Instant};
use std::{collections::BTreeMap, fs, path::PathBuf};

#[derive(Resource)]
struct Editor {
    project: bool,
    export_path: PathBuf,
    history: History,
    saved: Document,
    editing_gesture: bool,
    watch: Option<WatchFolder>,
    texture_action: Option<(String, String, bool)>,
    export_key: Option<String>,
    import_draft: Option<ImportDraft>,
    texture_info: BTreeMap<String, String>,
    path: PathBuf,
    doc: Document,
    dirty: bool,
    rebuild: bool,
    status: String,
    dialog: Option<Dialog>,
    picked_path: PathBuf,
    overwrite: bool,
    discard: bool,
    selected: Option<(String, Option<usize>)>,
    thumbnails: BTreeMap<String, egui::TextureHandle>,
    material: Handle<StandardMaterial>,
    images: Vec<Handle<Image>>,
    rotate: bool,
    tabs: Vec<MaterialTab>,
    active_tab: usize,
    explorer_root: PathBuf,
}
#[derive(Clone)]
struct MaterialTab {
    path: PathBuf,
    project: bool,
    export_path: PathBuf,
    doc: Document,
    saved: Document,
    dirty: bool,
}
struct WatchFolder {
    path: PathBuf,
    enabled: bool,
    last: Stamp,
    pending: Option<Stamp>,
    next: Instant,
    project: bool,
}
struct ImportDraft {
    pixels: Pixels,
    name: String,
    channels: usize,
    mapping: [ChannelSource; 4],
    srgb: bool,
    preview: Option<egui::TextureHandle>,
}
#[derive(Clone, Copy)]
enum Dialog {
    OpenProject,
    SaveProjectAs,
    ExportBmat,
    ExportFolder,
    ImportFolder,
    ExportTexture,
    New,
    Open,
    Import,
    Exit,
}
#[derive(Component)]
struct Cube;

#[derive(Resource)]
struct OrbitCamera {
    target: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
}
impl Default for OrbitCamera {
    fn default() -> Self {
        let pos = Vec3::new(3.5, 2.5, 5.);
        Self {
            target: Vec3::ZERO,
            yaw: pos.x.atan2(pos.z),
            pitch: (pos.y / pos.length()).asin(),
            distance: pos.length(),
        }
    }
}
impl OrbitCamera {
    fn rotation(&self) -> Quat {
        Quat::from_rotation_y(self.yaw) * Quat::from_rotation_x(-self.pitch)
    }
    fn transform(&self) -> Transform {
        Transform::from_translation(self.target + self.rotation() * Vec3::Z * self.distance)
            .looking_at(self.target, Vec3::Y)
    }
    fn orbit(&mut self, delta: egui::Vec2) {
        self.yaw = (self.yaw - delta.x * 0.007).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch + delta.y * 0.007).clamp(-1.5, 1.5);
    }
    fn zoom(&mut self, scroll: f32) {
        self.distance = (self.distance * (-scroll * 0.002).exp()).clamp(2.2, 50.);
    }
    fn pan(&mut self, delta: egui::Vec2, height: f32) {
        let scale = 2. * self.distance * (std::f32::consts::FRAC_PI_8).tan() / height.max(1.);
        self.target += self.rotation() * Vec3::new(-delta.x, delta.y, 0.) * scale;
    }
}

#[cfg(test)]
mod orbit_tests {
    use super::*;
    #[test]
    fn default_view_matches_initial_camera() {
        let view = OrbitCamera::default().transform();
        assert!(
            view.translation
                .abs_diff_eq(Vec3::new(3.5, 2.5, 5.), 0.00001)
        );
        assert!(
            view.forward()
                .as_vec3()
                .abs_diff_eq(-view.translation.normalize(), 0.00001)
        );
    }
    #[test]
    fn orbit_and_zoom_remain_bounded() {
        let mut orbit = OrbitCamera::default();
        orbit.orbit(egui::vec2(100000., 100000.));
        assert_eq!(orbit.pitch, 1.5);
        orbit.zoom(100000.);
        assert_eq!(orbit.distance, 2.2);
        orbit.zoom(-100000.);
        assert_eq!(orbit.distance, 50.);
        orbit.pan(egui::vec2(10., 20.), 600.);
        assert!(orbit.transform().translation.is_finite());
        assert_ne!(orbit.target, Vec3::ZERO);
    }
}

fn main() {
    let supplied_path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| "material.bmat".into());
    let project_folder = supplied_path.is_dir()
        && !supplied_path.join(workflow::PROJECT_MANIFEST).is_file();
    let path = if project_folder {
        let mut projects = fs::read_dir(&supplied_path)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir() && path.join(workflow::PROJECT_MANIFEST).is_file())
            .collect::<Vec<_>>();
        projects.sort();
        projects.into_iter().next().unwrap_or(supplied_path.clone())
    } else {
        supplied_path.clone()
    };
    let project_hint = path.extension().is_none();
    let (doc, project, export_path, status) = if path.is_dir() {
        match Document::open_project(&path) {
            Ok((doc, manifest)) => (
                doc,
                true,
                manifest.export_path,
                "Opened BMAT project".into(),
            ),
            Err(e) => {
                eprintln!("Cannot open project {}: {e}", path.display());
                std::process::exit(1);
            }
        }
    } else if path.exists() {
        match Document::open(&path) {
            Ok(doc) => (doc, false, PathBuf::new(), "Opened material".into()),
            Err(e) => {
                eprintln!("Cannot open {}: {e}", path.display());
                std::process::exit(1);
            }
        }
    } else if project_hint {
        (
            Document::default(),
            true,
            PathBuf::from("build/material.bmat"),
            "New BMAT project — Save to create the project".into(),
        )
    } else {
        (
            Document::default(),
            false,
            PathBuf::new(),
            "New material — Save to create the BMAT".into(),
        )
    };
    App::new()
        .add_plugins((
            DefaultPlugins.set(WindowPlugin {
                close_when_requested: false,
                primary_window: Some(Window {
                    title: "edbmat".into(),
                    ..default()
                }),
                ..default()
            }),
            EguiPlugin::default(),
        ))
        .insert_resource(Editor {
            project,
            export_path: export_path.clone(),
            history: History::default(),
            saved: doc.clone(),
            editing_gesture: false,
            watch: None,
            texture_action: None,
            export_key: None,
            import_draft: None,
            texture_info: BTreeMap::new(),
            path: path.clone(),
            doc: doc.clone(),
            dirty: false,
            rebuild: true,
            status,
            dialog: None,
            picked_path: PathBuf::new(),
            overwrite: false,
            discard: false,
            selected: None,
            thumbnails: BTreeMap::new(),
            material: default(),
            images: Vec::new(),
            rotate: true,
            tabs: vec![MaterialTab { path: path.clone(), project, export_path, doc: doc.clone(), saved: doc, dirty: false }],
            active_tab: 0,
            explorer_root: if project_folder {
                supplied_path
            } else {
                path.parent().unwrap_or(std::path::Path::new(".")).to_path_buf()
            },
        })
        .insert_resource(ClearColor(Color::srgb(0.075, 0.085, 0.105)))
        .insert_resource(GlobalAmbientLight {
            brightness: 350.,
            ..default()
        })
        .add_systems(Startup, setup)
        .init_resource::<OrbitCamera>()
        .add_systems(Update, (rebuild, rotate, close_request).chain())
        .add_systems(EguiPrimaryContextPass, ui)
        .run();
}
fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut editor: ResMut<Editor>,
    mut egui_settings: ResMut<EguiGlobalSettings>,
) {
    egui_settings.auto_create_primary_context = false;
    if editor.project && editor.watch.is_none() {
        let path = editor.path.clone();
        set_project_watch(&mut editor, path);
    }
    editor.material = materials.add(StandardMaterial::default());
    let skybox = images.add(
        image_from_ktx2(
            include_bytes!("../../assets/sky_skybox.ktx2"),
            false,
            ImageAddressMode::ClampToEdge,
        )
        .expect("Bundled skybox must be a valid KTX2 cubemap"),
    );
    let mut cube = Mesh::from(Cuboid::from_length(2.));
    cube.generate_tangents()
        .expect("Preview cube has valid normals and UVs");
    commands.spawn((
        Cube,
        Mesh3d(meshes.add(cube)),
        MeshMaterial3d(editor.material.clone()),
        Transform::default(),
    ));
    commands.spawn((
        Camera3d::default(),
        Skybox {
            image: Some(skybox),
            brightness: 1000.0,
            rotation: Quat::IDENTITY,
        },
        Transform::from_xyz(3.5, 2.5, 5.).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        PrimaryEguiContext,
        Camera2d,
        RenderLayers::none(),
        Camera {
            order: 1,
            clear_color: ClearColorConfig::Custom(Color::NONE),
            output_mode: CameraOutputMode::Write {
                blend_state: Some(BlendState::ALPHA_BLENDING),
                clear_color: ClearColorConfig::None,
            },
            ..default()
        },
    ));
    commands.spawn((
        PointLight {
            intensity: 250_000.,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(3., 4., 4.),
    ));
}
fn close_request(
    mut requests: MessageReader<WindowCloseRequested>,
    mut e: ResMut<Editor>,
    mut exit: MessageWriter<AppExit>,
) {
    if requests.read().next().is_some() {
        if e.dirty {
            dialog(&mut e, Dialog::Exit);
        } else {
            exit.write(AppExit::Success);
        }
    }
}
fn rotate(time: Res<Time>, editor: Res<Editor>, mut cubes: Query<&mut Transform, With<Cube>>) {
    if editor.rotate {
        for mut t in &mut cubes {
            t.rotate_y(time.delta_secs() * 0.45);
        }
    }
}
fn rebuild(
    mut editor: ResMut<Editor>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !editor.rebuild {
        return;
    }
    editor.rebuild = false;
    let result = (|| -> Result<(StandardMaterial, Vec<Handle<Image>>), String> {
        let baked = editor.doc.bake()?;
        let manifest: bmat::BmatManifest =
            ron::de::from_bytes(&baked["manifest.ron"]).map_err(|e| e.to_string())?;
        let mut handles = Vec::new();
        let mut get = |name: &str, srgb: bool| -> Result<Handle<Image>, String> {
            let image = image_from_ktx2(&baked[name], srgb, ImageAddressMode::Repeat)?;
            let h = images.add(image);
            handles.push(h.clone());
            Ok(h)
        };
        let mut material = StandardMaterial {
            base_color_texture: manifest
                .base_color_texture
                .as_deref()
                .map(|p| get(p, true))
                .transpose()?,
            metallic: if manifest.metallic_roughness_texture.is_some() {
                1.
            } else {
                0.
            },
            perceptual_roughness: 1.,
            metallic_roughness_texture: manifest
                .metallic_roughness_texture
                .as_deref()
                .map(|p| get(p, false))
                .transpose()?,
            occlusion_texture: manifest
                .occlusion_texture
                .as_deref()
                .map(|p| get(p, false))
                .transpose()?,
            normal_map_texture: if editor.doc.settings.normal.is_some() {
                Some(get("normal.ktx2", false)?)
            } else {
                None
            },
            emissive: if manifest.emissive_texture.is_some() {
                LinearRgba::WHITE
            } else {
                LinearRgba::BLACK
            },
            emissive_texture: manifest
                .emissive_texture
                .as_deref()
                .map(|p| get(p, true))
                .transpose()?,
            alpha_mode: match manifest.alpha_mode {
                BmatAlphaMode::Opaque => AlphaMode::Opaque,
                BmatAlphaMode::Mask => AlphaMode::Mask(0.5),
                BmatAlphaMode::Blend => AlphaMode::Blend,
            },
            ..default()
        };
        manifest.pbr.apply(&mut material, |path| get(path, false))?;
        Ok((material, handles))
    })();
    match result {
        Ok((material, handles)) => {
            if let Some(mut m) = materials.get_mut(&editor.material) {
                *m = material;
            }
            for h in editor.images.drain(..) {
                images.remove(h.id());
            }
            editor.images = handles;
        }
        Err(e) => editor.status = e,
    }
}
fn save(editor: &mut Editor, path: PathBuf) {
    let result = if editor.project {
        editor.doc.save_project(&editor.path, &editor.export_path)
    } else {
        editor.doc.save(&path)
    };
    match result {
        Ok(()) => {
            editor.saved = editor.doc.clone();
            editor.path = path;
            editor.dirty = false;
            editor.status = "Saved".into();
            editor.dialog = None;
        }
        Err(e) => editor.status = e,
    }
}
fn import_options(ui: &mut egui::Ui, d: &mut ImportDraft) {
    ui.label(format!(
        "Source: {} × {} · {} channels · {}",
        d.pixels.width, d.pixels.height, d.pixels.channels, d.pixels.format
    ));
    ui.label("Embedded texture name");
    ui.text_edit_singleline(&mut d.name);
    let before = (d.channels, d.mapping, d.srgb);
    egui::ComboBox::from_label("Output layout")
        .selected_text(["", "R", "RG", "RGB", "RGBA"][d.channels])
        .show_ui(ui, |ui| {
            for (n, label) in [(1, "R"), (2, "RG"), (3, "RGB"), (4, "RGBA")] {
                ui.selectable_value(&mut d.channels, n, label);
            }
        });
    if d.channels < 3 {
        d.srgb = false;
    }
    ui.add_enabled_ui(d.channels >= 3, |ui| {
        ui.checkbox(&mut d.srgb, "sRGB color texture (otherwise linear data)");
    });
    ui.small("8 bits per channel. Mapping copies channel values; sRGB sets interpretation, not a color conversion. Luma is a weighted RGB grayscale mix.");
    for i in 0..d.channels {
        egui::ComboBox::from_label(format!("Output {}", ["R", "G", "B", "A"][i]))
            .selected_text(format!("{:?}", d.mapping[i]))
            .show_ui(ui, |ui| {
                for source in [
                    ChannelSource::R,
                    ChannelSource::G,
                    ChannelSource::B,
                    ChannelSource::A,
                    ChannelSource::Luma,
                    ChannelSource::Zero,
                    ChannelSource::One,
                ] {
                    ui.selectable_value(&mut d.mapping[i], source, format!("{source:?}"));
                }
            });
    }
    if d.preview.is_none() || before != (d.channels, d.mapping, d.srgb) {
        let scale = (256. / d.pixels.width.max(d.pixels.height) as f32).min(1.);
        let w = (d.pixels.width as f32 * scale).round().max(1.) as usize;
        let h = (d.pixels.height as f32 * scale).round().max(1.) as usize;
        let mut colors = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let p = d.pixels.rgba
                    [(y * d.pixels.height / h) * d.pixels.width + x * d.pixels.width / w];
                let mut c = [0, 0, 0, 255];
                for i in 0..d.channels {
                    c[i] = d.mapping[i].sample(p);
                }
                if d.channels == 1 {
                    c[1] = c[0];
                    c[2] = c[0];
                }
                let a = c[3] as f32 / 255.;
                let checker = if (x / 12 + y / 12) % 2 == 0 {
                    90.
                } else {
                    150.
                };
                colors.push(egui::Color32::from_rgb(
                    (c[0] as f32 * a + checker * (1. - a)) as u8,
                    (c[1] as f32 * a + checker * (1. - a)) as u8,
                    (c[2] as f32 * a + checker * (1. - a)) as u8,
                ));
            }
        }
        d.preview = Some(ui.ctx().load_texture(
            "import-preview",
            egui::ColorImage::new([w, h], colors),
            egui::TextureOptions::NEAREST,
        ));
    }
    if let Some(preview) = &d.preview {
        ui.image((preview.id(), preview.size_vec2()));
    }
}
fn dialog(editor: &mut Editor, kind: Dialog) {
    if matches!(kind, Dialog::ExportTexture) {
        editor.export_key = editor.selected.as_ref().map(|(key, _)| key.clone());
    }
    if matches!(
        kind,
        Dialog::OpenProject | Dialog::SaveProjectAs | Dialog::ExportFolder | Dialog::ImportFolder
    ) {
        if let Some(path) = rfd::FileDialog::new().pick_folder() {
            editor.picked_path = path;
            editor.dialog = Some(kind);
            editor.discard = false;
            editor.overwrite = false;
        }
        return;
    }
    if !matches!(kind, Dialog::Exit) {
        let mut picker = rfd::FileDialog::new();
        if let Some(parent) = editor.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            picker = picker.set_directory(parent);
        }
        picker = if matches!(kind, Dialog::ExportTexture | Dialog::ExportBmat) {
            picker.add_filter("PNG texture", &["png"])
        } else if matches!(kind, Dialog::Import) {
            picker.add_filter("Texture", &["png", "ktx2"])
        } else {
            picker.add_filter("BMAT material", &["bmat"])
        };
        let chosen = match kind {
            Dialog::ExportTexture => picker.set_file_name("texture.png").save_file(),
            Dialog::ExportBmat => picker.set_file_name("material.bmat").save_file(),
            Dialog::Open | Dialog::Import => picker.pick_file(),
            _ => picker
                .set_file_name(
                    editor
                        .path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy(),
                )
                .save_file(),
        };
        let Some(path) = chosen else {
            return;
        };
        editor.picked_path = path;
        if matches!(kind, Dialog::Import) {
            let result = std::fs::read(&editor.picked_path)
                .map_err(|e| e.to_string())
                .and_then(|bytes| decode(&editor.picked_path.to_string_lossy(), &bytes));
            match result {
                Ok(pixels) => {
                    let channels = pixels.channels as usize;
                    let mut mapping = [
                        ChannelSource::R,
                        ChannelSource::G,
                        ChannelSource::B,
                        ChannelSource::A,
                    ];
                    if pixels.format.starts_with("PNG") && channels == 2 {
                        mapping[1] = ChannelSource::A;
                    }
                    editor.import_draft = Some(ImportDraft {
                        pixels,
                        channels,
                        mapping,
                        name: editor
                            .picked_path
                            .file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                        srgb: false,
                        preview: None,
                    });
                }
                Err(err) => {
                    editor.status = err;
                    return;
                }
            }
        }
    } else {
        editor.picked_path = editor.path.clone();
    }
    editor.dialog = Some(kind);
    editor.overwrite = false;
    editor.discard = false;
}
fn embedded_picker(
    ui: &mut egui::Ui,
    label: &str,
    source: &mut String,
    sources: &[String],
    selected: &mut Option<(String, Option<usize>)>,
) {
    egui::ComboBox::from_id_salt((label, "embedded"))
        .selected_text(source.as_str())
        .show_ui(ui, |ui| {
            for item in sources {
                ui.selectable_value(source, item.clone(), item);
            }
        });
    if ui.small_button("View texture").clicked() {
        *selected = Some((source.clone(), None));
    }
}
fn texture_slot(
    ui: &mut egui::Ui,
    label: &str,
    slot: &mut Option<String>,
    sources: &[String],
    selected: &mut Option<(String, Option<usize>)>,
) {
    ui.label(label);
    egui::ComboBox::from_id_salt(label)
        .selected_text(
            slot.as_deref()
                .unwrap_or(if label == "Base color" || label == "Emissive" {
                    "Constant"
                } else {
                    "None (default)"
                }),
        )
        .show_ui(ui, |ui| {
            ui.selectable_value(
                slot,
                None,
                if label == "Base color" || label == "Emissive" {
                    "Constant"
                } else {
                    "None (default)"
                },
            );
            for source in sources {
                ui.selectable_value(slot, Some(source.clone()), source);
            }
        });
    if let Some(src) = slot {
        if ui.small_button(format!("View {label}")).clicked() {
            *selected = Some((src.clone(), None));
        }
    }
}
fn coat_input(
    ui: &mut egui::Ui,
    label: &str,
    factor: &mut Option<f32>,
    texture: &mut Option<String>,
    default_value: f32,
    sources: &[String],
    selected: &mut Option<(String, Option<usize>)>,
) {
    ui.push_id(label, |ui| {
        ui.label(label);
        let mut mode = if texture.is_some() {
            2
        } else if factor.is_some() {
            1
        } else {
            0
        };
        let before = mode;
        ui.horizontal(|ui| {
            ui.selectable_value(&mut mode, 0, "None");
            ui.selectable_value(&mut mode, 1, "Constant");
            ui.add_enabled_ui(!sources.is_empty(), |ui| {
                ui.selectable_value(&mut mode, 2, "Mask");
            });
        });
        if mode != before {
            match mode {
                0 => {
                    *factor = None;
                    *texture = None;
                }
                1 => {
                    *texture = None;
                    factor.get_or_insert(default_value);
                }
                _ => {
                    *texture = sources.first().cloned();
                    *factor = Some(1.);
                }
            }
        }
        if mode == 1 {
            if let Some(v) = factor {
                ui.add(egui::Slider::new(v, 0.0..=1.0));
            }
        } else if mode == 2 {
            if let Some(src) = texture {
                embedded_picker(ui, label, src, sources, selected);
            }
            // Preserve authored multipliers when opening an existing material.
            if let Some(v) = factor {
                ui.add(egui::Slider::new(v, 0.0..=1.0).text("Multiplier"));
            }
        }
    });
}
fn optional_number(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut Option<f32>,
    initial: f32,
    range: std::ops::RangeInclusive<f32>,
) {
    ui.push_id(label, |ui| {
        ui.label(label);
        ui.horizontal(|ui| {
            if ui.selectable_label(value.is_none(), "None").clicked() {
                *value = None;
            }
            if ui.selectable_label(value.is_some(), "Value").clicked() && value.is_none() {
                *value = Some(initial);
            }
            if let Some(v) = value {
                ui.add(egui::DragValue::new(v).speed(0.01).range(range));
            }
        });
    });
}
fn scalar(
    ui: &mut egui::Ui,
    label: &str,
    input: &mut ScalarInput,
    sources: &[String],
    selected: &mut Option<(String, Option<usize>)>,
) {
    ui.separator();
    ui.label(label);
    let mut texture = match input {
        ScalarInput::None => 0,
        ScalarInput::Constant(_) => 1,
        ScalarInput::Texture { .. } => 2,
    };
    let before = texture;
    ui.horizontal(|ui| {
        ui.selectable_value(&mut texture, 0, "None");
        ui.selectable_value(&mut texture, 1, "Constant");
        ui.add_enabled_ui(!sources.is_empty(), |ui| {
            ui.selectable_value(&mut texture, 2, "Mask");
        });
    });
    if texture != before {
        *input = if texture == 0 {
            ScalarInput::None
        } else if texture == 2 {
            ScalarInput::Texture {
                source: sources[0].clone(),
                channel: 0,
            }
        } else {
            ScalarInput::Constant(1.)
        };
    }
    match input {
        ScalarInput::None => {
            ui.label("Uses the material default");
        }
        ScalarInput::Constant(v) => {
            ui.add(egui::Slider::new(v, 0. ..=1.).text("Value"));
        }
        ScalarInput::Texture { source, channel } => {
            egui::ComboBox::from_id_salt((label, "source"))
                .selected_text(source.as_str())
                .show_ui(ui, |ui| {
                    for s in sources {
                        ui.selectable_value(source, s.clone(), s);
                    }
                });
            ui.horizontal(|ui| {
                for (i, name) in ["R", "G", "B", "A"].iter().enumerate() {
                    ui.selectable_value(channel, i, *name);
                }
            });
            if ui.small_button("View mask").clicked() {
                *selected = Some((source.clone(), Some(*channel)));
            }
        }
    }
}
fn changed(e: &mut Editor) {
    e.dirty = e.doc != e.saved;
    e.rebuild = true;
    e.thumbnails.clear();
    e.texture_info.clear();
    if let Some(tab) = e.tabs.get_mut(e.active_tab) {
        tab.doc = e.doc.clone();
        tab.saved = e.saved.clone();
        tab.dirty = e.dirty;
    }
    if e.selected
        .as_ref()
        .is_some_and(|(key, _)| !e.doc.entries.contains_key(key))
    {
        e.selected = None;
    }
}
/// Restore the last on-disk project snapshot when the user discards edits.
/// This also undoes edits made by an external tool after the project was opened.
fn restore_discarded_project(e: &mut Editor) -> bool {
    if !(e.project && e.dirty) { return true; }
    match e.saved.save_project(&e.path, &e.export_path) {
        Ok(()) => true,
        Err(err) => { e.status = format!("Could not restore discarded project: {err}"); false }
    }
}
fn sync_tab(e: &mut Editor) {
    if let Some(tab) = e.tabs.get_mut(e.active_tab) {
        tab.path = e.path.clone();
        tab.project = e.project;
        tab.export_path = e.export_path.clone();
        tab.doc = e.doc.clone();
        tab.saved = e.saved.clone();
        tab.dirty = e.dirty;
    }
}
fn switch_tab(e: &mut Editor, index: usize) {
    if index >= e.tabs.len() || index == e.active_tab { return; }
    sync_tab(e);
    let tab = e.tabs[index].clone();
    e.active_tab = index;
    e.path = tab.path;
    e.project = tab.project;
    e.export_path = tab.export_path;
    e.doc = tab.doc;
    e.saved = tab.saved;
    e.dirty = tab.dirty;
    e.history = History::default();
    e.selected = None;
    e.thumbnails.clear();
    e.texture_info.clear();
    e.rebuild = true;
    e.status = "Switched material tab".into();
    if e.project { set_project_watch(e, e.path.clone()); } else { e.watch = None; }
}
fn open_material(e: &mut Editor, path: PathBuf) {
    let existing = e.tabs.iter().position(|tab| tab.path == path);
    if let Some(index) = existing { switch_tab(e, index); return; }
    let loaded = if path.is_dir() {
        Document::open_project(&path).map(|(doc, manifest)| (doc, true, manifest.export_path))
    } else {
        Document::open(&path).map(|doc| (doc, false, PathBuf::new()))
    };
    let Ok((doc, project, export_path)) = loaded else {
        e.status = format!("Could not open {}", path.display());
        return;
    };
    sync_tab(e);
    e.tabs.push(MaterialTab { path: path.clone(), project, export_path: export_path.clone(), doc: doc.clone(), saved: doc.clone(), dirty: false });
    e.active_tab = e.tabs.len() - 1;
    e.path = path.clone();
    e.project = project;
    e.export_path = export_path;
    e.doc = doc.clone();
    e.saved = doc;
    e.dirty = false;
    e.history = History::default();
    e.selected = None;
    e.thumbnails.clear();
    e.texture_info.clear();
    e.rebuild = true;
    e.status = format!("Opened {}", path.display());
    if project { set_project_watch(e, path); } else { e.watch = None; }
}
fn history_step(e: &mut Editor, redo: bool) {
    let success = if redo {
        e.history.redo(&mut e.doc)
    } else {
        e.history.undo(&mut e.doc)
    };
    if success {
        changed(e);
        e.editing_gesture = false;
        if let Some(w) = &mut e.watch {
            w.enabled = false;
        }
        e.status = if redo {
            "Redo (folder watching paused)"
        } else {
            "Undo (folder watching paused)"
        }
        .into();
    }
}
fn set_watch(e: &mut Editor, path: PathBuf) {
    let last = workflow::folder_stamp(&path).unwrap_or_default();
    e.watch = Some(WatchFolder {
        path,
        enabled: false,
        last,
        pending: None,
        next: Instant::now(),
        project: false,
    });
}
fn set_project_watch(e: &mut Editor, path: PathBuf) {
    let last = workflow::project_stamp(&path).unwrap_or_default();
    e.watch = Some(WatchFolder { path, enabled: true, last, pending: None, next: Instant::now(), project: true });
}
fn poll_folder(e: &mut Editor) {
    if e.dialog.is_some() || e.texture_action.is_some() {
        return;
    }
    let Some(w) = &mut e.watch else {
        return;
    };
    let project_watch = w.project;
    if !w.enabled || Instant::now() < w.next {
        return;
    }
    w.next = Instant::now() + Duration::from_secs(1);
    let stamp = match if w.project { workflow::project_stamp(&w.path) } else { workflow::folder_stamp(&w.path) } {
        Ok(s) => s,
        Err(err) => {
            e.status = format!("Folder reload paused: {err}");
            w.enabled = false;
            return;
        }
    };
    if stamp == w.last {
        w.pending = None;
        return;
    }
    // Require two stable observations so an external editor can finish writing.
    if w.pending.as_ref() != Some(&stamp) {
        w.pending = Some(stamp);
        return;
    }
    let previous = e.doc.clone();
    let result = if w.project {
        Document::open_project(&w.path).map(|(updated, _)| {
            let changed = updated != e.doc;
            if changed { e.doc = updated; }
            changed
        })
    } else {
        workflow::reload_folder(&mut e.doc, &w.path)
    };
    match result {
        Ok(updated) => {
            w.last = stamp;
            w.pending = None;
            if updated {
                e.history.record(previous);
                changed(e);
                e.status = if project_watch {
                    "Project changed externally; review, save, or discard to restore the previous files".into()
                } else {
                    "External textures reloaded (undoable; BMAT not saved automatically)".into()
                };
            }
        }
        Err(err) => {
            w.enabled = false;
            e.status = format!("Folder reload paused; previous textures retained: {err}");
        }
    }
}
fn texture_action_ui(ctx: &egui::Context, e: &mut Editor) {
    let Some((source, mut name, delete)) = e.texture_action.clone() else {
        return;
    };
    let mut confirm = false;
    let mut cancel = false;
    egui::Window::new(if delete {
        "Delete embedded texture"
    } else {
        "Rename embedded texture"
    })
    .collapsible(false)
    .show(ctx, |ui| {
        ui.label(&source);
        if delete {
            ui.label(
                "Remove this texture and clear all its material references? This can be undone.",
            );
        } else {
            ui.text_edit_singleline(&mut name);
            ui.small("All material references will follow the new name.");
        }
        ui.horizontal(|ui| {
            confirm = ui
                .button(if delete { "Delete" } else { "Rename" })
                .clicked();
            cancel = ui.button("Cancel").clicked();
        });
    });
    if cancel {
        e.texture_action = None;
        return;
    }
    e.texture_action = Some((source.clone(), name.clone(), delete));
    if confirm {
        let previous = e.doc.clone();
        let result = if delete {
            e.doc.delete_texture(&source).map(|()| None)
        } else {
            e.doc.rename_texture(&source, &name).map(Some)
        };
        match result {
            Ok(key) => {
                if previous != e.doc {
                    e.history.record(previous);
                }
                e.selected = key.map(|key| (key, None));
                changed(e);
                e.texture_action = None;
                if let Some(w) = &mut e.watch {
                    w.enabled = false;
                }
                e.status = "Texture updated; export a fresh folder before watching renamed/deleted textures".into();
            }
            Err(err) => e.status = err,
        }
    }
}
fn ui(
    mut contexts: EguiContexts,
    mut editor: ResMut<Editor>,
    mut cameras: Query<(&mut Camera, &mut Transform), With<Camera3d>>,
    mut orbit: ResMut<OrbitCamera>,
    windows: Query<&Window>,
    mut exit: MessageWriter<AppExit>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let e = &mut *editor;
    poll_folder(e);
    if !ctx.egui_wants_keyboard_input() && e.dialog.is_none() && e.texture_action.is_none() {
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z)) {
            history_step(e, false);
        }
        if ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y)
                || i.consume_key(
                    egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                    egui::Key::Z,
                )
        }) {
            history_step(e, true);
        }
    }
    let mut root_ui = egui::Ui::new(
        ctx.clone(),
        egui::Id::new("editor-root"),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.content_rect()),
    );
    let rect = egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show_inside(&mut root_ui, |root| {
            egui::Panel::top("menu").show_inside(root, |ui| {
                egui::MenuBar::new().ui(ui, |ui| {
                    ui.menu_button("File", |ui| {
                        if ui.button("Export texture folder…").clicked() { dialog(e, Dialog::ExportFolder); ui.close(); }
                        if ui.button("Reimport texture folder…").clicked() { dialog(e, Dialog::ImportFolder); ui.close(); }
                        if ui.button("New…").clicked() {
                            dialog(e, Dialog::New);
                            ui.close();
                        }
                        if ui.button("Open BMAT…").clicked() {
                            dialog(e, Dialog::Open);
                            ui.close();
                        }
                        if ui.button("Load Project…").clicked() {
                            dialog(e, Dialog::OpenProject);
                            ui.close();
                        }
                        if ui.button("Save Project    Ctrl+S").clicked() {
                            save(e, e.path.clone());
                            ui.close();
                        }
                        if ui.button("Save Project As…").clicked() {
                            dialog(e, Dialog::SaveProjectAs);
                            ui.close();
                        }
                        if ui.button("Export BMAT…").clicked() {
                            dialog(e, Dialog::ExportBmat);
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Import texture…").clicked() {
                            dialog(e, Dialog::Import);
                            ui.close();
                        }
                        if ui.button("Quit").clicked() {
                            dialog(e, Dialog::Exit);
                            ui.close();
                        }
                    });
                    ui.menu_button("Edit", |ui| {
                        if ui.add_enabled(e.history.can_undo(), egui::Button::new("Undo    Ctrl+Z")).clicked() { history_step(e, false); ui.close(); }
                        if ui.add_enabled(e.history.can_redo(), egui::Button::new("Redo    Ctrl+Shift+Z")).clicked() { history_step(e, true); ui.close(); }
                    });
                    ui.label(format!(
                        "{}{}",
                        e.path.display(),
                        if e.dirty { " *" } else { "" }
                    ));
                    ui.checkbox(&mut e.rotate, "Rotate preview");
                    if ui.button("Reset view").clicked() { *orbit = OrbitCamera::default(); }
                    ui.label("Drag: orbit · Shift-drag/middle: pan · Scroll: zoom");
                });
            });
            egui::Panel::top("tabs").show_inside(root, |ui| {
                egui::ScrollArea::horizontal()
                    .id_salt("material-tabs-scroll")
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for index in 0..e.tabs.len() {
                                let tab = &e.tabs[index];
                                let name = tab.path.file_name().unwrap_or_default().to_string_lossy();
                                let label = format!("{}{}", name, if tab.dirty { " *" } else { "" });
                                if ui.selectable_label(index == e.active_tab, label).clicked() {
                                    switch_tab(e, index);
                                }
                            }
                        });
                    }
                );
            });
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::S)) {
                save(e, e.path.clone());
            }
            egui::Panel::bottom("status").show_inside(root, |ui| {
                ui.label(&e.status);
                if let Some(w) = &mut e.watch {
                    ui.checkbox(&mut w.enabled, if w.project { "Watch project files" } else { "Watch exported textures" });
                    ui.small(w.path.display().to_string());
                }
            });
            egui::Panel::left("explorer")
                .default_size(220.)
                .size_range(160. ..=320.)
                .show_inside(root, |ui| {
                    ui.heading("Materials");
                    ui.small(e.explorer_root.display().to_string());
                    let mut projects = Vec::new();
                    if let Ok(entries) = std::fs::read_dir(&e.explorer_root) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.is_dir() && path.join(workflow::PROJECT_MANIFEST).is_file() {
                                projects.push(path);
                            }
                        }
                    }
                    projects.sort();
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for project in projects {
                            let selected = e.project && e.path == project;
                            let name = project.file_name().unwrap_or_default().to_string_lossy();
                            let response = ui.selectable_label(selected, name);
                            if response.double_clicked() {
                                open_material(e, project);
                            }
                        }
                    });
                });
            let before = e.doc.settings.clone();
            let sources: Vec<_> = e
                .doc
                .entries
                .keys()
                .filter(|s| s.starts_with("sources/"))
                .cloned()
                .collect();
            egui::Panel::left("inspector")
                .default_size(330.)
                .size_range(250. ..=500.)
                .show_inside(root, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.heading("Material");
                        if ui.button("Import texture…").clicked() {
                            dialog(e, Dialog::Import);
                        }
                        let s = &mut e.doc.settings;
                        ui.collapsing("Alpha", |ui| {
                            let mut choice = if s.alpha_default { None } else if s.alpha_none { Some(BmatAlphaMode::Mask) } else { Some(s.alpha) };
                            let before = choice;
                            egui::ComboBox::from_label("Alpha mode")
                                .selected_text(choice.map_or_else(|| "Default (Opaque)".into(), |a| format!("{a:?}")))
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut choice, None, "Default (Opaque)");
                                    for a in [
                                        BmatAlphaMode::Opaque,
                                        BmatAlphaMode::Mask,
                                        BmatAlphaMode::Blend,
                                    ] {
                                        ui.selectable_value(&mut choice, Some(a), format!("{a:?}"));
                                    }
                                });
                            if choice != before {
                                s.alpha_default = choice.is_none();
                                s.alpha_none = false;
                                s.alpha = choice.unwrap_or(BmatAlphaMode::Opaque);
                            }
                            ui.label("Alpha value");
                            let mut constant = s.pbr.alpha_factor.is_some();
                            ui.horizontal(|ui| {
                                ui.selectable_value(&mut constant, false, "None");
                                ui.selectable_value(&mut constant, true, "Constant");
                            });
                            if constant {
                                let value = s.pbr.alpha_factor.get_or_insert(1.);
                                ui.add(egui::Slider::new(value, 0.0..=1.0));
                            } else { s.pbr.alpha_factor = None; }
                            ui.small("Multiplies texture alpha. Blend shows transparency; Mask applies its cutoff. Opaque ignores alpha.");
                        });
                        ui.collapsing("Base color", |ui| {
                            let mut mode = if s.albedo_none { 0 } else if s.albedo.is_some() { 2 } else { 1 };
                            let before = mode;
                            ui.horizontal(|ui| {
                                ui.selectable_value(&mut mode, 0, "None");
                                ui.selectable_value(&mut mode, 1, "Constant");
                                ui.add_enabled_ui(!sources.is_empty(), |ui| { ui.selectable_value(&mut mode, 2, "Texture"); });
                            });
                            if mode != before {
                                s.albedo_none = mode == 0;
                                if mode == 1 { s.albedo = None; }
                                if mode == 2 && s.albedo.is_none() { s.albedo = sources.first().cloned(); }
                            }
                            if mode == 2 {
                                if let Some(source) = &mut s.albedo { embedded_picker(ui, "Base color", source, &sources, &mut e.selected); }
                            } else if mode == 1 {
                                let mut color = s.color.map(|v| (v * 255.).round() as u8);
                                if ui
                                    .color_edit_button_srgba_unmultiplied(&mut color)
                                    .changed()
                                {
                                    s.color = color.map(|v| v as f32 / 255.);
                                }
                            }
                        });
                        ui.collapsing("Normal", |ui| {
                            let mut texture = s.normal.is_some();
                            ui.horizontal(|ui| {
                                ui.selectable_value(&mut texture, false, "None");
                                ui.add_enabled_ui(!sources.is_empty(), |ui| { ui.selectable_value(&mut texture, true, "Texture"); });
                            });
                            if !texture { s.normal = None; }
                            else if s.normal.is_none() { s.normal = sources.first().cloned(); }
                            if let Some(source) = &mut s.normal { embedded_picker(ui, "Normal", source, &sources, &mut e.selected); }
                        });
                        ui.collapsing("Metallic", |ui| {
                            scalar(ui, "Metallic", &mut s.metallic, &sources, &mut e.selected);
                        });
                        ui.collapsing("Roughness", |ui| {
                            scalar(ui, "Roughness", &mut s.roughness, &sources, &mut e.selected);
                        });
                        ui.collapsing("Occlusion", |ui| {
                            scalar(ui, "Occlusion", &mut s.occlusion, &sources, &mut e.selected);
                        });
                        ui.collapsing("Emissive", |ui| {
                        ui.checkbox(&mut s.emissive_none, "None (default)");
                        ui.add_enabled_ui(!s.emissive_none, |ui| {
                            texture_slot(
                                ui,
                                "Emissive",
                                &mut s.emissive,
                                &sources,
                                &mut e.selected,
                            );
                            if s.emissive.is_none() {
                                ui.color_edit_button_rgb(&mut s.emission_color);
                            }
                        });
                        });
                        ui.separator();
                        ui.collapsing("Clearcoat", |ui| {
                            coat_input(ui, "Strength (mask R)", &mut s.pbr.clearcoat, &mut s.pbr.clearcoat_texture, 0., &sources, &mut e.selected);
                            coat_input(ui, "Roughness (mask G)", &mut s.pbr.clearcoat_perceptual_roughness, &mut s.pbr.clearcoat_roughness_texture, 0.5, &sources, &mut e.selected);
                            ui.label("Coat normal");
                            let mut texture = s.pbr.clearcoat_normal_texture.is_some();
                            ui.horizontal(|ui| {
                                ui.selectable_value(&mut texture, false, "None");
                                ui.add_enabled_ui(!sources.is_empty(), |ui| { ui.selectable_value(&mut texture, true, "Texture"); });
                            });
                            if !texture { s.pbr.clearcoat_normal_texture = None; }
                            else if s.pbr.clearcoat_normal_texture.is_none() { s.pbr.clearcoat_normal_texture = sources.first().cloned(); }
                            if let Some(src) = &mut s.pbr.clearcoat_normal_texture { embedded_picker(ui, "Coat normal", src, &sources, &mut e.selected); }
                            ui.label("Maps multiply their factors. None strength uses 0 (no coat); None coat roughness uses 0.5.");
                        });
                        ui.collapsing("Transmission / volume", |ui| {
                            optional_number(ui, "Diffuse transmission", &mut s.pbr.diffuse_transmission, 0.0, 0.0..=1.0);
                            optional_number(ui, "Specular transmission", &mut s.pbr.specular_transmission, 0.0, 0.0..=1.0);
                            optional_number(ui, "Thickness", &mut s.pbr.thickness, 0.0, 0.0..=f32::MAX);
                            optional_number(ui, "IOR", &mut s.pbr.ior, 1.5, 1.0..=f32::MAX);
                            optional_number(ui, "Attenuation distance", &mut s.pbr.attenuation_distance, 1.0, f32::MIN_POSITIVE..=f32::MAX);
                            texture_slot(ui, "Diffuse transmission map (A)", &mut s.pbr.diffuse_transmission_texture, &sources, &mut e.selected);
                            texture_slot(ui, "Specular transmission map (R)", &mut s.pbr.specular_transmission_texture, &sources, &mut e.selected);
                            texture_slot(ui, "Thickness map (G)", &mut s.pbr.thickness_texture, &sources, &mut e.selected);
                            let mut none = s.pbr.attenuation_color.is_none();
                            if ui.checkbox(&mut none, "Attenuation color: None (default)").changed() {
                                s.pbr.attenuation_color = if none { None } else { Some([1.;3]) };
                            }
                            if let Some(color) = &mut s.pbr.attenuation_color { ui.color_edit_button_rgb(color); }
                            ui.label("Texture samples multiply their factors. Set a nonzero transmission and thickness to see refraction. None IOR uses 1.5; None attenuation distance means no absorption.");

                        });
                        ui.collapsing("Anisotropy", |ui| {
                            optional_number(ui, "Anisotropy strength", &mut s.pbr.anisotropy_strength, 0.0, 0.0..=1.0);
                            optional_number(ui, "Rotation (radians)", &mut s.pbr.anisotropy_rotation, 0.0, -f32::MAX..=f32::MAX);
                            texture_slot(ui, "Anisotropy map (RG direction, B strength)", &mut s.pbr.anisotropy_texture, &sources, &mut e.selected);
                            ui.label("Requires mesh tangents. Map direction uses RG in [0,1] mapped to [-1,1]; B multiplies strength.");
                        });
                        ui.collapsing("Data", |ui| {
                            texture_slot(ui, "Data", &mut s.data, &sources, &mut e.selected);
                        });
                        ui.collapsing("Embedded textures", |ui| {
                            for src in &sources {
                                let info = e.texture_info.entry(src.clone()).or_insert_with(|| match e.doc.pixels(src) {
                                    Ok(p) => format!("{} × {} · {} channels · {} · {}", p.width, p.height, p.channels, ["", "R", "RG", "RGB", "RGBA"][p.channels.min(4) as usize], p.format),
                                    Err(err) => err,
                                });
                                if ui
                                    .selectable_label(
                                        e.selected.as_ref().is_some_and(|(n, _)| n == src),
                                        src,
                                    )
                                    .clicked()
                                {
                                    e.selected = Some((src.clone(), None));
                                }
                                ui.small(info.as_str());
                            }
                        });
                        if let Some((src, channel)) = e.selected.clone() {
                            ui.separator();
                            ui.label(&src);
                            ui.horizontal(|ui| {
                                if ui.button("Export PNG…").clicked() { dialog(e, Dialog::ExportTexture); }
                                if ui.button("Rename…").clicked() { e.texture_action = Some((src.clone(), std::path::Path::new(&src).file_stem().unwrap_or_default().to_string_lossy().into_owned(), false)); }
                                if ui.button("Delete…").clicked() { e.texture_action = Some((src.clone(), String::new(), true)); }
                            });
                            let key = format!("{src}:{channel:?}");
                            if !e.thumbnails.contains_key(&key) {
                                match e.doc.pixels(&src) {
                                    Ok(p) => {
                                        let scale = (512. / p.width.max(p.height) as f32).min(1.);
                                        let w = (p.width as f32 * scale).round().max(1.) as usize;
                                        let h = (p.height as f32 * scale).round().max(1.) as usize;
                                        let colors = (0..h)
                                            .flat_map(|y| {
                                                let p = &p;
                                                (0..w).map(move |x| {
                                                    let mut c = p.rgba[(y * p.height / h)
                                                        * p.width
                                                        + x * p.width / w];
                                                    if let Some(ch) = channel {
                                                        c = [c[ch], c[ch], c[ch], 255];
                                                    } else if p.channels == 1 {
                                                        c = [c[0], c[0], c[0], 255];
                                                    } else if p.channels == 2 {
                                                        c = [c[0], c[1], 0, 255];
                                                    }
                                                    let checker = if (x / 12 + y / 12) % 2 == 0 {
                                                        90.
                                                    } else {
                                                        150.
                                                    };
                                                    let a = c[3] as f32 / 255.;
                                                    egui::Color32::from_rgb(
                                                        (c[0] as f32 * a + checker * (1. - a))
                                                            as u8,
                                                        (c[1] as f32 * a + checker * (1. - a))
                                                            as u8,
                                                        (c[2] as f32 * a + checker * (1. - a))
                                                            as u8,
                                                    )
                                                })
                                            })
                                            .collect();
                                        e.thumbnails.insert(
                                            key.clone(),
                                            ctx.load_texture(
                                                &key,
                                                egui::ColorImage::new([w, h], colors),
                                                egui::TextureOptions::NEAREST,
                                            ),
                                        );
                                        e.status = format!("{} × {} pixels", p.width, p.height);
                                    }
                                    Err(err) => e.status = err,
                                }
                            }
                            if let Some(texture) = e.thumbnails.get(&key) {
                                ui.add(egui::Image::new(texture).max_width(ui.available_width()));
                            }
                        }
                    });
                });
            if before != e.doc.settings {
                if !e.editing_gesture {
                    let mut previous = e.doc.clone(); previous.settings = before;
                    e.history.record(previous);
                }
                e.editing_gesture = ctx.input(|i| i.pointer.any_down());
                e.dirty = true;
                e.rebuild = true;
            }
            let rect = root.available_rect_before_wrap();
            let response = root.interact(rect, egui::Id::new("material-orbit-viewport"), egui::Sense::click_and_drag());
            if e.dialog.is_none() && e.texture_action.is_none() {
                if response.double_clicked() { *orbit = OrbitCamera::default(); }
                let (delta, shift, scroll) = ctx.input(|i| (i.pointer.delta(), i.modifiers.shift, i.smooth_scroll_delta.y));
                if response.dragged_by(egui::PointerButton::Middle) || (shift && response.dragged_by(egui::PointerButton::Primary)) {
                    orbit.pan(delta, rect.height());
                } else if response.dragged_by(egui::PointerButton::Primary) || response.dragged_by(egui::PointerButton::Secondary) {
                    orbit.orbit(delta);
                }
                if response.hovered() { orbit.zoom(scroll); }
            }
            rect
        })
        .inner;
    if !ctx.input(|i| i.pointer.any_down()) {
        e.editing_gesture = false;
    }
    texture_action_ui(ctx, e);
    if let Some(kind) = e.dialog {
        egui::Window::new(match kind {
            Dialog::OpenProject => "Open BMAT project folder",
            Dialog::SaveProjectAs => "Save BMAT project folder as",
            Dialog::ExportBmat => "Export runtime BMAT",
            Dialog::ExportFolder => "Export to an empty texture directory",
            Dialog::ImportFolder => "Reimport texture directory",
            Dialog::ExportTexture => "Export texture PNG",
            Dialog::New => "New BMAT",
            Dialog::Exit => "Quit editor",
            Dialog::Open => "Open BMAT",
            Dialog::Import => "Import PNG or KTX2",
        })
        .collapsible(false)
        .resizable(true)
        .show(ctx, |ui| {
            if !matches!(kind, Dialog::Exit) {
                ui.label("File path");
                ui.label(e.picked_path.display().to_string());
            }
            let path = e.picked_path.clone();
            if matches!(kind, Dialog::Import) {
                if let Some(draft) = &mut e.import_draft {
                    import_options(ui, draft);
                }
            }
            let needs_overwrite = matches!(kind, Dialog::ExportTexture | Dialog::ExportBmat) && path.exists();
            if needs_overwrite {
                ui.checkbox(&mut e.overwrite, "Replace the existing file");
            }
            let needs_discard =
                matches!(kind, Dialog::Open | Dialog::OpenProject | Dialog::New | Dialog::Exit | Dialog::ImportFolder) && e.dirty;
            if needs_discard {
                ui.checkbox(&mut e.discard, "Discard unsaved changes");
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        (matches!(kind, Dialog::Exit) || !e.picked_path.as_os_str().is_empty())
                            && (!needs_overwrite || e.overwrite)
                            && (!needs_discard || e.discard),
                        egui::Button::new("Confirm"),
                    )
                    .clicked()
                {
                    let previous = e.doc.clone();
                    match kind {
                        Dialog::SaveProjectAs => {
                            let export_path = if e.export_path.as_os_str().is_empty() { PathBuf::from("build/material.bmat") } else { e.export_path.clone() };
                            match e.doc.save_project(&path, &export_path) {
                                Ok(()) => { e.project = true; e.path = path; e.export_path = export_path; e.saved = e.doc.clone(); e.dirty = false; e.dialog = None; e.status = "Project saved".into(); }
                                Err(err) => e.status = err,
                            }
                        },
                        Dialog::ExportBmat => match e.doc.save(&path) {
                            Ok(()) => { e.dialog = None; e.status = "BMAT exported".into(); }
                            Err(err) => e.status = err,
                        },
                        Dialog::OpenProject => if restore_discarded_project(e) { match Document::open_project(&path) {
                            Ok((doc, manifest)) => {
                                e.doc = doc;
                                e.path = path;
                                e.project = true;
                                e.export_path = manifest.export_path;
                                e.saved = e.doc.clone();
                                e.history = History::default();
                                e.dirty = false;
                                e.rebuild = true;
                                e.selected = None;
                                e.thumbnails.clear();
                                e.texture_info.clear();
                                set_project_watch(e, e.path.clone());
                                e.dialog = None;
                                e.status = "Opened BMAT project".into();
                            }
                            Err(err) => e.status = err,
                        } },
                        Dialog::ExportFolder => match workflow::export_folder(&e.doc, &path) {
                            Ok(()) => { set_watch(e, path); e.dialog = None; e.status = "Texture folder exported. Enable watching to reload external edits.".into(); }
                            Err(err) => e.status = err,
                        },
                        Dialog::ImportFolder => match workflow::import_folder(&path) {
                            Ok(doc) => { e.doc = doc; changed(e); set_watch(e, path); e.dialog = None; e.status = "Texture folder reimported (not yet saved)".into(); }
                            Err(err) => e.status = err,
                        },
                        Dialog::ExportTexture => {
                            let result = e.export_key.as_ref().ok_or_else(|| "Select a texture first".to_string()).and_then(|src| e.doc.texture_png(src)).and_then(|bytes| workflow::write_export(&path, &bytes));
                            match result { Ok(()) => { e.dialog = None; e.status = "PNG exported".into(); }, Err(err) => e.status = err }
                        },
                        Dialog::Exit => {
                            if restore_discarded_project(e) { exit.write(AppExit::Success); }
                        }
                        Dialog::New => {
                            if !restore_discarded_project(e) {
                                e.dialog = None;
                            } else if path.exists() {
                                e.status = "Use Open for an existing file".into();
                            } else {
                                e.doc = Document::default();
                                e.history = History::default(); e.watch = None;
                                e.path = path;
                                e.project = false;
                                e.export_path.clear();
                                e.dirty = true;
                                e.rebuild = true;
                                e.selected = None;
                                e.thumbnails.clear();
                                e.texture_info.clear();
                                e.dialog = None;
                                e.status = "New material".into();
                            }
                        }
                        Dialog::Open => if restore_discarded_project(e) { match Document::open(&path) {
                            Ok(doc) => {
                                e.doc = doc;
                                e.history = History::default(); e.watch = None; e.saved = e.doc.clone();
                                e.path = path;
                                e.project = false;
                                e.export_path.clear();
                                e.dirty = false;
                                e.rebuild = true;
                                e.selected = None;
                                e.thumbnails.clear();
                                e.texture_info.clear();
                                e.dialog = None;
                                e.status = "Opened material".into();
                            }
                            Err(err) => e.status = err,
                        } },
                        Dialog::Import => match e
                            .import_draft
                            .as_ref()
                            .ok_or_else(|| "Select a texture first".to_string())
                            .and_then(|d| {
                                e.doc.import_mapped(
                                    &d.name,
                                    &d.pixels,
                                    &d.mapping[..d.channels],
                                    d.srgb,
                                )
                            }) {
                            Ok(src) => {
                                e.selected = Some((src, None));
                                e.dirty = true;
                                e.dialog = None;
                                e.import_draft = None;
                                e.status =
                                    "Texture embedded — choose it in a material input".into();
                            }
                            Err(err) => e.status = err,
                        },
                    }
                    if !matches!(kind, Dialog::Open | Dialog::New) && previous != e.doc { e.history.record(previous); }
                }
                if ui.button("Cancel").clicked() {
                    e.dialog = None;
                    e.import_draft = None;
                }
            });
            ui.label(&e.status);
        });
    }
    if let Ok(window) = windows.single() {
        let scale = window.scale_factor() * ctx.zoom_factor();
        let pos = UVec2::new((rect.min.x * scale) as u32, (rect.min.y * scale) as u32);
        let size = UVec2::new(
            (rect.width() * scale) as u32,
            (rect.height() * scale) as u32,
        )
        .min(UVec2::new(window.physical_width(), window.physical_height()).saturating_sub(pos));
        if size.x > 0 && size.y > 0 {
            for (mut camera, mut transform) in &mut cameras {
                *transform = orbit.transform();
                camera.viewport = Some(Viewport {
                    physical_position: pos,
                    physical_size: size,
                    ..default()
                });
            }
        }
    }
    Ok(())
}
