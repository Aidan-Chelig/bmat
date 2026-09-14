//! Small native BMAT authoring/inspection tool. It intentionally keeps the
//! first editor pass simple: a live rotating cube plus an inspector that is
//! ready to grow into channel editing without sidecar files.

use std::{env, fs, path::PathBuf};

use bevy::prelude::*;
use bevy_materialize::prelude::{GenericMaterial3d, MaterializePlugin};
use bmat::{BmatAssetPlugin, BmatInspection, inspect_bmat};

#[derive(Resource)]
struct EditorState {
    path: PathBuf,
    inspection: Option<BmatInspection>,
}

#[derive(Component)]
struct PreviewCube;

fn main() {
    let path = env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("material.bmat"));
    let inspection = fs::read(&path).ok().and_then(|bytes| inspect_bmat(&bytes).ok());
    App::new()
        .insert_resource(EditorState { path: path.clone(), inspection })
        .add_plugins(DefaultPlugins)
        .add_plugins(MaterializePlugin::new(bevy_materialize::prelude::TomlMaterialDeserializer))
        .add_plugins(BmatAssetPlugin)
        .add_systems(Startup, setup)
        .add_systems(Update, (rotate_preview, inspector_ui))
        .run();
}

fn setup(mut commands: Commands, assets: Res<AssetServer>, state: Res<EditorState>) {
    commands.spawn((
        PreviewCube,
        Mesh3d(assets.add(Cuboid::from_length(2.0).into())),
        GenericMaterial3d(assets.load(state.path.to_string_lossy().to_string())),
        Transform::from_xyz(0.0, 0.0, 0.0),
    ));
    commands.spawn((Camera3d::default(), Transform::from_xyz(3.5, 2.5, 5.0).looking_at(Vec3::ZERO, Vec3::Y)));
    commands.spawn((PointLight { intensity: 1800.0, shadow_maps_enabled: true, ..default() }, Transform::from_xyz(3.0, 4.0, 4.0)));
}

fn rotate_preview(time: Res<Time>, mut cube: Query<&mut Transform, With<PreviewCube>>) {
    for mut transform in &mut cube { transform.rotate_y(time.delta_secs() * 0.7); transform.rotate_x(time.delta_secs() * 0.25); }
}

fn inspector_ui(mut commands: Commands, state: Res<EditorState>, mut done: Local<bool>) {
    if *done { return; }
    *done = true;
    let title = format!("edbmat — {}", state.path.display());
    let details = state.inspection.as_ref().map(|i| format!("BMAT v{}\n\nTextures:\n{}\n\nAlpha: {:?}", i.manifest.version, i.entries.iter().filter(|e| e.ends_with(".ktx2")).map(|e| format!("  {e}")).collect::<Vec<_>>().join("\n"), i.manifest.alpha_mode)).unwrap_or_else(|| "New BMAT project\n\nSave will create this bundle directly.".to_owned());
    commands.spawn(Node { position_type: PositionType::Absolute, right: Val::Px(0.0), top: Val::Px(0.0), width: Val::Px(300.0), height: Val::Percent(100.0), padding: UiRect::all(Val::Px(18.0)), ..default() }).with_children(|parent| {
        parent.spawn((Text::new(format!("{title}\n\n{details}")), TextFont { font_size: FontSize::Px(16.0), ..default() }, TextColor(Color::WHITE)));
    });
}
