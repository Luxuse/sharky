use std::{
    fs::{self, File},
    io::{self, BufReader, BufWriter, Read, Write, Seek, SeekFrom},
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
use unrar::Archive as UnrarArchive;
use sevenz_rust::SevenZReader;
use lzma_rs::lzma_decompress;
use brotli::Decompressor as BrotliDecoder;

// Structures pour le support ISO
struct IsoDirectory {
    name: String,
    entries: Vec<IsoEntry>,
}

struct IsoFile {
    name: String,
    size: u32,
    location: u32,
}

enum IsoEntry {
    Directory(IsoDirectory),
    File(IsoFile),
}

/// Outil de compression/décompression multi-format
#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Outil de compression/décompression supportant ZIP, RAR, 7Z, ISO, TAR, GZ, BZ2, XZ, ZSTD, LZMA, BROTLI"
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

    match ext.to_lowercase().as_str() {
        "zip" => decompress_zip(&args.input, &args.output, args.buffer_size),
        "rar" => decompress_rar(&args.input, &args.output),
        "7z" => decompress_7z(&args.input, &args.output),
        "iso" => decompress_iso(&args.input, &args.output, args.buffer_size),
        "tar" => decompress_tar_plain(File::open(&args.input)?, &args.output),
        "gz" => {
            if input_path_str.ends_with(".tar.gz") {
                let f = File::open(&args.input)?;
                let gz = GzDecoder::new(f);
                decompress_tar_plain(gz, &args.output)
            } else {
                decompress_single_file_gz(&args.input, &args.output)
            }
        },
        "tgz" => {
            let f = File::open(&args.input)?;
            let gz = GzDecoder::new(f);
            decompress_tar_plain(gz, &args.output)
        },
        "bz2" => {
            if input_path_str.ends_with(".tar.bz2") {
                let f = File::open(&args.input)?;
                let bz = BzDecoder::new(f);
                decompress_tar_plain(bz, &args.output)
            } else {
                decompress_single_file_bz2(&args.input, &args.output)
            }
        },
        "xz" => {
            if input_path_str.ends_with(".tar.xz") {
                let f = File::open(&args.input)?;
                let xz = XzDecoder::new(f);
                decompress_tar_plain(xz, &args.output)
            } else {
                decompress_single_file_xz(&args.input, &args.output)
            }
        },
        "zst" | "zstd" => {
            if input_path_str.ends_with(".tar.zst") || input_path_str.ends_with(".tar.zstd") {
                let f = File::open(&args.input)?;
                let zstd = ZstdDecoder::new(f)?;
                decompress_tar_plain(zstd, &args.output)
            } else {
                decompress_single_file_zstd(&args.input, &args.output)
            }
        },
        "lzma" => decompress_single_file_lzma(&args.input, &args.output),
        "br" => decompress_single_file_brotli(&args.input, &args.output),
        "lz4" => decompress_single_file_lz4(&args.input, &args.output),
        "cab" => decompress_cab(&args.input, &args.output),
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
        let current_filename_display;

        let next_archive_state = {
            match archive.read_header() {
                Ok(Some(open_archive_with_entry)) => {
                    let entry = open_archive_with_entry.entry();
                    let entry_path = output.join(&entry.filename);
                    current_filename_display = entry.filename.display().to_string();

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
        pb.set_message(format!("Extracting: {}", current_filename_display));
        pb.inc(1);
    }

    pb.finish_with_message(format!("RAR decompression done. Extracted {} files/directories.", extracted_count));
    Ok(())
}

fn decompress_iso(input: &PathBuf, output: &PathBuf, buffer_size: usize) -> io::Result<()> {
    println!("Attempting ISO decompression...");
    
    let mut file = File::open(input)?;
    
    // Vérifier la signature ISO 9660
    let mut buffer = [0u8; 8];
    file.seek(SeekFrom::Start(32768))?; // Volume descriptor commence à 32KB
    file.read_exact(&mut buffer)?;
    
    if &buffer[1..6] != b"CD001" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid ISO 9660 signature"
        ));
    }
    
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner:.green} [{elapsed_precise}] {msg}")
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?
    );
    pb.enable_steady_tick(Duration::from_millis(100));
    pb.set_message("Reading ISO structure...");
    
    // Lire le Primary Volume Descriptor
    let mut pvd = [0u8; 2048];
    file.seek(SeekFrom::Start(32768))?;
    file.read_exact(&mut pvd)?;
    
    // Extraire les informations du répertoire racine
    let root_dir_location = u32::from_le_bytes([pvd[158], pvd[159], pvd[160], pvd[161]]);
    let root_dir_size = u32::from_le_bytes([pvd[166], pvd[167], pvd[168], pvd[169]]);
    
    pb.set_message("Extracting files...");
    
    let mut extracted_count = 0;
    extract_iso_directory(
        &mut file, 
        root_dir_location, 
        root_dir_size, 
        output, 
        "",
        &pb,
        &mut extracted_count,
        buffer_size
    )?;
    
    pb.finish_with_message(format!("ISO decompression done. Extracted {} files/directories.", extracted_count));
    Ok(())
}

fn extract_iso_directory(
    file: &mut File,
    location: u32,
    size: u32,
    output_base: &PathBuf,
    current_path: &str,
    pb: &ProgressBar,
    extracted_count: &mut u32,
    buffer_size: usize,
) -> io::Result<()> {
    let sector_size = 2048u32;
    let start_pos = (location as u64) * (sector_size as u64);
    
    file.seek(SeekFrom::Start(start_pos))?;
    let mut dir_data = vec![0u8; size as usize];
    file.read_exact(&mut dir_data)?;
    
    let mut offset = 0;
    while offset < size as usize {
        if dir_data[offset] == 0 {
            break;
        }
        
        let record_length = dir_data[offset] as usize;
        if record_length == 0 || offset + record_length > size as usize {
            break;
        }
        
        let name_length = dir_data[offset + 32] as usize;
        if name_length > 0 && offset + 33 + name_length <= size as usize {
            let name_bytes = &dir_data[offset + 33..offset + 33 + name_length];
            
            // Clean up file name - remove version info and handle special characters
            let mut name = String::new();
            for &b in name_bytes {
                if b == b';' {
                    break;
                }
                // Replace NUL and other problematic characters
                if b >= 32 && b < 127 && b != b'<' && b != b'>' && b != b':' && b != b'"' 
                    && b != b'/' && b != b'\\' && b != b'|' && b != b'?' && b != b'*' {
                    name.push(b as char);
                }
            }
            
            // Skip empty names and special entries
            if !name.is_empty() && name != "." && name != ".." {
                let file_location = u32::from_le_bytes([
                    dir_data[offset + 2],
                    dir_data[offset + 3],
                    dir_data[offset + 4],
                    dir_data[offset + 5]
                ]);
                
                let file_size = u32::from_le_bytes([
                    dir_data[offset + 10],
                    dir_data[offset + 11],
                    dir_data[offset + 12],
                    dir_data[offset + 13]
                ]);
                
                let flags = dir_data[offset + 25];
                let is_directory = (flags & 0x02) != 0;
                
                let full_path = if current_path.is_empty() {
                    name.clone()
                } else {
                    format!("{}/{}", current_path, name)
                };
                
                // Convert path to safe Windows format
                let safe_path = full_path.replace('/', "\\");
                let output_path = output_base.join(safe_path);
                
                if let Err(e) = if is_directory {
                    fs::create_dir_all(&output_path).and_then(|_| {
                        pb.set_message(format!("Created directory: {}", output_path.display()));
                        extract_iso_directory(
                            file,
                            file_location,
                            file_size,
                            output_base,
                            &full_path,
                            pb,
                            extracted_count,
                            buffer_size
                        )
                    })
                } else {
                    if let Some(parent) = output_path.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    
                    pb.set_message(format!("Extracting: {}", output_path.display()));
                    
                    let file_start = (file_location as u64) * (sector_size as u64);
                    file.seek(SeekFrom::Start(file_start))?;
                    
                    let mut output_file = File::create(&output_path)?;
                    let mut remaining = file_size as u64;
                    let mut buffer = vec![0u8; buffer_size.min(remaining as usize)];
                    
                    while remaining > 0 {
                        let to_read = buffer_size.min(remaining as usize);
                        let bytes_read = file.read(&mut buffer[..to_read])?;
                        if bytes_read == 0 {
                            break;
                        }
                        output_file.write_all(&buffer[..bytes_read])?;
                        remaining -= bytes_read as u64;
                    }
                    Ok(())
                } {
                    eprintln!("Warning: Failed to extract '{}': {}", output_path.display(), e);
                    continue;
                }
                
                *extracted_count += 1;
                pb.inc(1);
            }
        }
        
        offset += record_length;
    }
    
    Ok(())
}

fn decompress_7z(input: &PathBuf, output: &PathBuf) -> io::Result<()> {
    println!("Attempting 7Z decompression...");
    
    let file = File::open(input)?;
    let file_size = file.metadata()?.len();
    
    let mut reader = SevenZReader::new(file, file_size, sevenz_rust::Password::empty())
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("Failed to open 7Z archive: {}", e)))?;
    
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner:.green} [{elapsed_precise}] {msg}")
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?
    );
    pb.enable_steady_tick(Duration::from_millis(100));
    
    let mut extracted_count = 0;
    
    reader.for_each_entries(|entry, reader| {
        let entry_path = output.join(&entry.name);
        
        pb.set_message(format!("Extracting: {}", entry.name));
        
        if entry.is_directory() {
            fs::create_dir_all(&entry_path)?;
        } else {
            if let Some(parent) = entry_path.parent() {
                fs::create_dir_all(parent)?;
            }
            
            let mut output_file = File::create(&entry_path)?;
            io::copy(reader, &mut output_file)?;
        }
        
        extracted_count += 1;
        pb.inc(1);
        Ok(true)
    }).map_err(|e| io::Error::new(io::ErrorKind::Other, format!("7Z extraction error: {}", e)))?;
    
    pb.finish_with_message(format!("7Z decompression done. Extracted {} files/directories.", extracted_count));
    Ok(())
}

fn decompress_single_file_gz(input: &PathBuf, output: &PathBuf) -> io::Result<()> {
    let input_file = File::open(input)?;
    let mut decoder = GzDecoder::new(input_file);
    
    let output_name = input.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("decompressed");
    let output_file_path = output.join(output_name);
    
    if let Some(parent) = output_file_path.parent() {
        fs::create_dir_all(parent)?;
    }
    
    let mut output_file = File::create(&output_file_path)?;
    io::copy(&mut decoder, &mut output_file)?;
    
    println!("GZ decompression done: {:?}", output_file_path);
    Ok(())
}

fn decompress_single_file_bz2(input: &PathBuf, output: &PathBuf) -> io::Result<()> {
    let input_file = File::open(input)?;
    let mut decoder = BzDecoder::new(input_file);
    
    let output_name = input.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("decompressed");
    let output_file_path = output.join(output_name);
    
    if let Some(parent) = output_file_path.parent() {
        fs::create_dir_all(parent)?;
    }
    
    let mut output_file = File::create(&output_file_path)?;
    io::copy(&mut decoder, &mut output_file)?;
    
    println!("BZ2 decompression done: {:?}", output_file_path);
    Ok(())
}

fn decompress_single_file_xz(input: &PathBuf, output: &PathBuf) -> io::Result<()> {
    let input_file = File::open(input)?;
    let mut decoder = XzDecoder::new(input_file);
    
    let output_name = input.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("decompressed");
    let output_file_path = output.join(output_name);
    
    if let Some(parent) = output_file_path.parent() {
        fs::create_dir_all(parent)?;
    }
    
    let mut output_file = File::create(&output_file_path)?;
    io::copy(&mut decoder, &mut output_file)?;
    
    println!("XZ decompression done: {:?}", output_file_path);
    Ok(())
}

fn decompress_single_file_zstd(input: &PathBuf, output: &PathBuf) -> io::Result<()> {
    let input_file = File::open(input)?;
    let mut decoder = ZstdDecoder::new(input_file)?;
    
    let output_name = input.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("decompressed");
    let output_file_path = output.join(output_name);
    
    if let Some(parent) = output_file_path.parent() {
        fs::create_dir_all(parent)?;
    }
    
    let mut output_file = File::create(&output_file_path)?;
    io::copy(&mut decoder, &mut output_file)?;
    
    println!("ZSTD decompression done: {:?}", output_file_path);
    Ok(())
}

fn decompress_single_file_lzma(input: &PathBuf, output: &PathBuf) -> io::Result<()> {
    let input_data = fs::read(input)?;
    let mut output_data = Vec::new();
    
    lzma_decompress(&mut input_data.as_slice(), &mut output_data)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("LZMA decompression error: {}", e)))?;
    
    let output_name = input.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("decompressed");
    let output_file_path = output.join(output_name);
    
    if let Some(parent) = output_file_path.parent() {
        fs::create_dir_all(parent)?;
    }
    
    fs::write(&output_file_path, output_data)?;
    
    println!("LZMA decompression done: {:?}", output_file_path);
    Ok(())
}

fn decompress_single_file_brotli(input: &PathBuf, output: &PathBuf) -> io::Result<()> {
    let input_file = File::open(input)?;
    let mut decoder = BrotliDecoder::new(input_file, 4096);
    
    let output_name = input.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("decompressed");
    let output_file_path = output.join(output_name);
    
    if let Some(parent) = output_file_path.parent() {
        fs::create_dir_all(parent)?;
    }
    
    let mut output_file = File::create(&output_file_path)?;
    io::copy(&mut decoder, &mut output_file)?;
    
    println!("Brotli decompression done: {:?}", output_file_path);
    Ok(())
}

fn decompress_single_file_lz4(input: &PathBuf, output: &PathBuf) -> io::Result<()> {
    // Pour LZ4, nous utiliserons une implémentation simple
    // Vous devrez ajouter la crate lz4_flex à vos dépendances
    let input_data = fs::read(input)?;
    
    // Décompression LZ4 (nécessite lz4_flex crate)
    let decompressed = lz4_flex::decompress_size_prepended(&input_data)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("LZ4 decompression error: {}", e)))?;
    
    let output_name = input.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("decompressed");
    let output_file_path = output.join(output_name);
    
    if let Some(parent) = output_file_path.parent() {
        fs::create_dir_all(parent)?;
    }
    
    fs::write(&output_file_path, decompressed)?;
    
    println!("LZ4 decompression done: {:?}", output_file_path);
    Ok(())
}

fn decompress_cab(input: &PathBuf, output: &PathBuf) -> io::Result<()> {
    println!("CAB decompression not fully implemented - requires external library");
    // Pour les fichiers CAB, vous pourriez utiliser une crate comme `cab` ou appeler un outil externe
    // Voici un exemple basique qui nécessiterait l'ajout d'une crate appropriée
    
    println!("CAB files require additional implementation. File: {:?}", input);
    println!("Consider using external tools like 'cabextract' for now.");

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner:.green} [{elapsed_precise}] {msg}")
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?
    );
    pb.enable_steady_tick(Duration::from_millis(100));

    // Initialize the reader variable (example: using a file input)
    let file = File::open(input)?;
    let reader = BufReader::new(file);

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

fn decompress_tar_plain<R: Read>(reader: R, output: &PathBuf) -> io::Result<()> {
    let mut archive = Archive::new(reader);
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner:.green} [{elapsed_precise}] {msg}")
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?
    );
    pb.enable_steady_tick(Duration::from_millis(100));

    for entry in archive.entries()? {
        let mut file = entry?;
        let path = file.path()?.to_path_buf();
        let outpath = output.join(&path);
        
        pb.set_message(format!("Extracting: {}", path.display()));

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
    
    pb.finish_with_message("TAR extraction complete");
    Ok(())
}