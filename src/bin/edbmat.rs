use bevy::{
    camera::{CameraOutputMode, Viewport, visibility::RenderLayers},
    image::ImageAddressMode,
    prelude::*,
    render::render_resource::BlendState,
    window::WindowCloseRequested,
};
use bevy_egui::{
    EguiContexts, EguiGlobalSettings, EguiPlugin, EguiPrimaryContextPass, PrimaryEguiContext, egui,
};
use bmat::{
    BmatAlphaMode,
    editor::{ChannelSource, Document, Pixels, ScalarInput, decode},
    image_from_ktx2,
};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Resource)]
struct Editor {
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
    New,
    Open,
    SaveAs,
    Import,
    Exit,
}
#[derive(Component)]
struct Cube;

fn main() {
    let path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| "material.bmat".into());
    let (doc, status) = if path.exists() {
        match Document::open(&path) {
            Ok(doc) => (doc, "Opened material".into()),
            Err(e) => {
                eprintln!("Cannot open {}: {e}", path.display());
                std::process::exit(1);
            }
        }
    } else {
        (
            Document::default(),
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
            import_draft: None,
            texture_info: BTreeMap::new(),
            path,
            doc,
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
        })
        .insert_resource(ClearColor(Color::srgb(0.075, 0.085, 0.105)))
        .insert_resource(GlobalAmbientLight {
            brightness: 350.,
            ..default()
        })
        .add_systems(Startup, setup)
        .add_systems(Update, (rebuild, rotate, close_request).chain())
        .add_systems(EguiPrimaryContextPass, ui)
        .run();
}
fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut editor: ResMut<Editor>,
    mut egui_settings: ResMut<EguiGlobalSettings>,
) {
    egui_settings.auto_create_primary_context = false;
    editor.material = materials.add(StandardMaterial::default());
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
    match editor.doc.save(&path) {
        Ok(()) => {
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
    if !matches!(kind, Dialog::Exit) {
        let mut picker = rfd::FileDialog::new();
        if let Some(parent) = editor.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            picker = picker.set_directory(parent);
        }
        picker = if matches!(kind, Dialog::Import) {
            picker.add_filter("Texture", &["png", "ktx2"])
        } else {
            picker.add_filter("BMAT material", &["bmat"])
        };
        let chosen = match kind {
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
fn coat_input(ui: &mut egui::Ui, label: &str, factor: &mut Option<f32>, texture: &mut Option<String>, default_value: f32, sources: &[String], selected: &mut Option<(String, Option<usize>)>) {
    ui.push_id(label, |ui| {
        ui.label(label);
        let mut mode = if texture.is_some() { 2 } else if factor.is_some() { 1 } else { 0 };
        let before = mode;
        ui.horizontal(|ui| {
            ui.selectable_value(&mut mode, 0, "None");
            ui.selectable_value(&mut mode, 1, "Constant");
            ui.add_enabled_ui(!sources.is_empty(), |ui| { ui.selectable_value(&mut mode, 2, "Mask"); });
        });
        if mode != before {
            match mode {
                0 => { *factor = None; *texture = None; }
                1 => { *texture = None; factor.get_or_insert(default_value); }
                _ => { *texture = sources.first().cloned(); *factor = Some(1.); }
            }
        }
        if mode == 1 {
            if let Some(v) = factor { ui.add(egui::Slider::new(v, 0.0..=1.0)); }
        } else if mode == 2 {
            if let Some(src) = texture { embedded_picker(ui, label, src, sources, selected); }
            // Preserve authored multipliers when opening an existing material.
            if let Some(v) = factor { ui.add(egui::Slider::new(v, 0.0..=1.0).text("Multiplier")); }
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
fn ui(
    mut contexts: EguiContexts,
    mut editor: ResMut<Editor>,
    mut cameras: Query<&mut Camera, With<Camera3d>>,
    windows: Query<&Window>,
    mut exit: MessageWriter<AppExit>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let e = &mut *editor;
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
                        if ui.button("New…").clicked() {
                            dialog(e, Dialog::New);
                            ui.close();
                        }
                        if ui.button("Open…").clicked() {
                            dialog(e, Dialog::Open);
                            ui.close();
                        }
                        if ui.button("Save    Ctrl+S").clicked() {
                            save(e, e.path.clone());
                            ui.close();
                        }
                        if ui.button("Save As…").clicked() {
                            dialog(e, Dialog::SaveAs);
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
                    ui.label(format!(
                        "{}{}",
                        e.path.display(),
                        if e.dirty { " *" } else { "" }
                    ));
                    ui.checkbox(&mut e.rotate, "Rotate preview");
                });
            });
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::S)) {
                save(e, e.path.clone());
            }
            egui::Panel::bottom("status").show_inside(root, |ui| {
                ui.label(&e.status);
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
                e.dirty = true;
                e.rebuild = true;
            }
            root.available_rect_before_wrap()
        })
        .inner;
    if let Some(kind) = e.dialog {
        egui::Window::new(match kind {
            Dialog::New => "New BMAT",
            Dialog::Exit => "Quit editor",
            Dialog::Open => "Open BMAT",
            Dialog::SaveAs => "Save BMAT As",
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
            let needs_overwrite = matches!(kind, Dialog::SaveAs) && path.exists();
            if needs_overwrite {
                ui.checkbox(&mut e.overwrite, "Replace the existing file");
            }
            let needs_discard =
                matches!(kind, Dialog::Open | Dialog::New | Dialog::Exit) && e.dirty;
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
                    match kind {
                        Dialog::Exit => {
                            exit.write(AppExit::Success);
                        }
                        Dialog::New => {
                            if path.exists() {
                                e.status = "Use Open for an existing file".into();
                            } else {
                                e.doc = Document::default();
                                e.path = path;
                                e.dirty = true;
                                e.rebuild = true;
                                e.selected = None;
                                e.thumbnails.clear();
                                e.texture_info.clear();
                                e.dialog = None;
                                e.status = "New material".into();
                            }
                        }
                        Dialog::SaveAs => save(e, path),
                        Dialog::Open => match Document::open(&path) {
                            Ok(doc) => {
                                e.doc = doc;
                                e.path = path;
                                e.dirty = false;
                                e.rebuild = true;
                                e.selected = None;
                                e.thumbnails.clear();
                                e.texture_info.clear();
                                e.dialog = None;
                                e.status = "Opened material".into();
                            }
                            Err(err) => e.status = err,
                        },
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
            for mut camera in &mut cameras {
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
