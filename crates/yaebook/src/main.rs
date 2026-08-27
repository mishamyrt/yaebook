use std::env;
use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use yaebook_core::{Client, build_output_path, parse_book_id};
use yaebook_epub::prepare_epub;

const HELP: &str = r"yaebook — download a Yandex Books title as EPUB

Usage:
  yaebook [OPTIONS] <BOOK_URL | UUID>

Arguments:
  BOOK_URL | UUID          A book URL such as https://books.yandex.ru/books/UUID

Options:
  --token TOKEN            OAuth token; otherwise YA_BOOKS_TOKEN is used
  -o, --output-dir PATH    Export directory (default: current directory)
  -h, --help               Show help
  -V, --version            Show version
";

#[allow(clippy::print_stderr, clippy::print_stdout)]
fn main() {
    match parse_args(env::args_os().skip(1)) {
        Ok(Command::Help) => print!("{HELP}"),
        Ok(Command::Version) => println!("yaebook {}", env!("CARGO_PKG_VERSION")),
        Ok(Command::Run(options)) => {
            if let Err(error) = run(options) {
                eprintln!("Error: {error}");
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("Error: {error}\n\n{HELP}");
            std::process::exit(2);
        }
    }
}

#[allow(clippy::print_stderr, clippy::print_stdout)]
fn run(options: Options) -> Result<(), CliError> {
    let token = match options.token {
        Some(token) => token,
        None => env::var("YA_BOOKS_TOKEN").map_err(|_| {
            CliError(
                "provide --token or set the YA_BOOKS_TOKEN environment variable"
                    .into(),
            )
        })?,
    };
    let token = token.trim();
    if token.is_empty() {
        return Err(CliError("the token must not be empty".into()));
    }

    let uuid = parse_book_id(&options.book).map_err(CliError::from_display)?;
    let client = Client::new(token).map_err(CliError::from_display)?;
    let downloaded = client
        .download_book(&uuid, |message| eprintln!("{message}"))
        .map_err(CliError::from_display)?;
    let output_path =
        build_output_path(&options.output_directory, &downloaded.metadata);
    let epub = prepare_epub(
        &downloaded.epub,
        &downloaded.metadata,
        downloaded.cover.as_deref(),
        |message| eprintln!("{message}"),
    )
    .map_err(CliError::from_display)?;

    eprintln!("Saving file…");
    atomic_write(&output_path, &epub)?;
    println!("Done: {}", output_path.display());
    Ok(())
}

fn atomic_write(path: &Path, content: &[u8]) -> Result<(), CliError> {
    let parent = path.parent().ok_or_else(|| {
        CliError("could not determine the output directory".into())
    })?;
    fs::create_dir_all(parent).map_err(CliError::from_display)?;
    let file_name = path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| CliError("the output filename is not valid UTF-8".into()))?;
    let temporary = parent.join(format!(".{file_name}.{}.part", std::process::id()));

    let result = (|| {
        let mut file = File::create(&temporary)?;
        file.write_all(content)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(CliError::from_display)
}

#[derive(Debug, Eq, PartialEq)]
struct Options {
    token: Option<String>,
    book: String,
    output_directory: PathBuf,
}

#[derive(Debug, Eq, PartialEq)]
enum Command {
    Run(Options),
    Help,
    Version,
}

fn parse_args(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<Command, CliError> {
    let mut arguments = arguments.into_iter();
    let mut token = None;
    let mut output_directory = None;
    let mut positional = Vec::new();
    let mut options = true;

    while let Some(argument) = arguments.next() {
        if options && argument == "--" {
            options = false;
        } else if options && (argument == "-h" || argument == "--help") {
            return Ok(Command::Help);
        } else if options && (argument == "-V" || argument == "--version") {
            return Ok(Command::Version);
        } else if options && argument == "--token" {
            let value = arguments
                .next()
                .ok_or_else(|| CliError("expected a value after --token".into()))?;
            token = Some(unicode_argument(value, "token")?);
        } else if options
            && argument.to_str().is_some_and(|value| value.starts_with("--token="))
        {
            let value = argument.to_string_lossy();
            token = Some(value["--token=".len()..].to_owned());
        } else if options && (argument == "-o" || argument == "--output-dir") {
            output_directory =
                Some(PathBuf::from(arguments.next().ok_or_else(|| {
                    CliError(format!(
                        "expected a value after {}",
                        argument.to_string_lossy()
                    ))
                })?));
        } else if options
            && argument
                .to_str()
                .is_some_and(|value| value.starts_with("--output-dir="))
        {
            let value = argument.to_string_lossy();
            output_directory = Some(PathBuf::from(&value["--output-dir=".len()..]));
        } else if options && argument.to_string_lossy().starts_with('-') {
            return Err(CliError(format!(
                "unknown option: {}",
                argument.to_string_lossy()
            )));
        } else {
            positional.push(argument);
        }
    }

    if positional.is_empty() {
        return Err(CliError("book URL is missing".into()));
    }
    if positional.len() > 1 {
        return Err(CliError("expected only a book URL".into()));
    }
    let book = unicode_argument(positional.remove(0), "book URL")?;
    let output_directory = output_directory.unwrap_or_else(|| PathBuf::from("."));
    Ok(Command::Run(Options { token, book, output_directory }))
}

fn unicode_argument(value: OsString, name: &str) -> Result<String, CliError> {
    value.into_string().map_err(|_| CliError(format!("{name} is not valid UTF-8")))
}

#[derive(Debug)]
struct CliError(String);

impl CliError {
    fn from_display(error: impl fmt::Display) -> Self {
        Self(error.to_string())
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for CliError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_token_book_and_output_directory() {
        let command = parse_args([
            "--token".into(),
            "secret".into(),
            "--output-dir".into(),
            "/tmp/books".into(),
            "https://books.yandex.ru/books/HLwwn7ea".into(),
        ])
        .unwrap();
        assert_eq!(
            command,
            Command::Run(Options {
                token: Some("secret".into()),
                book: "https://books.yandex.ru/books/HLwwn7ea".into(),
                output_directory: PathBuf::from("/tmp/books"),
            })
        );
    }

    #[test]
    fn accepts_short_output_directory_option() {
        let command =
            parse_args(["HLwwn7ea".into(), "-o".into(), "/tmp/books".into()])
                .unwrap();
        let Command::Run(options) = command else {
            panic!("expected run command");
        };
        assert_eq!(options.output_directory, Path::new("/tmp/books"));
    }

    #[test]
    fn defaults_to_current_directory_and_rejects_invalid_arguments() {
        let command = parse_args(["HLwwn7ea".into()]).unwrap();
        let Command::Run(options) = command else {
            panic!("expected run command");
        };
        assert_eq!(options.output_directory, Path::new("."));
        assert!(parse_args(["--interactive".into(), "HLwwn7ea".into()]).is_err());
        assert!(parse_args(["HLwwn7ea".into(), "/tmp/books".into()]).is_err());
    }
}
