//! Compile an editable BMAT project directory into its configured runtime archive.
fn main() {
    if let Err(error) = run(std::env::args().skip(1)) {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run(mut args: impl Iterator<Item = String>) -> Result<(), String> {
    let input = args.next().ok_or_else(usage)?;
    if input == "--help" || input == "-h" {
        return Err(usage());
    }
    let mut output = None;
    let mut overwrite = false;
    for arg in args {
        match arg.as_str() {
            "-f" | "--overwrite" => overwrite = true,
            "-h" | "--help" => return Err(usage()),
            value if value.starts_with('-') => {
                return Err(format!("unknown option {value}\n\n{}", usage()));
            }
            value if output.is_none() => output = Some(value.into()),
            _ => return Err(usage()),
        }
    }
    let input = std::path::PathBuf::from(input);
    let (doc, manifest) = bmat::editor::Document::open_project(&input)?;
    let destination = output.map_or_else(|| input.join(manifest.export_path), |path| path);
    if destination.exists() && !overwrite {
        return Err(format!(
            "output exists (use --overwrite): {}",
            destination.display()
        ));
    }
    doc.save(&destination)
}

fn usage() -> String {
    "Usage: bmat_project_to_bmat <project-folder> [output-file] [--overwrite|-f]".into()
}
