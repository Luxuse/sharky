use std::{
    fs::{self, File},
    io::{self, BufReader, BufWriter, Read, Write},
    path::PathBuf,
    time::{Duration, Instant},
};

use clap::{CommandFactory, Parser};
use indicatif::{ProgressBar, ProgressStyle};
use walkdir::WalkDir;

use bzip2::read::BzDecoder;
use flate2::read::GzDecoder;
use tar::{Archive, Builder};
use xz2::read::XzDecoder;
use xz2::write::XzEncoder;
use zip::ZipArchive;
use zstd::stream::read::Decoder as ZstdDecoder;
use zstd::stream::write::Encoder as ZstdEncoder;
use unrar::Archive as UnrarArchive; // Import the UnrarArchive struct

/// Outil de compression/décompression : tar → XZ → Zstd
#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Tar + XZ (preset configurable) + Zstd (0–22) pour un ratio max"
)]
struct Args {
    #[arg(short = 'c', long = "compress", conflicts_with = "decompress")]
    compress: bool,

    #[arg(short = 'd', long = "decompress", conflicts_with = "compress")]
    decompress: bool,

    #[arg(short, long, value_name = "PATH")]
    input: PathBuf,

    #[arg(short, long, value_name = "PATH")]
    output: PathBuf,

    /// Niveau Zstd (0–22)
    #[arg(short = 'z', long = "zstd-level", default_value_t = 19)]
    zstd_level: i32,

    /// Niveau XZ preset (0–9)
    #[arg(short = 'x', long = "xz-preset", default_value_t = 9)]
    xz_preset: u32,

    /// Fichier dictionnaire Zstd (optionnel)
    #[arg(long = "dict", value_name = "FILE")]
    dict: Option<PathBuf>,

    /// Motifs d'exclusion
    #[arg(long = "exclude", value_name = "PATTERN")]
    exclude: Vec<String>,

    /// Taille du tampon en octets
    #[arg(long = "buffer-size", default_value_t = 4 * 1024 * 1024)]
    buffer_size: usize,
}

fn main() -> io::Result<()> {
    let args = Args::parse();
    if args.compress && !(0..=22).contains(&args.zstd_level) {
        eprintln!("Zstd level must be between 0 and 22");
        std::process::exit(1);
    }
    if args.compress && !(0..=9).contains(&args.xz_preset) {
        eprintln!("XZ preset must be between 0 and 9");
        std::process::exit(1);
    }

    let start = Instant::now();
    let res = if args.compress {
        compress_path(&args)
    } else if args.decompress {
        decompress_path(&args)
    } else {
        let mut cmd = Args::command();
        cmd.print_help()?;
        return Ok(());
    };
    res.map_err(|e| { eprintln!("Error: {}", e); e })?;

    println!("Total time: {:.2?}", start.elapsed());
    Ok(())
}

fn compress_path(args: &Args) -> io::Result<()> {
    println!("© 2025, Matheo Simard");
    println!(
        "Compression: {:?} → {:?} (XZ preset {}, Zstd lvl {})",
        args.input, args.output, args.xz_preset, args.zstd_level
    );

    let outfile = BufWriter::with_capacity(args.buffer_size, File::create(&args.output)?);
    let mut zstd_encoder = if let Some(dic) = &args.dict {
        let dict_data = fs::read(dic)?;
        ZstdEncoder::with_dictionary(outfile, args.zstd_level, &dict_data)?
    } else {
        ZstdEncoder::new(outfile, args.zstd_level)?
    };

    let mut xz_encoder = XzEncoder::new(&mut zstd_encoder, args.xz_preset);
    {
        let mut tar_builder = Builder::new(&mut xz_encoder);
        let pb = build_progress(&args.input)?;
        traverse_and_append(&args.input, &mut tar_builder, &pb, &args.exclude)?;
        pb.finish_and_clear();
    }
    xz_encoder.finish()?;
    zstd_encoder.finish()?;

    let size = fs::metadata(&args.output)?.len();
    println!("Output size: {} bytes", size);
    Ok(())
}

fn decompress_path(args: &Args) -> io::Result<()> {
    println!("© 2025, Matheo Simard");
    println!("Decompressing {:?} → {:?}", args.input, args.output);
    fs::create_dir_all(&args.output)?;

    let input_path_str = args.input.to_string_lossy();
    let ext = args.input.extension().and_then(|s| s.to_str()).unwrap_or("");

    match ext {
        "zip" => decompress_zip(&args.input, &args.output, args.buffer_size),
        "rar" => decompress_rar(&args.input, &args.output),
        "tar" => decompress_tar_plain(File::open(&args.input)?, &args.output),
        "gz" | "tgz" if input_path_str.ends_with(".tar.gz") || ext == "tgz" => {
            let f = File::open(&args.input)?;
            let gz = GzDecoder::new(f);
            decompress_tar_plain(gz, &args.output)
        }
        "bz2" if input_path_str.ends_with(".tar.bz2") => {
            let f = File::open(&args.input)?;
            let bz = BzDecoder::new(f);
            decompress_tar_plain(bz, &args.output)
        }
        _ => {
            let infile_count = BufReader::with_capacity(args.buffer_size, File::open(&args.input)?);
            let zstd_count = ZstdDecoder::new(infile_count)?;
            let xz_count = XzDecoder::new(zstd_count);
            let mut archive_count = Archive::new(xz_count);

            let entry_count = archive_count.entries()?.count();
            let pb = ProgressBar::new(entry_count as u64);
            pb.set_style(
                ProgressStyle::with_template("{spinner:.green} [{elapsed_precise}] {bar:40.cyan/blue} {pos}/{len}")
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?
                    .progress_chars("#>-"),
            );

            let infile_decompress = BufReader::with_capacity(args.buffer_size, File::open(&args.input)?);
            let zstd_decompress = ZstdDecoder::new(infile_decompress)?;
            let xz_decompress = XzDecoder::new(zstd_decompress);
            let mut archive_decompress = Archive::new(xz_decompress);

            for file in archive_decompress.entries()? {
                let mut file = file?;
                let path = file.path()?.to_path_buf();
                let outpath = args.output.join(path);

                if file.header().entry_type().is_dir() {
                    fs::create_dir_all(&outpath)?;
                } else {
                    if let Some(parent) = outpath.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    let mut outfile = File::create(&outpath)?;
                    io::copy(&mut file, &mut outfile)?;
                }
                pb.inc(1);
            }
            pb.finish_with_message("Decompression done");
            Ok(())
        }
    }
}

fn decompress_zip(input: &PathBuf, output: &PathBuf, _bufsize: usize) -> io::Result<()> {
    let f = File::open(input)?;
    let mut archive = ZipArchive::new(f)?;
    let pb = ProgressBar::new(archive.len() as u64);
    pb.set_style(
        ProgressStyle::with_template("{spinner:.green} [{elapsed_precise}] {bar:40.cyan/blue} {pos}/{len}")
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?
            .progress_chars("#>-"),
    );
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let outpath = output.join(file.name());
        if file.is_dir() {
            fs::create_dir_all(&outpath)?;
        } else {
            if let Some(p) = outpath.parent() {
                fs::create_dir_all(p)?;
            }
            let mut outfile = File::create(&outpath)?;
            io::copy(&mut file, &mut outfile)?;
        }
        pb.inc(1);
    }
    pb.finish_with_message("Zip decompression done.");
    Ok(())
}

fn decompress_rar(input: &PathBuf, output: &PathBuf) -> io::Result<()> {
    println!("Attempting RAR decompression (requires external unrar library)...");

    let mut archive = UnrarArchive::new(input.as_path())
        .open_for_processing()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("Failed to open RAR archive: {}", e)))?;

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner:.green} [{elapsed_precise}] {msg}")
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?
    );
    pb.enable_steady_tick(Duration::from_millis(100));

    let mut extracted_count = 0;

    loop {
        let current_filename_display; // Declare here to make it available after the scope

        let next_archive_state = {
            match archive.read_header() {
                Ok(Some(open_archive_with_entry)) => {
                    let entry = open_archive_with_entry.entry();
                    let entry_path = output.join(&entry.filename);
                    current_filename_display = entry.filename.display().to_string(); // Assign here

                    if entry.is_directory() {
                        fs::create_dir_all(&entry_path)?;
                        open_archive_with_entry.skip()
                            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("Failed to skip RAR directory entry: {}", e)))?
                    } else {
                        if let Some(parent) = entry_path.parent() {
                            fs::create_dir_all(parent)?;
                        }
                        open_archive_with_entry.extract_to(&entry_path)
                            .map_err(|e| {
                                // Now current_filename_display is an owned String, no borrow issue
                                io::Error::new(io::ErrorKind::Other, format!("Failed to extract RAR file '{}': {}", current_filename_display, e))
                            })?
                    }
                }
                Ok(None) => break,
                Err(e) => return Err(io::Error::new(io::ErrorKind::Other, format!("Error reading RAR header: {}", e))),
            }
        };

        archive = next_archive_state;
        extracted_count += 1;
        pb.set_message(format!("Extracting: {}", current_filename_display)); // Use the captured string
        pb.inc(1);
    }

    pb.finish_with_message(format!("RAR decompression done. Extracted {} files/directories.", extracted_count));
    Ok(())
}


fn decompress_tar_plain<R: Read>(reader: R, output: &PathBuf) -> io::Result<()> {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner:.green} [{elapsed_precise}] {msg}")
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?
    );
    pb.enable_steady_tick(Duration::from_millis(100));
    let mut archive = Archive::new(reader);
    for entry in archive.entries()? {
        let mut file = entry?;
        let path = file.path()?.to_path_buf();
        let outpath = output.join(&path);
        if file.header().entry_type().is_dir() {
            fs::create_dir_all(&outpath)?;
        } else {
            if let Some(p) = outpath.parent() {
                fs::create_dir_all(p)?;
            }
            let mut outfile = File::create(&outpath)?;
            io::copy(&mut file, &mut outfile)?;
        }
        pb.inc(1);
    }
    pb.finish_with_message("Decompression done.");
    Ok(())
}

fn build_progress(path: &PathBuf) -> io::Result<ProgressBar> {
    let count = WalkDir::new(path)
        .into_iter()
        .filter_map(Result::ok)
        .count() as u64;
    let pb = ProgressBar::new(count.max(1));
    let style = ProgressStyle::with_template(
        "{spinner:.green} [{elapsed_precise}] {bar:40.cyan/blue} {pos}/{len}"
    )
    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
    pb.set_style(style.progress_chars("#>-"));
    Ok(pb)
}

fn traverse_and_append(
    input: &PathBuf,
    builder: &mut Builder<impl Write>,
    pb: &ProgressBar,
    excludes: &[String],
) -> io::Result<()> {
    let skip = |p: &PathBuf| excludes.iter().any(|pat| p.to_string_lossy().contains(pat));
    if input.is_dir() {
        let root = input.file_name().unwrap();
        builder.append_dir(root, input)?;
        pb.inc(1);
        for entry in WalkDir::new(input).min_depth(1).into_iter().filter_map(Result::ok) {
            let path = entry.path().to_path_buf();
            if skip(&path) { continue }
            let rel = path.strip_prefix(input).unwrap();
            let tp = PathBuf::from(root).join(rel);
            if entry.file_type().is_dir() {
                builder.append_dir(&tp, &path)?;
            } else {
                let mut f = File::open(&path)?;
                builder.append_file(&tp, &mut f)?;
            }
            pb.inc(1);
        }
    } else if !skip(input) {
        let mut f = File::open(input)?;
        builder.append_file(input.file_name().unwrap(), &mut f)?;
    }
    Ok(())
}