//! Yandex Books API client, shared book model, and output naming.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

const BASE_URL: &str = "https://api.bookmate.yandex.net/api/v5";
const APP_USER_AGENT: &str = "Google/Pixel_4a Android/12 Bookmate/6.66.0";
const MAX_ATTEMPTS: usize = 3;

#[derive(Debug, Error)]
pub enum Error {
    #[error("provide a Yandex Books URL or UUID")]
    InvalidBook,
    #[error("the token is invalid; get a new Yandex Books token")]
    InvalidToken,
    #[error("the book is unavailable for reading with this account")]
    Unavailable,
    #[error("Yandex Books returned error {0}")]
    Http(u16),
    #[error("could not connect to Yandex Books: {0}")]
    Transport(String),
    #[error("Yandex Books returned an invalid response: {0}")]
    InvalidResponse(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SeriesInfo {
    pub title: String,
    pub position_label: String,
    pub uuid: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct BookMetadata {
    pub uuid: String,
    pub title: String,
    pub authors: Vec<String>,
    pub annotation: String,
    pub genres: Vec<String>,
    pub publishers: Vec<String>,
    pub series: Vec<SeriesInfo>,
    pub cover_url: Option<String>,
    pub source: String,
    pub original_year: Option<String>,
    pub rights: Option<String>,
}

#[derive(Debug)]
pub struct DownloadedBook {
    pub metadata: BookMetadata,
    pub epub: Vec<u8>,
    pub cover: Option<Vec<u8>>,
}

pub struct Client {
    agent: ureq::Agent,
    token: String,
}

impl Client {
    pub fn new(token: impl Into<String>) -> Result<Self> {
        let agent_builder = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(30))
            .timeout_read(Duration::from_secs(120))
            .timeout_write(Duration::from_secs(120))
            .redirects(10);
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let agent_builder = agent_builder.tls_connector(std::sync::Arc::new(
            ureq::native_tls::TlsConnector::new().map_err(|error| {
                Error::Transport(format!("could not initialize TLS: {error}"))
            })?,
        ));
        let agent = agent_builder.build();
        Ok(Self { agent, token: token.into() })
    }

    pub fn download_book(
        &self,
        uuid: &str,
        mut report: impl FnMut(&str),
    ) -> Result<DownloadedBook> {
        report("Fetching book details…");
        let data = self.request_json(&format!("{BASE_URL}/books/{uuid}"))?;
        let root = object(&data);
        let book_value = root.get("book").unwrap_or(&data);
        let book = object(book_value);

        if book.get("can_be_read").and_then(Value::as_bool) == Some(false) {
            return Err(Error::Unavailable);
        }

        let metadata = extract_metadata(root, book, uuid);
        report("Downloading EPUB…");
        let epub = self
            .request_bytes(&format!("{BASE_URL}/books/{uuid}/content/v4"), true)?;
        if !epub.starts_with(b"PK") {
            return Err(Error::InvalidResponse(
                "the server returned a file that is not an EPUB".into(),
            ));
        }

        let cover = metadata
            .cover_url
            .as_deref()
            .and_then(|url| self.request_bytes(url, false).ok());

        Ok(DownloadedBook { metadata, epub, cover })
    }

    fn request_json(&self, url: &str) -> Result<Value> {
        self.request(url, true)?
            .into_json()
            .map_err(|error| Error::InvalidResponse(error.to_string()))
    }

    fn request_bytes(&self, url: &str, authenticated: bool) -> Result<Vec<u8>> {
        let mut content = Vec::new();
        self.request(url, authenticated)?
            .into_reader()
            .read_to_end(&mut content)
            .map_err(|error| Error::Transport(error.to_string()))?;
        Ok(content)
    }

    fn request(&self, url: &str, authenticated: bool) -> Result<ureq::Response> {
        let mut last_error = String::from("unknown error");

        for attempt in 0..MAX_ATTEMPTS {
            let mut request = self.agent.get(url);
            if authenticated {
                request = request
                    .set("app-user-agent", APP_USER_AGENT)
                    .set("auth-token", &self.token);
            }

            match request.call() {
                Ok(response) => return Ok(response),
                Err(ureq::Error::Status(401, _)) => return Err(Error::InvalidToken),
                Err(ureq::Error::Status(status, _))
                    if status == 429 || status >= 500 =>
                {
                    last_error = if status == 429 {
                        "too many requests".into()
                    } else {
                        format!("server error {status}")
                    };
                }
                Err(ureq::Error::Status(status, _)) => {
                    return Err(Error::Http(status));
                }
                Err(ureq::Error::Transport(error)) => last_error = error.to_string(),
            }

            if attempt + 1 < MAX_ATTEMPTS {
                thread::sleep(Duration::from_secs(1 << attempt));
            }
        }

        Err(Error::Transport(last_error))
    }
}

pub fn parse_book_id(input: &str) -> Result<String> {
    let trimmed = input.trim();
    let end = trimmed.find(['?', '#']).unwrap_or(trimmed.len());
    let value = trimmed[..end].trim_end_matches('/');

    let without_scheme = strip_ascii_prefix(value, "https://")
        .or_else(|| strip_ascii_prefix(value, "http://"))
        .unwrap_or(value);
    if let Some(uuid) = strip_ascii_prefix(without_scheme, "books.yandex.ru/books/")
        && valid_uuid(uuid)
    {
        return Ok(uuid.to_owned());
    }
    if valid_uuid(value) {
        return Ok(value.to_owned());
    }
    Err(Error::InvalidBook)
}

pub fn build_output_path(
    output_directory: &Path,
    metadata: &BookMetadata,
) -> PathBuf {
    let source_author = metadata
        .authors
        .first()
        .map(String::as_str)
        .filter(|author| !author.is_empty())
        .unwrap_or("Unknown author");
    let author = sanitize_filename(&format_author_name(source_author));
    let title = sanitize_filename(if metadata.title.is_empty() {
        "Unknown title"
    } else {
        &metadata.title
    });

    output_directory.join(sanitize_filename(&format!("{author}. {title}.epub")))
}

pub fn sanitize_filename(value: &str) -> String {
    let replaced: String = value
        .chars()
        .map(|character| match character {
            '\\' | '/' | '|' => '-',
            ':' => ';',
            '*' => 'x',
            '?' => '!',
            '"' => '\'',
            '<' => '(',
            '>' => ')',
            character if character.is_control() => ' ',
            character => character,
        })
        .collect();
    let cleaned = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "Unknown".into()
    } else {
        cleaned
    }
}

pub fn format_author_name(name: &str) -> String {
    let parts: Vec<_> = name.split_whitespace().collect();
    match parts.as_slice() {
        [first, last] => format!("{last} {first}"),
        [first, middle, last] => format!("{last} {first} {middle}"),
        _ => name.to_owned(),
    }
}

fn extract_metadata(
    root: &Map<String, Value>,
    book: &Map<String, Value>,
    requested_uuid: &str,
) -> BookMetadata {
    let root_authors = extract_authors(root);
    let authors =
        if root_authors.is_empty() { extract_authors(book) } else { root_authors };
    let genres = extract_named(book.get("topics"))
        .into_iter()
        .filter(|genre| genre != "Виртуальный рассказчик")
        .collect();

    BookMetadata {
        uuid: string(book.get("uuid"))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| requested_uuid.to_owned()),
        title: string(book.get("title"))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Unknown title".into()),
        authors,
        annotation: string(book.get("annotation")).unwrap_or_default(),
        genres,
        publishers: extract_named(book.get("publishers")),
        series: extract_series(root, book),
        cover_url: extract_cover_url(root, book),
        source: format!("https://books.yandex.ru/books/{requested_uuid}"),
        original_year: scalar_string(book.get("original_year")),
        rights: string(book.get("owner_catalog_title"))
            .filter(|value| !value.is_empty()),
    }
}

fn extract_authors(data: &Map<String, Value>) -> Vec<String> {
    let mut authors = Vec::new();
    if let Some(values) = data.get("authors_objects").and_then(Value::as_array) {
        for value in values {
            let author = object(value);
            push_unique(
                &mut authors,
                string(author.get("name")).or_else(|| string(author.get("title"))),
            );
        }
    }

    match data.get("authors") {
        Some(Value::String(value)) => {
            for author in value.split(',') {
                push_unique(&mut authors, Some(author.trim().to_owned()));
            }
        }
        Some(Value::Array(values)) => {
            for value in values {
                let author = match value {
                    Value::String(value) => Some(value.clone()),
                    Value::Object(value) => string(value.get("name")),
                    _ => None,
                };
                push_unique(&mut authors, author);
            }
        }
        _ => {}
    }
    authors
}

fn extract_named(value: Option<&Value>) -> Vec<String> {
    let Some(values) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for value in values {
        let name = match value {
            Value::String(value) => Some(value.clone()),
            Value::Object(value) => {
                string(value.get("title")).or_else(|| string(value.get("name")))
            }
            _ => None,
        };
        push_unique(&mut result, name);
    }
    result
}

fn extract_series(
    root: &Map<String, Value>,
    book: &Map<String, Value>,
) -> Vec<SeriesInfo> {
    let values = book
        .get("series_list")
        .and_then(Value::as_array)
        .or_else(|| root.get("series_list").and_then(Value::as_array));
    values
        .into_iter()
        .flatten()
        .map(object)
        .map(|series| SeriesInfo {
            title: string(series.get("title")).unwrap_or_default(),
            position_label: scalar_string(series.get("position_label"))
                .or_else(|| scalar_string(series.get("position")))
                .unwrap_or_default(),
            uuid: string(series.get("uuid")).unwrap_or_default(),
        })
        .collect()
}

fn extract_cover_url(
    root: &Map<String, Value>,
    book: &Map<String, Value>,
) -> Option<String> {
    let cover = root
        .get("cover")
        .map(object)
        .filter(|cover| !cover.is_empty())
        .unwrap_or_else(|| object(book.get("cover").unwrap_or(&Value::Null)));
    ["large", "medium", "small"]
        .into_iter()
        .find_map(|key| string(cover.get(key)).filter(|value| !value.is_empty()))
}

fn object(value: &Value) -> &Map<String, Value> {
    static EMPTY: std::sync::LazyLock<Map<String, Value>> =
        std::sync::LazyLock::new(Map::new);
    value.as_object().unwrap_or(&EMPTY)
}

fn string(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_owned)
}

fn scalar_string(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(value)) => Some(value.clone()),
        Some(Value::Number(value)) => Some(value.to_string()),
        _ => None,
    }
}

fn push_unique(values: &mut Vec<String>, value: Option<String>) {
    if let Some(value) = value.filter(|value| !value.is_empty())
        && !values.contains(&value)
    {
        values.push(value);
    }
}

fn strip_ascii_prefix<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    value
        .get(..prefix.len())
        .filter(|candidate| candidate.eq_ignore_ascii_case(prefix))
        .map(|_| &value[prefix.len()..])
}

fn valid_uuid(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn client_has_an_https_backend() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || drop(listener.accept().unwrap()));

        let error = Client::new("")
            .unwrap()
            .agent
            .get(&format!("https://{address}"))
            .call()
            .unwrap_err();
        server.join().unwrap();

        assert_ne!(error.kind(), ureq::ErrorKind::UnknownScheme);
    }

    #[test]
    fn parses_book_urls_and_raw_ids() {
        assert_eq!(
            parse_book_id("https://books.yandex.ru/books/AbCd_123?from=search")
                .unwrap(),
            "AbCd_123"
        );
        assert_eq!(parse_book_id("AbCd-123").unwrap(), "AbCd-123");
        assert!(parse_book_id("https://books.yandex.ru/series/AbCd_123").is_err());
        assert!(
            parse_book_id("https://evilbooks.yandex.ru/books/AbCd_123").is_err()
        );
    }

    #[test]
    fn extracts_metadata_from_api_shape() {
        let data: Value = serde_json::from_str(
            r#"{
                "authors_objects": [{"name": "Тест Автор"}],
                "cover": {"large": "https://example.test/cover.jpg"},
                "book": {
                    "uuid": "test-book",
                    "title": "Тестовая книга",
                    "annotation": "Описание",
                    "topics": [{"title": "Тест"}, {"title": "Виртуальный рассказчик"}],
                    "publishers": [{"name": "Издательство"}],
                    "series_list": [{"title": "Цикл", "position": 2}],
                    "original_year": 2024,
                    "owner_catalog_title": "Правообладатель"
                }
            }"#,
        )
        .unwrap();
        let metadata =
            extract_metadata(object(&data), object(&data["book"]), "fallback");

        assert_eq!(metadata.uuid, "test-book");
        assert_eq!(metadata.authors, ["Тест Автор"]);
        assert_eq!(metadata.genres, ["Тест"]);
        assert_eq!(metadata.series[0].position_label, "2");
        assert_eq!(metadata.original_year.as_deref(), Some("2024"));
    }

    #[test]
    fn places_epub_directly_in_output_directory() {
        let metadata = BookMetadata {
            title: "Тестовая книга".into(),
            authors: vec!["Тест Автор".into()],
            series: vec![SeriesInfo {
                title: "Цикл".into(),
                ..SeriesInfo::default()
            }],
            ..BookMetadata::default()
        };
        assert_eq!(
            build_output_path(Path::new("/tmp/books"), &metadata),
            Path::new("/tmp/books/Автор Тест. Тестовая книга.epub")
        );
    }

    #[test]
    fn sanitizes_unsafe_filename_characters() {
        assert_eq!(sanitize_filename("  A/B: C?  "), "A-B; C!");
        assert_eq!(sanitize_filename(".."), "Unknown");
    }
}
