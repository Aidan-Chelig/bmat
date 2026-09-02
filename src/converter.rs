//! Convert layered OpenRaster material files into Bevy-loadable KTX2 textures.
//!
//! A source such as `crate.ora` produces a `crate.bmat` bundle containing
//! `albedo.ktx2`, `normal.ktx2`, `orm.ktx2`, `emissive.ktx2`, and `data.ktx2`
//! entries when
//! those semantic layers are present. The material texture follows Bevy's PBR
//! convention: AO in red, roughness in green, and metallic in blue.

use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{self, Read},
    path::{Path, PathBuf},
};

use exr::prelude::{FlatImage, read_all_flat_layers_from_file};
use quick_xml::{Reader as XmlReader, XmlVersion, events::Event};
use zip::ZipArchive;

const SEMANTICS: [&str; 7] = [
    "albedo",
    "normal",
    "roughness",
    "metallic",
    "ao",
    "emissive",
    "data",
];
const EPSILON: f32 = 1.0e-5;

/// Mirrors `bmat.rs`'s `BmatAlphaMode` — the two are independently defined
/// (this binary and the game don't share a library crate) but must agree on
/// the RON variant names written here and parsed there.
#[derive(Debug, Clone, Copy, Default, serde::Deserialize)]
enum TextureAlphaMode {
    Opaque,
    #[default]
    Mask,
    Blend,
}

/// Optional per-texture settings read from a `<name>.ron` file sitting next
/// to the `.ora`/`.exr` source, e.g. `assets/materials-src/glass.ron`
/// alongside `assets/materials-src/glass.ora`:
///
/// ```ron
/// (alpha_mode: Blend)
/// ```
///
/// Absent by default, which keeps every existing material's hardcoded
/// `Mask` cutout behavior unchanged.
#[derive(Debug, Default, serde::Deserialize)]
struct TextureSettings {
    #[serde(default)]
    alpha_mode: TextureAlphaMode,
}

fn read_texture_settings(input: &Path) -> Result<TextureSettings, Box<dyn std::error::Error>> {
    let settings_path = input.with_extension("ron");
    if !settings_path.is_file() {
        return Ok(TextureSettings::default());
    }
    let contents = fs::read_to_string(&settings_path)
        .map_err(|error| format!("could not read {}: {error}", settings_path.display()))?;
    let settings: TextureSettings = ron::de::from_str(&contents)
        .map_err(|error| format!("invalid settings in {}: {error}", settings_path.display()))?;
    Ok(settings)
}

pub fn main() {
    let args = match Args::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };

    if let Err(error) = run(&args) {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

#[derive(Debug)]
struct Args {
    input: String,
    output: PathBuf,
    overwrite: bool,
}

impl Args {
    fn parse(mut args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut positional = Vec::new();
        let mut overwrite = false;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-f" | "--overwrite" => overwrite = true,
                "-h" | "--help" => return Err(Self::usage()),
                value if value.starts_with('-') => {
                    return Err(format!("unknown option {value}\n\n{}", Self::usage()));
                }
                value => positional.push(value.to_owned()),
            }
        }
        if positional.len() != 2 {
            return Err(Self::usage());
        }
        Ok(Self {
            input: positional.remove(0),
            output: positional.remove(0).into(),
            overwrite,
        })
    }

    fn usage() -> String {
        "Usage: ora_to_ktx2 <input-ora-or-glob> <output-directory> [--overwrite|-f]".to_owned()
    }
}

fn run(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let inputs = expand_inputs(&args.input)?;
    if inputs.is_empty() {
        return Err(format!("input did not match any EXR/ORA files: {}", args.input).into());
    }
    fs::create_dir_all(&args.output)?;

    let mut converted = 0;
    let mut failures = Vec::new();
    for input in inputs {
        match convert_file(&input, &args.output, args.overwrite) {
            Ok(()) => converted += 1,
            Err(error) => {
                eprintln!("warning: {error}");
                failures.push(error.to_string());
            }
        }
    }
    if converted == 0 {
        return Err(failures
            .into_iter()
            .next()
            .unwrap_or_else(|| "no inputs could be converted".to_owned())
            .into());
    }
    Ok(())
}

fn expand_inputs(input: &str) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    if !input.contains('*') && !input.contains('?') && !input.contains('[') {
        let path = PathBuf::from(input);
        if path.is_file() {
            return Ok(vec![path]);
        }
        if path.is_dir() {
            let mut paths = fs::read_dir(path)?
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.is_file() && is_supported_input(path))
                .collect::<Vec<_>>();
            paths.sort();
            return Ok(paths);
        }
        return Ok(Vec::new());
    }

    let mut paths = glob::glob(input)
        .map_err(|error| format!("invalid input glob {input:?}: {error}"))?
        .collect::<Result<Vec<_>, _>>()?;
    paths.retain(|path| path.is_file() && is_supported_input(path));
    paths.sort();
    Ok(paths)
}

fn is_supported_input(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("exr") || extension.eq_ignore_ascii_case("ora")
    })
}

#[derive(Clone, Copy)]
struct PixelChannels<'a> {
    width: usize,
    height: usize,
    red: Option<&'a [f32]>,
    green: Option<&'a [f32]>,
    blue: Option<&'a [f32]>,
    alpha: Option<&'a [f32]>,
    scalar: Option<&'a [f32]>,
}

#[derive(Clone)]
struct SemanticImage {
    width: usize,
    height: usize,
    channels: usize,
    pixels: Vec<f32>,
}

pub fn convert_file(
    input: &Path,
    output_dir: &Path,
    overwrite: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if input
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("ora"))
    {
        return convert_ora_file(input, output_dir, overwrite);
    }
    let image = read_all_flat_layers_from_file(input)
        .map_err(|error| format!("could not read {}: {error}", input.display()))?;
    let base = input
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| format!("input has no valid file stem: {}", input.display()))?;

    let mut outputs = Vec::new();
    if let Some(albedo) = semantic_image(&image, "albedo")? {
        outputs.push(("albedo", albedo, true));
    }
    if let Some(normal) = semantic_image(&image, "normal")? {
        outputs.push(("normal", normal, false));
    }
    if let Some(emissive) = semantic_image(&image, "emissive")? {
        outputs.push(("emissive", emissive, true));
    }
    if let Some(data) = semantic_image(&image, "data")? {
        outputs.push(("data", scalarize_image(&data, "data"), false));
    }

    let roughness = semantic_image(&image, "roughness")?;
    let metallic = semantic_image(&image, "metallic")?;
    let ao = semantic_image(&image, "ao")?;
    if roughness.is_some() || metallic.is_some() || ao.is_some() {
        let (width, height) = roughness
            .as_ref()
            .map(|image| (image.width, image.height))
            .or_else(|| metallic.as_ref().map(|image| (image.width, image.height)))
            .or_else(|| ao.as_ref().map(|image| (image.width, image.height)))
            .expect("material semantic exists");
        // Bevy/glTF ORM semantics: absent occlusion and roughness should not
        // darken or polish the material, while absent metallic stays dielectric.
        let mut packed = default_orm_pixels(width * height);
        for (channel, semantic) in [ao, roughness, metallic].into_iter().enumerate() {
            if let Some(semantic) = semantic {
                if semantic.width != width || semantic.height != height {
                    return Err(format!(
                        "material layers in {} have different dimensions",
                        input.display()
                    )
                    .into());
                }
                for (pixel, value) in semantic.pixels.into_iter().enumerate() {
                    packed[pixel * 3 + channel] = value;
                }
            }
        }
        outputs.push((
            "orm",
            SemanticImage {
                width,
                height,
                channels: 3,
                pixels: packed,
            },
            false,
        ));
    }

    if outputs.is_empty() {
        eprintln!("available EXR layers/channels in {}:", input.display());
        for layer in &image.layer_data {
            let layer_name = layer
                .attributes
                .layer_name
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| "<unnamed>".to_owned());
            let channels = layer
                .channel_data
                .list
                .iter()
                .map(|channel| channel.name.to_string())
                .collect::<Vec<_>>();
            eprintln!("  {layer_name}: {}", channels.join(", "));
        }
    }
    write_outputs(input, output_dir, base, overwrite, outputs)
}

/// Writes an uncompressed 8-bit image as the albedo texture of a BMAT bundle.
///
/// `channels` must be 1 (luminance), 2 (luminance/alpha), 3 (RGB), or 4
/// (RGBA). Luminance inputs are expanded to RGB because BMAT base-color
/// textures use color KTX2 formats.
pub fn write_albedo_bmat(
    output: &Path,
    width: usize,
    height: usize,
    pixels: &[u8],
    channels: usize,
    overwrite: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if !(1..=4).contains(&channels) || pixels.len() != width * height * channels {
        return Err(format!(
            "invalid {}x{} image with {channels} channels and {} bytes",
            width,
            height,
            pixels.len()
        )
        .into());
    }
    let output_dir = output.parent().unwrap_or_else(|| Path::new("."));
    let base = output
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| format!("output has no valid file stem: {}", output.display()))?;
    let output_channels = if channels == 2 || channels == 4 { 4 } else { 3 };
    let mut values = Vec::with_capacity(width * height * output_channels);
    for pixel in pixels.chunks_exact(channels) {
        let channel = |index: usize| f32::from(pixel[index]) / 255.0;
        match channels {
            1 => values.extend_from_slice(&[channel(0), channel(0), channel(0)]),
            2 => values.extend_from_slice(&[channel(0), channel(0), channel(0), channel(1)]),
            3 => values.extend_from_slice(&[channel(0), channel(1), channel(2)]),
            4 => values.extend_from_slice(&[channel(0), channel(1), channel(2), channel(3)]),
            _ => unreachable!(),
        }
    }
    write_outputs(
        output,
        output_dir,
        base,
        overwrite,
        vec![(
            "albedo",
            SemanticImage {
                width,
                height,
                channels: output_channels,
                pixels: values,
            },
            true,
        )],
    )
}

fn default_orm_pixels(pixel_count: usize) -> Vec<f32> {
    vec![1.0, 1.0, 0.0].repeat(pixel_count)
}

#[allow(dead_code)]
pub fn write_ora_albedo_preview(
    input: &Path,
    output: &Path,
    overwrite: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if output.exists() && !overwrite {
        return Ok(());
    }

    let file = fs::File::open(input)?;
    let mut archive = ZipArchive::new(file)?;
    let mut stack_xml = String::new();
    archive
        .by_name("stack.xml")?
        .read_to_string(&mut stack_xml)?;
    let layers = parse_ora_stack(&stack_xml)?;
    let (_, source) = layers
        .iter()
        .find(|(name, _)| find_ora_layer_name(name, "albedo"))
        .or_else(|| {
            layers
                .iter()
                .find(|(name, _)| find_ora_layer_name(name, "data"))
        })
        .ok_or_else(|| format!("{} contains no albedo or data layer", input.display()))?;
    let mut png = Vec::new();
    archive.by_name(source)?.read_to_end(&mut png)?;
    let image = image::load_from_memory(&png)?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    image.save_with_format(output, image::ImageFormat::Png)?;
    Ok(())
}

fn write_outputs(
    input: &Path,
    output_dir: &Path,
    base: &str,
    overwrite: bool,
    outputs: Vec<(&str, SemanticImage, bool)>,
) -> Result<(), Box<dyn std::error::Error>> {
    if outputs.is_empty() {
        return Err(format!(
            "{} contains none of the supported layers: {}",
            input.display(),
            SEMANTICS.join(", ")
        )
        .into());
    }
    let bundle_path = output_dir.join(format!("{base}.bmat"));
    if bundle_path.exists() && !overwrite {
        return Err(format!(
            "refusing to overwrite {}; pass --overwrite",
            bundle_path.display()
        )
        .into());
    }

    let settings = read_texture_settings(input)?;

    let mut bundle = Vec::with_capacity(outputs.len());
    for (suffix, image, srgb) in outputs {
        let (pixels, channels) = pack_unorm8(&image.pixels, image.channels, srgb);
        let bytes = if suffix == "normal" {
            encode_normal_ktx2_with_mips(image.width, image.height, &pixels, channels)?
        } else {
            encode_ktx2(image.width, image.height, &pixels, channels, srgb)?
        };
        bundle.push((format!("{suffix}.ktx2"), bytes));
    }
    bundle.push((
        "manifest.ron".to_owned(),
        manifest_ron(&bundle, settings.alpha_mode).into_bytes(),
    ));
    write_tar_bundle(&bundle_path, &bundle)?;
    println!("{} -> {}", input.display(), bundle_path.display());
    Ok(())
}

fn manifest_ron(entries: &[(String, Vec<u8>)], alpha_mode: TextureAlphaMode) -> String {
    let optional_layer = |name: &str| {
        entries
            .iter()
            .any(|(entry, _)| entry == &format!("{name}.ktx2"))
            .then(|| format!("Some(\"{name}.ktx2\")"))
            .unwrap_or_else(|| "None".to_owned())
    };
    let base_color = entries
        .iter()
        .any(|(entry, _)| entry == "albedo.ktx2")
        .then(|| "Some(\"albedo.ktx2\")".to_owned())
        .unwrap_or_else(|| "None".to_owned());
    let alpha_mode = match alpha_mode {
        TextureAlphaMode::Opaque => "Opaque",
        TextureAlphaMode::Mask => "Mask",
        TextureAlphaMode::Blend => "Blend",
    };
    format!(
        "(\n    version: 1,\n    base_color_texture: {base_color},\n    normal_map_texture: {},\n    metallic_roughness_texture: {},\n    occlusion_texture: {},\n    emissive_texture: {},\n    data_texture: {},\n    alpha_mode: {alpha_mode},\n)\n",
        optional_layer("normal"),
        optional_layer("orm"),
        optional_layer("orm"),
        optional_layer("emissive"),
        optional_layer("data"),
    )
}

fn write_tar_bundle(path: &Path, entries: &[(String, Vec<u8>)]) -> io::Result<()> {
    let mut archive = Vec::new();
    for (name, data) in entries {
        let mut header = [0u8; 512];
        write_tar_field(&mut header[0..100], name.as_bytes());
        write_tar_field(&mut header[100..108], b"0000644\0");
        write_tar_field(&mut header[108..116], b"0000000\0");
        write_tar_field(&mut header[116..124], b"0000000\0");
        let size = format!("{:011o}\0", data.len());
        write_tar_field(&mut header[124..136], size.as_bytes());
        write_tar_field(&mut header[136..148], b"00000000000\0");
        header[148..156].fill(b' ');
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let checksum: u32 = header.iter().map(|byte| u32::from(*byte)).sum();
        let checksum = format!("{:06o}\0 ", checksum);
        write_tar_field(&mut header[148..156], checksum.as_bytes());
        archive.extend_from_slice(&header);
        archive.extend_from_slice(data);
        let padding = (512 - data.len() % 512) % 512;
        archive.resize(archive.len() + padding, 0);
    }
    archive.resize(archive.len() + 1024, 0);
    let temporary = path.with_extension("bmat.tmp");
    fs::write(&temporary, archive)?;
    fs::rename(temporary, path)?;

    // Bevy ignores individual asset rename events. Opening the completed
    // destination for writing produces a CloseWrite notification, ensuring
    // it reloads only after the whole archive has been published.
    OpenOptions::new().write(true).open(path)?;
    Ok(())
}

fn write_tar_field(field: &mut [u8], value: &[u8]) {
    let length = value.len().min(field.len());
    field[..length].copy_from_slice(&value[..length]);
}

struct OraLayer {
    name: String,
    image: SemanticImage,
}

fn convert_ora_file(
    input: &Path,
    output_dir: &Path,
    overwrite: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let file = fs::File::open(input)?;
    let mut archive = ZipArchive::new(file)?;
    let mut stack_xml = String::new();
    archive
        .by_name("stack.xml")?
        .read_to_string(&mut stack_xml)?;
    let layer_sources = parse_ora_stack(&stack_xml)?;
    let mut layers = Vec::new();
    for (name, source) in layer_sources {
        let mut png = Vec::new();
        archive.by_name(&source)?.read_to_end(&mut png)?;
        let image = image::load_from_memory(&png)?.into_rgba32f();
        let (width, height) = image.dimensions();
        let has_alpha = image.pixels().any(|pixel| (pixel[3] - 1.0).abs() > EPSILON);
        let channels = if has_alpha { 4 } else { 3 };
        let pixels = image
            .pixels()
            .flat_map(|pixel| pixel.0.into_iter().take(channels))
            .collect();
        layers.push(OraLayer {
            name,
            image: SemanticImage {
                width: width as usize,
                height: height as usize,
                channels,
                pixels,
            },
        });
    }

    let mut outputs = Vec::new();
    for (semantic, srgb) in [("albedo", true), ("normal", false), ("emissive", true)] {
        if let Some(layer) = find_ora_layer(&layers, semantic) {
            outputs.push((semantic, layer.image.clone(), srgb));
        }
    }
    if let Some(layer) = find_ora_layer(&layers, "data") {
        outputs.push(("data", scalarize_ora_layer(&layer.image, "data"), false));
    }
    let scalar_layers = ["roughness", "metallic", "ao"].map(|semantic| {
        find_ora_layer(&layers, semantic).map(|layer| scalarize_ora_layer(&layer.image, semantic))
    });
    if scalar_layers.iter().any(Option::is_some) {
        let (width, height) = scalar_layers
            .iter()
            .find_map(|layer| layer.as_ref().map(|layer| (layer.width, layer.height)))
            .expect("at least one scalar layer exists");
        // Bevy/glTF ORM semantics: absent occlusion and roughness should not
        // darken or polish the material, while absent metallic stays dielectric.
        let mut packed = default_orm_pixels(width * height);
        for (channel, layer) in [
            scalar_layers[2].clone(),
            scalar_layers[0].clone(),
            scalar_layers[1].clone(),
        ]
        .into_iter()
        .enumerate()
        {
            if let Some(layer) = layer {
                if layer.width != width || layer.height != height {
                    return Err(format!(
                        "material layers in {} have different dimensions",
                        input.display()
                    )
                    .into());
                }
                for (pixel, value) in layer.pixels.iter().copied().enumerate() {
                    packed[pixel * 3 + channel] = value;
                }
            }
        }
        outputs.push((
            "orm",
            SemanticImage {
                width,
                height,
                channels: 3,
                pixels: packed,
            },
            false,
        ));
    }

    let base = input
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| format!("input has no valid file stem: {}", input.display()))?;
    write_outputs(input, output_dir, base, overwrite, outputs)
}

fn scalarize_ora_layer(image: &SemanticImage, semantic: &str) -> SemanticImage {
    if image.pixels.chunks_exact(image.channels).any(|values| {
        values
            .iter()
            .take(3)
            .skip(1)
            .any(|value| (value - values[0]).abs() > EPSILON)
    }) {
        eprintln!("warning: RGB channels differ in scalar layer {semantic}; using the red channel");
    }
    SemanticImage {
        width: image.width,
        height: image.height,
        channels: 1,
        pixels: image
            .pixels
            .chunks_exact(image.channels)
            .map(|values| values[0])
            .collect(),
    }
}

fn scalarize_image(image: &SemanticImage, semantic: &str) -> SemanticImage {
    if image.channels == 1 {
        return image.clone();
    }
    scalarize_ora_layer(image, semantic)
}

fn parse_ora_stack(xml: &str) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    let mut reader = XmlReader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut layers = Vec::new();
    loop {
        match reader.read_event()? {
            Event::Empty(element) | Event::Start(element)
                if element.name().as_ref() == b"layer" =>
            {
                let mut name = None;
                let mut source = None;
                for attribute in element.attributes().flatten() {
                    match attribute.key.as_ref() {
                        b"name" => {
                            name = Some(
                                attribute
                                    .decoded_and_normalized_value(
                                        XmlVersion::Implicit1_0,
                                        reader.decoder(),
                                    )?
                                    .into_owned(),
                            )
                        }
                        b"src" => {
                            source = Some(
                                attribute
                                    .decoded_and_normalized_value(
                                        XmlVersion::Implicit1_0,
                                        reader.decoder(),
                                    )?
                                    .into_owned(),
                            )
                        }
                        _ => {}
                    }
                }
                if let (Some(name), Some(source)) = (name, source) {
                    layers.push((name, source));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(layers)
}

fn find_ora_layer<'a>(layers: &'a [OraLayer], semantic: &str) -> Option<&'a OraLayer> {
    layers
        .iter()
        .find(|layer| find_ora_layer_name(&layer.name, semantic))
}

fn find_ora_layer_name(name: &str, semantic: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    name == semantic
        || name
            .strip_prefix(semantic)
            .is_some_and(|suffix| suffix.starts_with(['.', '/', '_', ' ']))
}

fn semantic_image(image: &FlatImage, semantic: &str) -> Result<Option<SemanticImage>, io::Error> {
    let mut matches = Vec::new();
    for layer in &image.layer_data {
        let layer_name = layer
            .attributes
            .layer_name
            .as_ref()
            .map(|name| name.to_string().to_ascii_lowercase())
            .unwrap_or_default();
        let mut channels = BTreeMap::new();
        for channel in &layer.channel_data.list {
            let name = channel.name.to_string().to_ascii_lowercase();
            let component = if layer_name == semantic {
                component_name(&name)
            } else if name == semantic {
                "y"
            } else if name
                .strip_prefix(semantic)
                .and_then(|rest| rest.strip_prefix(['.', '/', '_']))
                .is_some()
            {
                component_name(&name)
            } else {
                continue;
            };
            channels.insert(
                component.to_owned(),
                channel.sample_data.values_as_f32().collect(),
            );
        }
        if !channels.is_empty() {
            matches.push((layer.size.0, layer.size.1, channels));
        }
    }

    let Some((width, height, channels)) = matches.into_iter().next() else {
        return Ok(None);
    };
    if matches_count(semantic, image) > 1 {
        eprintln!("warning: multiple EXR layers match {semantic}; using the first");
    }
    let red = channels.get("r").or_else(|| channels.get("x"));
    let green = channels.get("g").or_else(|| channels.get("y"));
    let blue = channels.get("b").or_else(|| channels.get("z"));
    let alpha = channels.get("a");
    let scalar = channels.get("y").or_else(|| channels.get("l")).or(red);
    let channel_data = PixelChannels {
        width,
        height,
        red: red.map(Vec::as_slice),
        green: green.map(Vec::as_slice),
        blue: blue.map(Vec::as_slice),
        alpha: alpha.map(Vec::as_slice),
        scalar: scalar.map(Vec::as_slice),
    };
    let scalar_semantic = matches!(semantic, "roughness" | "metallic" | "ao" | "data");
    if scalar_semantic {
        scalar_image(channel_data, semantic).map(Some)
    } else {
        color_image(channel_data, semantic).map(Some)
    }
}

fn matches_count(semantic: &str, image: &FlatImage) -> usize {
    image
        .layer_data
        .iter()
        .filter(|layer| {
            layer
                .attributes
                .layer_name
                .as_ref()
                .is_some_and(|name| name.to_string().eq_ignore_ascii_case(semantic))
        })
        .count()
}

fn component_name(name: &str) -> &str {
    name.rsplit_once(['.', '/', '_'])
        .map_or(name, |(_, component)| component)
}

fn scalar_image(channels: PixelChannels<'_>, semantic: &str) -> Result<SemanticImage, io::Error> {
    let values = channels.scalar.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("scalar layer {semantic} has no usable channel"),
        )
    })?;
    for (name, channel) in [
        ("R", channels.red),
        ("G", channels.green),
        ("B", channels.blue),
    ] {
        if let Some(channel) = channel {
            if channel.len() == values.len()
                && channel
                    .iter()
                    .zip(values)
                    .any(|(left, right)| (left - right).abs() > EPSILON)
            {
                eprintln!("warning: RGB channels differ in scalar layer {semantic}; using {name}");
                break;
            }
        }
    }
    Ok(SemanticImage {
        width: channels.width,
        height: channels.height,
        channels: 1,
        pixels: values.to_vec(),
    })
}

fn color_image(channels: PixelChannels<'_>, semantic: &str) -> Result<SemanticImage, io::Error> {
    let count = channels.width * channels.height;
    let red = channels.red.or(channels.scalar).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("color layer {semantic} has no red channel"),
        )
    })?;
    let green = channels.green.unwrap_or(red);
    let blue = channels.blue.unwrap_or(red);
    if red.len() != count
        || green.len() != count
        || blue.len() != count
        || channels.alpha.is_some_and(|alpha| alpha.len() != count)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("layer {semantic} channel dimensions do not match"),
        ));
    }
    let alpha = channels
        .alpha
        .filter(|alpha| alpha.iter().any(|value| (value - 1.0).abs() > EPSILON));
    let channel_count = if alpha.is_some() { 4 } else { 3 };
    let mut pixels = Vec::with_capacity(count * channel_count);
    for pixel in 0..count {
        pixels.extend([red[pixel], green[pixel], blue[pixel]]);
        if let Some(alpha) = alpha {
            pixels.push(alpha[pixel]);
        }
    }
    Ok(SemanticImage {
        width: channels.width,
        height: channels.height,
        channels: channel_count,
        pixels,
    })
}

fn pack_unorm8(values: &[f32], channels: usize, srgb: bool) -> (Vec<u8>, usize) {
    assert!(matches!(channels, 1 | 3 | 4));
    let mut output = Vec::with_capacity(values.len());
    for pixel in values.chunks_exact(channels) {
        for (channel, &value) in pixel.iter().enumerate() {
            let value = (srgb && channel < 3)
                .then(|| linear_to_srgb(value))
                .unwrap_or(value);
            output.push((value.clamp(0.0, 1.0) * 255.0).round() as u8);
        }
    }
    (output, channels)
}

fn linear_to_srgb(value: f32) -> f32 {
    if value <= 0.0031308 {
        value * 12.92
    } else {
        1.055 * value.max(0.0).powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
fn write_ktx2(
    path: &Path,
    width: usize,
    height: usize,
    pixels: &[u8],
    channels: usize,
    srgb: bool,
) -> io::Result<()> {
    fs::write(path, encode_ktx2(width, height, pixels, channels, srgb)?)
}

fn encode_ktx2(
    width: usize,
    height: usize,
    pixels: &[u8],
    channels: usize,
    srgb: bool,
) -> io::Result<Vec<u8>> {
    encode_ktx2_levels(width, height, &[pixels.to_vec()], channels, srgb)
}

fn encode_normal_ktx2_with_mips(
    width: usize,
    height: usize,
    pixels: &[u8],
    channels: usize,
) -> io::Result<Vec<u8>> {
    if !matches!(channels, 3 | 4) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "normal maps must have three or four channels",
        ));
    }
    let mut levels = vec![pixels.to_vec()];
    let (mut level_width, mut level_height) = (width, height);
    while level_width > 1 || level_height > 1 {
        let next_width = (level_width / 2).max(1);
        let next_height = (level_height / 2).max(1);
        let previous = levels.last().expect("base normal level exists");
        let mut next = Vec::with_capacity(next_width * next_height * channels);
        for y in 0..next_height {
            for x in 0..next_width {
                let mut normal = [0.0f32; 3];
                let mut alpha = 0.0;
                let mut samples = 0.0;
                for source_y in y * 2..(y * 2 + 2).min(level_height) {
                    for source_x in x * 2..(x * 2 + 2).min(level_width) {
                        let offset = (source_y * level_width + source_x) * channels;
                        for channel in 0..3 {
                            normal[channel] += previous[offset + channel] as f32 / 127.5 - 1.0;
                        }
                        if channels == 4 {
                            alpha += previous[offset + 3] as f32 / 255.0;
                        }
                        samples += 1.0;
                    }
                }
                let length = normal.iter().map(|value| value * value).sum::<f32>().sqrt();
                let normal = if length > f32::EPSILON {
                    normal.map(|value| value / length)
                } else {
                    [0.0, 0.0, 1.0]
                };
                next.extend(normal.map(|value| ((value * 0.5 + 0.5) * 255.0).round() as u8));
                if channels == 4 {
                    next.push(((alpha / samples) * 255.0).round() as u8);
                }
            }
        }
        levels.push(next);
        level_width = next_width;
        level_height = next_height;
    }
    encode_ktx2_levels(width, height, &levels, channels, false)
}

fn encode_ktx2_levels(
    width: usize,
    height: usize,
    levels: &[Vec<u8>],
    channels: usize,
    srgb: bool,
) -> io::Result<Vec<u8>> {
    let format = match (channels, srgb) {
        (1, false) => ktx2::Format::R8_UNORM,
        (3, false) => ktx2::Format::R8G8B8_UNORM,
        (3, true) => ktx2::Format::R8G8B8_SRGB,
        (4, false) => ktx2::Format::R8G8B8A8_UNORM,
        (4, true) => ktx2::Format::R8G8B8A8_SRGB,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unsupported KTX2 channel count {channels}"),
            ));
        }
    };
    let (basic_dfd, type_size) = ktx2::dfd::Basic::from_format(format)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let dfd_block = ktx2::dfd::Block::Basic(basic_dfd).to_vec();
    let dfd_length = 4 + dfd_block.len();
    let level_index_offset = ktx2::Header::LENGTH;
    let dfd_offset = level_index_offset + ktx2::LevelIndex::LENGTH * levels.len();
    let alignment = least_common_multiple(channels, 4);
    let mut next_data_offset = align_up(dfd_offset + dfd_length, alignment);
    let level_indices = levels
        .iter()
        .map(|level| {
            let index = ktx2::LevelIndex {
                byte_offset: next_data_offset as u64,
                byte_length: level.len() as u64,
                uncompressed_byte_length: level.len() as u64,
            };
            next_data_offset = align_up(next_data_offset + level.len(), alignment);
            index
        })
        .collect::<Vec<_>>();
    let mut bytes = Vec::with_capacity(next_data_offset);
    let header = ktx2::Header {
        format: Some(format),
        type_size,
        pixel_width: width as u32,
        pixel_height: height as u32,
        pixel_depth: 0,
        layer_count: 0,
        face_count: 1,
        level_count: levels.len() as u32,
        supercompression_scheme: None,
        index: ktx2::Index {
            dfd_byte_offset: dfd_offset as u32,
            dfd_byte_length: dfd_length as u32,
            kvd_byte_offset: 0,
            kvd_byte_length: 0,
            sgd_byte_offset: 0,
            sgd_byte_length: 0,
        },
    };
    bytes.extend_from_slice(&header.as_bytes());
    for level in &level_indices {
        bytes.extend_from_slice(&level.as_bytes());
    }
    bytes.extend_from_slice(&(dfd_length as u32).to_le_bytes());
    bytes.extend_from_slice(&dfd_block);
    for (index, level) in level_indices.iter().zip(levels) {
        bytes.resize(index.byte_offset as usize, 0);
        bytes.extend_from_slice(level);
    }
    Ok(bytes)
}

fn align_up(value: usize, alignment: usize) -> usize {
    value.div_ceil(alignment) * alignment
}

fn least_common_multiple(left: usize, right: usize) -> usize {
    left / greatest_common_divisor(left, right) * right
}

fn greatest_common_divisor(mut left: usize, mut right: usize) -> usize {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_raw_albedo_as_bmat() {
        let dir = std::env::temp_dir().join(format!("bmat_raw_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let output = dir.join("preview.bmat");
        write_albedo_bmat(&output, 1, 1, &[128, 64, 32, 255], 4, true).unwrap();
        let entries = crate::read_tar_entries(&fs::read(&output).unwrap()).unwrap();
        assert!(entries.contains_key("albedo.ktx2"));
        assert!(entries.contains_key("manifest.ron"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_orm_channels_use_neutral_defaults() {
        assert_eq!(default_orm_pixels(2), vec![1.0, 1.0, 0.0, 1.0, 1.0, 0.0]);
    }

    #[test]
    fn writes_a_ktx2_that_the_reader_accepts() {
        let path = std::env::temp_dir().join(format!("ora_to_ktx2_{}.ktx2", std::process::id()));
        write_ktx2(&path, 2, 1, &[0, 1, 2, 255, 3, 4, 5, 255], 4, false).unwrap();
        let bytes = fs::read(&path).unwrap();
        let reader = ktx2::Reader::new(bytes).unwrap();
        assert_eq!(reader.header().pixel_width, 2);
        assert_eq!(reader.header().pixel_height, 1);
        assert_eq!(reader.levels().next().unwrap().data.len(), 8);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn material_bundle_does_not_leave_loose_ktx2_files() {
        let output =
            std::env::temp_dir().join(format!("ora_to_ktx2_bundle_{}", std::process::id()));
        fs::create_dir_all(&output).unwrap();
        write_outputs(
            Path::new("material.ora"),
            &output,
            "material",
            true,
            vec![(
                "albedo",
                SemanticImage {
                    width: 1,
                    height: 1,
                    channels: 3,
                    pixels: vec![0.25, 0.5, 0.75],
                },
                true,
            )],
        )
        .unwrap();

        assert!(output.join("material.bmat").is_file());
        assert!(fs::read_dir(&output).unwrap().all(|entry| {
            entry
                .unwrap()
                .path()
                .extension()
                .is_none_or(|ext| ext != "ktx2")
        }));
        fs::remove_dir_all(output).unwrap();
    }

    #[test]
    fn scalar_values_remain_single_channel() {
        assert_eq!(pack_unorm8(&[0.5], 1, false), (vec![128], 1));
    }

    #[test]
    fn rgba_alpha_is_preserved_without_srgb_conversion() {
        let (packed, channels) = pack_unorm8(&[0.25, 0.5, 0.75, 0.2], 4, true);
        assert_eq!(channels, 4);
        assert_eq!(packed[3], 51);
        assert_ne!(packed[0], 64);
    }

    #[test]
    fn opaque_alpha_channel_is_omitted() {
        let red = [0.25, 0.5];
        let green = [0.5, 0.75];
        let blue = [0.75, 1.0];
        let alpha = [1.0, 1.0];
        let image = color_image(
            PixelChannels {
                width: 2,
                height: 1,
                red: Some(&red),
                green: Some(&green),
                blue: Some(&blue),
                alpha: Some(&alpha),
                scalar: None,
            },
            "albedo",
        )
        .unwrap();
        assert_eq!(image.channels, 3);
        assert_eq!(image.pixels.len(), 6);
    }

    #[test]
    fn writes_rgb_ktx2_without_alpha() {
        let path =
            std::env::temp_dir().join(format!("ora_to_ktx2_rgb_{}.ktx2", std::process::id()));
        write_ktx2(&path, 2, 1, &[0, 1, 2, 3, 4, 5], 3, false).unwrap();
        let bytes = fs::read(&path).unwrap();
        let image = bevy::image::Image::from_buffer(
            &bytes,
            bevy::image::ImageType::Extension("ktx2"),
            bevy::image::CompressedImageFormats::NONE,
            false,
            bevy::image::ImageSampler::Default,
            bevy::asset::RenderAssetUsages::default(),
        )
        .unwrap();
        assert_eq!(
            image.texture_descriptor.format,
            bevy::render::render_resource::TextureFormat::Rgba8Unorm
        );
        let reader = ktx2::Reader::new(bytes).unwrap();
        assert_eq!(reader.header().format, Some(ktx2::Format::R8G8B8_UNORM));
        assert_eq!(reader.levels().next().unwrap().data.len(), 6);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn manifest_uses_bevy_standard_material_field_names() {
        let entries = vec![
            ("albedo.ktx2".to_owned(), Vec::new()),
            ("normal.ktx2".to_owned(), Vec::new()),
            ("orm.ktx2".to_owned(), Vec::new()),
            ("emissive.ktx2".to_owned(), Vec::new()),
            ("data.ktx2".to_owned(), Vec::new()),
        ];
        assert_eq!(
            manifest_ron(&entries, TextureAlphaMode::Mask),
            "(\n    version: 1,\n    base_color_texture: Some(\"albedo.ktx2\"),\n    normal_map_texture: Some(\"normal.ktx2\"),\n    metallic_roughness_texture: Some(\"orm.ktx2\"),\n    occlusion_texture: Some(\"orm.ktx2\"),\n    emissive_texture: Some(\"emissive.ktx2\"),\n    data_texture: Some(\"data.ktx2\"),\n    alpha_mode: Mask,\n)\n"
        );
    }

    #[test]
    fn manifest_embeds_requested_alpha_mode() {
        let entries = vec![("albedo.ktx2".to_owned(), Vec::new())];
        assert!(manifest_ron(&entries, TextureAlphaMode::Blend).contains("alpha_mode: Blend,"));
    }

    #[test]
    fn missing_settings_sidecar_defaults_to_mask() {
        let settings = read_texture_settings(Path::new("does_not_exist.ora")).unwrap();
        assert!(matches!(settings.alpha_mode, TextureAlphaMode::Mask));
    }

    #[test]
    fn settings_sidecar_overrides_alpha_mode() {
        let dir = std::env::temp_dir().join(format!("ora_to_ktx2_settings_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let source = dir.join("glass.ora");
        fs::write(&source, b"").unwrap();
        fs::write(dir.join("glass.ron"), b"(alpha_mode: Blend)").unwrap();

        let settings = read_texture_settings(&source).unwrap();
        assert!(matches!(settings.alpha_mode, TextureAlphaMode::Blend));

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn writes_single_channel_linear_ktx2() {
        let bytes = encode_ktx2(2, 1, &[0, 255], 1, false).unwrap();
        let image = bevy::image::Image::from_buffer(
            &bytes,
            bevy::image::ImageType::Extension("ktx2"),
            bevy::image::CompressedImageFormats::NONE,
            false,
            bevy::image::ImageSampler::Default,
            bevy::asset::RenderAssetUsages::default(),
        )
        .unwrap();
        assert_eq!(
            image.texture_descriptor.format,
            bevy::render::render_resource::TextureFormat::R8Unorm
        );
        let reader = ktx2::Reader::new(bytes).unwrap();
        assert_eq!(reader.header().format, Some(ktx2::Format::R8_UNORM));
        assert_eq!(reader.levels().next().unwrap().data.len(), 2);
    }

    #[test]
    fn normal_maps_include_renormalized_mip_levels() {
        let normals = [128, 128, 255].repeat(16);
        let bytes = encode_normal_ktx2_with_mips(4, 4, &normals, 3).unwrap();
        let reader = ktx2::Reader::new(bytes).unwrap();
        assert_eq!(reader.header().level_count, 3);
        let levels = reader.levels().collect::<Vec<_>>();
        assert_eq!(levels[0].data.len(), 4 * 4 * 3);
        assert_eq!(levels[1].data.len(), 2 * 2 * 3);
        assert_eq!(levels[2].data, &[128, 128, 255]);
    }
}
