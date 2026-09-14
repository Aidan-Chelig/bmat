//! Migrate OpenRaster material sources into editable BMAT project folders.
use std::{fs, path::{Path, PathBuf}};

fn main() {
    if let Err(error) = run(std::env::args().skip(1)) {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run(mut args: impl Iterator<Item = String>) -> Result<(), String> {
    let input = args.next().ok_or_else(usage)?;
    if input == "-h" || input == "--help" { return Err(usage()); }
    let output = PathBuf::from(args.next().ok_or_else(usage)?);
    let mut overwrite = false;
    for arg in args {
        match arg.as_str() {
            "-f" | "--overwrite" => overwrite = true,
            "-h" | "--help" => return Err(usage()),
            _ => return Err(format!("unknown option {arg}\n\n{}", usage())),
        }
    }
    let mut files = Vec::new();
    let input_path = Path::new(&input);
    if input_path.is_file() {
        if input_path.extension().is_some_and(|e| e.eq_ignore_ascii_case("ora")) { files.push(input_path.to_owned()); }
    } else if input_path.is_dir() {
        for entry in fs::read_dir(input_path).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.is_file() && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("ora")) { files.push(path); }
        }
    } else { return Err(format!("input does not exist: {}", input_path.display())); }
    files.sort();
    if files.is_empty() { return Err(format!("no ORA files found under {}", input_path.display())); }
    fs::create_dir_all(&output).map_err(|e| e.to_string())?;
    for ora in files {
        let stem = ora.file_stem().and_then(|s| s.to_str()).ok_or("invalid ORA filename")?;
        let project = output.join(stem);
        if project.exists() && !overwrite { return Err(format!("project exists (use --overwrite): {}", project.display())); }
        let temp = tempfile::tempdir().map_err(|e| e.to_string())?;
        bmat::converter::convert_file(&ora, temp.path(), true).map_err(|e| e.to_string())?;
        let baked = temp.path().join(format!("{stem}.bmat"));
        let doc = bmat::editor::Document::open(&baked)?;
        doc.save_project(&project, Path::new(&format!("build/{stem}.bmat")))?;
        println!("{} -> {}", ora.display(), project.display());
    }
    Ok(())
}

fn usage() -> String {
    "Usage: ora_to_bmat_project <ora-file-or-folder> <output-folder> [--overwrite|-f]".into()
}
