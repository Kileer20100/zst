use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use tar::{Builder, Header, Archive};
use walkdir::WalkDir;
use zstd::stream::Encoder;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Compress {
        input_folder: String,
        output_file: String,
    },
    Decompress {
        input_file: String,
        output_folder: String,
    },
}

struct FileData {
    rel_path: PathBuf,
    compressed: Vec<u8>,
    header: Header,
    success: bool,
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Compress { input_folder, output_file } => {
            compress_folder(&input_folder, &output_file)?
        }
        Commands::Decompress { input_file, output_folder } => {
            decompress_folder(&input_file, &output_folder)?
        }
    }

    Ok(())
}

fn compress_folder(input_folder: &str, output_file: &str) -> io::Result<()> {
    let entries: Vec<_> = WalkDir::new(input_folder)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .collect();

    let total_size: u64 = entries.iter().map(|e| e.metadata().unwrap().len()).sum();

    let global_pb = ProgressBar::new(total_size);
    global_pb.set_style(
        ProgressStyle::with_template(
            "[{elapsed_precise}] {bar:40.cyan/blue} {bytes}/{total_bytes} ({percent}%)"
        )
        .unwrap()
        .progress_chars("█▉▊▋▌▍▎▏ "),
    );

    let results = Arc::new(Mutex::new(Vec::<FileData>::new()));

    entries.par_iter().for_each(|entry| {
        let path = entry.path().to_path_buf();
        let rel_path = path.strip_prefix(input_folder).unwrap().to_path_buf();
        let file_size = entry.metadata().unwrap().len();

        let mut file = match File::open(&path) {
            Ok(f) => f,
            Err(_) => {
                results.lock().unwrap().push(FileData {
                    rel_path,
                    compressed: vec![],
                    header: Header::new_gnu(),
                    success: false,
                });
                return;
            }
        };

        let mut buffer = Vec::with_capacity(file_size as usize);
        let mut chunk = [0u8; 8192];
        loop {
            let n = match file.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => {
                    results.lock().unwrap().push(FileData {
                        rel_path,
                        compressed: vec![],
                        header: Header::new_gnu(),
                        success: false,
                    });
                    return;
                }
            };
            buffer.extend_from_slice(&chunk[..n]);
            global_pb.inc(n as u64);
        }

        // Compress
        let compressed = match zstd::encode_all(&buffer[..], 21) {
            Ok(c) => c,
            Err(_) => {
                results.lock().unwrap().push(FileData {
                    rel_path,
                    compressed: vec![],
                    header: Header::new_gnu(),
                    success: false,
                });
                return;
            }
        };

        let mut header = Header::new_gnu();
        if let Err(_) = header.set_path(&rel_path) {
            results.lock().unwrap().push(FileData {
                rel_path,
                compressed,
                header: Header::new_gnu(),
                success: false,
            });
            return;
        }
        header.set_size(compressed.len() as u64);
        header.set_cksum();

        results.lock().unwrap().push(FileData {
            rel_path,
            compressed,
            header,
            success: true,
        });
    });

    global_pb.finish_with_message("📦 Сжатие завершено");

    // Сохраняем архив
    let output = File::create(output_file)?;
    let writer = BufWriter::new(Encoder::new(output, 21)?.auto_finish());
    let mut tar = Builder::new(writer);

    for file in results.lock().unwrap().iter().filter(|f| f.success) {
        tar.append(&file.header, &file.compressed[..])?;
    }

    println!("\n📃 Результаты:");
    for file in results.lock().unwrap().iter() {
        println!(
            "{:<60} [{}]",
            file.rel_path.display(),
            if file.success { "OK" } else { "ERR" }
        );
    }

    println!("\n✅ Папка '{}' сжата в '{}'", input_folder, output_file);
    Ok(())
}

fn decompress_folder(input_file: &str, output_folder: &str) -> io::Result<()> {
    let file = File::open(input_file)?;
    let decoder = zstd::stream::Decoder::new(BufReader::new(file))?;
    let mut archive = Archive::new(decoder);

    fs::create_dir_all(output_folder)?;
    archive.unpack(output_folder)?;
    println!("✅ Архив '{}' распакован в '{}'", input_file, output_folder);
    Ok(())
}
