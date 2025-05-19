use std::{
    fs::{self, File},
    io::{self, BufReader, BufWriter, Write},
    path::PathBuf,
    time::Instant,
};

use clap::{Parser, CommandFactory};
use indicatif::{ProgressBar, ProgressStyle};
use walkdir::WalkDir;

use xz2::write::XzEncoder;
use xz2::read::XzDecoder;
use tar::{Archive, Builder, Entry};
use zstd::stream::write::Encoder as ZstdEncoder;
use zstd::stream::read::Decoder as ZstdDecoder;

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
    let mut zstd = if let Some(dic) = &args.dict {
        let dict_data = fs::read(dic)?;
        ZstdEncoder::with_dictionary(outfile, args.zstd_level, &dict_data)?
    } else {
        ZstdEncoder::new(outfile, args.zstd_level)?
    };

    let mut xz = XzEncoder::new(&mut zstd, args.xz_preset);
    {
        let mut tar = Builder::new(&mut xz);
        let pb = build_progress(&args.input)?;
        traverse_and_append(&args.input, &mut tar, &pb, &args.exclude)?;
        pb.finish_and_clear();
    }
    xz.finish()?;
    zstd.finish()?;

    let size = fs::metadata(&args.output)?.len();
    println!("Output size: {} bytes", size);
    Ok(())
}

fn decompress_path(args: &Args) -> io::Result<()> {
    println!("© 2025, Matheo Simard");
    println!("Decompressing {:?} → {:?}", args.input, args.output);

    fs::create_dir_all(&args.output)?;
    let infile = BufReader::with_capacity(args.buffer_size, File::open(&args.input)?);
    let zstd = ZstdDecoder::new(infile)?;
    let xz = XzDecoder::new(zstd);
    let mut archive = Archive::new(xz);

    // Count the number of entries in the archive
    let entry_count = archive.entries()?.count();
    let pb = ProgressBar::new(entry_count as u64);
    pb.set_style(
        ProgressStyle::with_template("{spinner:.green} [{elapsed_precise}] {bar:40.cyan/blue} {pos}/{len}")
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?,
    );

    // Reset the archive to the beginning
    let infile = BufReader::with_capacity(args.buffer_size, File::open(&args.input)?);
    let zstd = ZstdDecoder::new(infile)?;
    let xz = XzDecoder::new(zstd);
    let mut archive = Archive::new(xz);

    for file in archive.entries()? {
        let mut file = file?;
        let path = file.path()?.to_path_buf();
        let outpath = args.output.join(path);

        if file.header().entry_type() == tar::EntryType::Directory {
            fs::create_dir_all(&outpath)?;
        } else {
            if let Some(parent) = outpath.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let mut outfile = File::create(&outpath)?;
            io::copy(&mut file, &mut outfile)?;
        }
        pb.inc(1);
    }

    pb.finish_with_message("Decompression done");
    Ok(())
}

fn build_progress(path: &PathBuf) -> io::Result<ProgressBar> {
    let count = WalkDir::new(path).into_iter().filter_map(Result::ok).count() as u64;
    let pb = ProgressBar::new(count.max(1));
    let style = ProgressStyle::with_template(
        "{spinner:.green} [{elapsed_precise}] {bar:40.cyan/blue} {pos}/{len}"
    )
    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
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
