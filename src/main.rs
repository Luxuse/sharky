use std::fs::File;
use std::io::{self, BufReader, BufWriter, Write, Read};
use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::{Parser, CommandFactory};
use indicatif::{ProgressBar, ProgressStyle};
use walkdir::WalkDir;

use flate2::{write::{DeflateEncoder}, read::{DeflateDecoder}};
use flate2::Compression;
use tar::{Archive, Builder};
use zstd::stream::{Encoder as ZstdEncoder, Decoder as ZstdDecoder};

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Compress/decompress using tar + Deflate (level 9) + Zstd (Alternative Hybrid) with streaming and progress indication"
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

    // Level for Zstd compression (0-22) - Applied last
    #[arg(short = 'l', long = "zstd-level", default_value_t = 7)] // Default 7 is a balance, use higher for ratio
    zstd_level: i32,

    // Deflate level is fixed at 9 for maximum intermediate compression
    // Removed #[arg(long = "deflate-level", default_value_t = 6)]
    // removed deflate_level: u32,
}

// Use a larger buffer size for streaming I/O
const BUFFER_SIZE: usize = 1024 * 1024; // 1MB buffer

// Function to count directory entries for progress bar
fn count_dir_entries(path: &Path) -> io::Result<u64> {
    Ok(WalkDir::new(path).into_iter().filter_map(Result::ok).count() as u64)
}

// Function to get the size of a file
fn get_file_size(path: &Path) -> io::Result<u64> {
    File::open(path)?.metadata().map(|m| m.len())
}

// Removed deflate_level from signature
fn compress_path(input: &Path, output: &Path, zstd_level: i32) -> io::Result<()> {
    // Updated message to show fixed Deflate level
    println!("copyright (c) 2025, Matheo simard");
    println!("Compressing '{:?}' -> '{:?}' with sharky {}...", input, output, zstd_level);

    let input_size = if input.is_file() {
        get_file_size(input).ok()
    } else {
        None
    };

    if let Some(size) = input_size {
        println!("- Input size: {} bytes", size);
    } else if input.is_dir() {
         println!("- Input is a directory. Preparing for entry count based progress.");
    }

    // Create the output file writer, buffered for performance
    let file_writer = BufWriter::with_capacity(BUFFER_SIZE, File::create(output)?);

    // 3. Create the Zstd encoder (Strong Main Compression), writing to the file_writer
    let zstd_encoder = ZstdEncoder::new(file_writer, zstd_level)?;

    // 2. Create the Deflate encoder (Intermediate Compression), writing to the zstd_encoder
    // Hardcoded Deflate level to 9
    let mut deflate_encoder = DeflateEncoder::new(zstd_encoder, Compression::new(9));

    // 1. Create the tar builder (Archiving), writing to the deflate_encoder
    // Use a block to scope the tar_builder's lifetime
    {
        let mut tar_builder = Builder::new(&mut deflate_encoder); // Tar builder needs a mutable reference

        if input.is_dir() {
            let total_entries = count_dir_entries(input)?;
            let pb = ProgressBar::new(total_entries);
            pb.set_style(ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} entries ({eta})")
                .unwrap()
                .progress_chars("#>-"));
            let root_name_os = input.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::Other, "Missing folder name"))?;
            let root_name_pathbuf = PathBuf::from(root_name_os);
            pb.set_message(format!("Appending directory '{:?}'", root_name_os));

            // Append the root directory entry itself and tick the bar
            tar_builder.append_dir(&root_name_pathbuf, input)?;
            pb.inc(1);

            // Manually walk the directory to append entries one by one and update the progress bar
            for entry in WalkDir::new(input).min_depth(1).into_iter().filter_map(|e| e.ok()) {
                let entry_path = entry.path();
                let entry_type = entry.file_type();
                // Strip the input directory path to get the relative path *within* the directory
                let relative_path = entry_path.strip_prefix(input).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
                // Join the root directory name (as a PathBuf) with the relative path
                let tar_path = root_name_pathbuf.join(relative_path);

                if entry_type.is_file() {
                    pb.set_message(format!("Appending file '{:?}'", tar_path));
                    let mut file = File::open(entry_path)?;
                    tar_builder.append_file(&tar_path, &mut file)?;
                } else if entry_type.is_dir() {
                     pb.set_message(format!("Appending directory '{:?}'", tar_path));
                    tar_builder.append_dir(&tar_path, entry_path)?;
                }

                pb.inc(1);
            }
             pb.finish_with_message(format!("Finished appending directory '{:?}'", root_name_os));

        } else if input.is_file() {
            let pb = ProgressBar::new_spinner();
             pb.set_style(ProgressStyle::default_spinner()
                .template("{spinner:.green} [{elapsed_precise}] {msg}")
                .unwrap());
            pb.set_message(format!("Compressing file '{:?}'...", input));
            pb.enable_steady_tick(std::time::Duration::from_millis(100));

            let mut f = File::open(input)?;
            tar_builder.append_file(input.file_name().unwrap(), &mut f)?;

            pb.finish_with_message(format!("Finished compressing file '{:?}'", input.file_name().unwrap_or_else(|| input.as_os_str())));

        } else {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Input is not file or directory"));
        }
    } // End of tar_builder scope


    // Now that tar_builder is dropped, finalize the encoders in reverse order
    let pb_finish = ProgressBar::new_spinner();
     pb_finish.set_style(ProgressStyle::default_spinner()
        .template("{spinner:.green} {msg}")
        .unwrap());
    pb_finish.enable_steady_tick(std::time::Duration::from_millis(100));

    pb_finish.set_message("Finalizing Deflate stream...");
    // finish() on DeflateEncoder returns the underlying writer (ZstdEncoder)
    let mut zstd_encoder = deflate_encoder.finish()?;

    pb_finish.set_message("Finalizing Zstd stream...");
    // finish() on ZstdEncoder returns the underlying writer (BufWriter)
    let mut file_writer = zstd_encoder.finish()?;

    pb_finish.finish_with_message("Compression streams finalized");


    // Ensure everything is written to disk
    let pb_flush = ProgressBar::new_spinner();
     pb_flush.set_style(ProgressStyle::default_spinner()
        .template("{spinner:.green} {msg}")
        .unwrap());
    pb_flush.enable_steady_tick(std::time::Duration::from_millis(100));
    pb_flush.set_message("Flushing output to disk...");
    file_writer.flush()?;
    pb_flush.finish_with_message("Output flushed to disk");


    // Report output file size
    let output_size = get_file_size(output)?;
    println!("- Output size: {} bytes", output_size);

    println!("Compression complete.");
    Ok(())
}

fn decompress_path(input: &Path, output: &Path) -> io::Result<()> {
    // Updated message
    println!("copyright (c) 2025, Matheo simard");
    println!("Decompressing '{:?}' -> '{:?}' (streaming, IN PROGRESS)...", input, output);

    // Report input file size
    let input_size = get_file_size(input)?;
    println!("- Input size: {} bytes", input_size);

    let pb = ProgressBar::new_spinner();
     pb.set_style(ProgressStyle::default_spinner()
        .template("{spinner:.green} [{elapsed_precise}] {msg}")
        .unwrap());
    pb.enable_steady_tick(std::time::Duration::from_millis(100));
    pb.set_message(format!("Opening input file '{:?}'...", input));


    // 3. Open the input file reader, buffered
    let file_reader = BufReader::with_capacity(BUFFER_SIZE, File::open(input)?);
    pb.set_message("Input file opened. Decoding Zstd stream...");

    // 2. Create the Zstd decoder, reading from the file_reader
    let zstd_decoder = ZstdDecoder::new(file_reader)?;
    pb.set_message("Zstd decoder created. Decoding Deflate stream...");

    // 1. Create the Deflate decoder, reading from the zstd_decoder
    let deflate_decoder = DeflateDecoder::new(zstd_decoder);
    pb.set_message("Deflate decoder created. Creating tar archive reader...");


    // Create the tar archive reader, reading from the deflate_decoder
    let mut archive = Archive::new(deflate_decoder);
    pb.set_message("Tar archive reader created. Ensuring output directory...");


    // Ensure the output directory exists
    std::fs::create_dir_all(output)?;
    pb.set_message(format!("Output directory '{:?}' ensured. Extracting archive contents...", output));

    // Extract the archive to the output directory
    archive.unpack(output)?;
    pb.finish_with_message("Extraction complete.");

    // Note: Getting the exact size of the extracted content requires summing up
    // all files recursively after extraction, which can be slow.
    println!("Decompression complete.");
    Ok(())
}

fn main() {
    let args = Args::parse();
     // Validate Zstd level range
    if args.compress && (args.zstd_level < 0 || args.zstd_level > 22) {
        eprintln!("Error: Zstd compression level must be between 0 and 22.");
        std::process::exit(1);
    }
    // Removed Deflate level validation as it's hardcoded

    let start = Instant::now();
    let result = if args.compress {
        // Removed deflate_level from function call
        compress_path(&args.input, &args.output, args.zstd_level)
    } else if args.decompress {
        // Levels are not used for decompression
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
    let duration = start.elapsed();
    println!("Total operation time: {:.2?}", duration);
}