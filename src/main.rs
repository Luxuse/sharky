use std::{
    fs::{self, File},
    io::{self, BufReader, BufWriter, Read, Write},
    path::PathBuf,
    time::{Duration, Instant}, // Added back Duration
};

use clap::{CommandFactory, Parser};
use indicatif::{ProgressBar, ProgressStyle};
use walkdir::WalkDir;

// Compression/decompression libs
// Ensure these are present and uncommented in your Cargo.toml:
// zip = "3.0.0"
// flate2 = "1.0"
// bzip2 = "0.5.2"
// xz2 = "0.1.7"
// tar = "0.4"
// zstd = "0.13.3"

use bzip2::read::BzDecoder; // Used for .tar.bz2
use flate2::read::GzDecoder; // Used for .tar.gz
use tar::{Archive, Builder};
use xz2::read::XzDecoder;
use xz2::write::XzEncoder;
use zip::ZipArchive; // Used for .zip files
use zstd::stream::read::Decoder as ZstdDecoder;
use zstd::stream::write::Encoder as ZstdEncoder;

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
        // Handles generic .tar files. The `decompress_tar_plain` function doesn't need buffer_size as a param.
        "tar" => decompress_tar_plain(File::open(&args.input)?, &args.output),
        // Handles .tar.gz or .tgz
        "gz" | "tgz" if input_path_str.ends_with(".tar.gz") || ext == "tgz" => {
            let f = File::open(&args.input)?;
            let gz = GzDecoder::new(f);
            decompress_tar_plain(gz, &args.output)
        }
        // Handles .tar.bz2
        "bz2" if input_path_str.ends_with(".tar.bz2") => {
            let f = File::open(&args.input)?;
            let bz = BzDecoder::new(f);
            decompress_tar_plain(bz, &args.output)
        }
        // Default to XZ + Zstd if no other format matches the extension
        _ => {
            // First pass: Count entries for progress bar
            let infile_count = BufReader::with_capacity(args.buffer_size, File::open(&args.input)?);
            let zstd_count = ZstdDecoder::new(infile_count)?;
            let xz_count = XzDecoder::new(zstd_count);
            let mut archive_count = Archive::new(xz_count);

            // `entries()` consumes the archive, so we must recreate the stream for the second pass
            let entry_count = archive_count.entries()?.count();
            let pb = ProgressBar::new(entry_count as u64);
            pb.set_style(
                ProgressStyle::with_template("{spinner:.green} [{elapsed_precise}] {bar:40.cyan/blue} {pos}/{len}")
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?
                    .progress_chars("#>-"),
            );

            // Second pass: Actual decompression
            // Re-open the file and re-create the decompression chain
            let infile_decompress = BufReader::with_capacity(args.buffer_size, File::open(&args.input)?);
            let zstd_decompress = ZstdDecoder::new(infile_decompress)?;
            let xz_decompress = XzDecoder::new(zstd_decompress);
            let mut archive_decompress = Archive::new(xz_decompress);

            for file in archive_decompress.entries()? { // Iterate using the new archive instance
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

// Removed 'mut' from 'reader' as it's not necessary here.
fn decompress_tar_plain<R: Read>(reader: R, output: &PathBuf) -> io::Result<()> {
    let pb = ProgressBar::new_spinner(); // Use spinner for indeterminate progress if we can't count entries
    pb.set_style(
        ProgressStyle::with_template("{spinner:.green} [{elapsed_precise}] {msg}")
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?
    );
    pb.enable_steady_tick(Duration::from_millis(100)); // Spinner tick rate
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
        pb.inc(1); // Increment for each entry processed
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