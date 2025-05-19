use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::{Parser, CommandFactory};
use indicatif::{ProgressBar, ProgressStyle};
use walkdir::WalkDir;

use lz4_flex::frame::{FrameDecoder, FrameEncoder};
use tar::{Archive, Builder};
use xz2::{read::XzDecoder, write::XzEncoder};

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Compress or decompress files/directories using tar + LZ4 frame + LZMA2"
)]
struct Args {
    #[arg(short = 'c', long = "compress", conflicts_with = "decompress")]
    compress: bool,

    #[arg(short = 'd', long = "decompress", conflicts_with = "compress")]
    decompress: bool,

    #[arg(short, long)]
    input: PathBuf,

    #[arg(short, long)]
    output: PathBuf,

    #[arg(short = 'l', long = "level", default_value_t = 7)]
    level: u32,
}

fn count_dir_entries(path: &Path) -> io::Result<usize> {
    Ok(WalkDir::new(path).into_iter().filter_map(Result::ok).count())
}

fn compress_path(input: &Path, output: &Path, level: u32) -> io::Result<()> {
    println!("Compressing '{:?}' -> '{:?}' with XZ level {}...", input, output, level);

    // Build tar archive – **preserve the top‑level folder** so that
        // extracting reproduces the original directory structure (foo/…)
        let mut tar_data = Vec::new();
    {
        let mut builder = Builder::new(&mut tar_data);
        if input.is_dir() {
            // Add the directory itself as root, then all its children
            let root = input.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::Other, "Missing folder name"))?;
            builder.append_dir_all(root, input)?;
        } else if input.is_file() {
            let mut f = File::open(input)?;
            builder.append_file(input.file_name().unwrap(), &mut f)?;
        } else {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Input is not file or directory"));
        }
        builder.finish()?;
    }
    println!("- Tar archive built ({} bytes)", tar_data.len());

    // LZ4 compression
    let mut lz4 = FrameEncoder::new(Vec::new());
    lz4.write_all(&tar_data)?;
    let lz4_data = lz4.finish()?;
    println!("- LZ4 compressed ({} bytes)", lz4_data.len());

    // XZ compression
    let mut xz = XzEncoder::new(Vec::new(), level);
    xz.write_all(&lz4_data)?;
    let xz_data = xz.finish()?;
    println!("- XZ compressed ({} bytes)", xz_data.len());

    // Write to output
    File::create(output)?.write_all(&xz_data)?;
    println!("Output written");
    Ok(())
}

fn decompress_path(input: &Path, output: &Path) -> io::Result<()> {
    println!("Decompressing '{:?}' -> '{:?}'...", input, output);
    // Read entire input
    let mut buf = Vec::new();
    File::open(input)?.read_to_end(&mut buf)?;

    // XZ decode
    let mut xz_dec = XzDecoder::new(&buf[..]);
    let mut lz4_buf = Vec::new();
    xz_dec.read_to_end(&mut lz4_buf)?;

    // LZ4 decode
    let mut lz4_dec = FrameDecoder::new(&lz4_buf[..]);
    let mut tar_buf = Vec::new();
    lz4_dec.read_to_end(&mut tar_buf)?;

    // Untar all, preserving the top-level folder
    let mut archive = Archive::new(&tar_buf[..]);
    std::fs::create_dir_all(output)?;
    archive.unpack(output)?;

    println!("Extraction done");
    Ok(())
}

fn main() {
    let args = Args::parse();
    let start = Instant::now();
    let result = if args.compress {
        compress_path(&args.input, &args.output, args.level)
    } else if args.decompress {
        decompress_path(&args.input, &args.output)
    } else {
        let mut cmd = Args::command();
        cmd.print_help().unwrap();
        return;
    };
    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
    println!("Done in {:.2?}", start.elapsed());
}
