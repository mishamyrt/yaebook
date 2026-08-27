//! EPUB preparation for downloaded Yandex Books titles.

mod archive;
mod opf;
mod paths;
mod xhtml;

use thiserror::Error;
use yaebook_core::BookMetadata;

use archive::Archive;

#[derive(Debug, Error)]
pub enum Error {
    #[error("the downloaded file is not a valid EPUB archive: {0}")]
    InvalidArchive(String),
    #[error("the EPUB does not contain an OPF file")]
    MissingOpf,
    #[error("the OPF does not contain package, metadata, or manifest")]
    MissingPackage,
    #[error("could not convert the chapter to XHTML")]
    InvalidXhtml,
    #[error("could not generate the EPUB date: {0}")]
    Time(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
    #[error(transparent)]
    XmlParse(#[from] xmltree::ParseError),
    #[error(transparent)]
    XmlWrite(#[from] xmltree::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

pub fn prepare_epub(
    source: &[u8],
    metadata: &BookMetadata,
    cover: Option<&[u8]>,
    report: impl FnMut(&str),
) -> Result<Vec<u8>> {
    let mut archive = Archive::read(source)?;
    opf::prepare(&mut archive, metadata, cover, report)?;
    archive.finish()
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read, Write};

    use xmltree::{Element, XMLNode};
    use yaebook_core::BookMetadata;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipArchive, ZipWriter};

    use super::*;

    const PNG: &[u8] = &[137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 0];

    #[test]
    fn builds_styled_epub_without_embedded_fonts() {
        let result =
            prepare_epub(&fixture(false), &metadata(), Some(PNG), |_| {}).unwrap();

        assert_eq!(&result[..4], b"PK\x03\x04");
        assert_eq!(u16::from_le_bytes([result[8], result[9]]), 0);
        let name_length = u16::from_le_bytes([result[26], result[27]]) as usize;
        assert_eq!(&result[30..30 + name_length], b"mimetype");

        let mut archive = ZipArchive::new(Cursor::new(result)).unwrap();
        assert_eq!(read(&mut archive, "mimetype"), b"application/epub+zip");
        assert!(archive.by_name("EPUB/Images/cover.png").is_ok());
        assert!(archive.by_name("EPUB/Text/cover.xhtml").is_ok());
        assert_eq!(
            archive.by_name("EPUB/Images/unchanged.jpg").unwrap().compression(),
            CompressionMethod::Stored
        );
        assert_eq!(
            read(&mut archive, "EPUB/Images/unchanged.jpg"),
            b"\xff\xd8\xffunchanged"
        );
        assert!((0..archive.len()).all(|index| {
            !archive.by_index(index).unwrap().name().starts_with("EPUB/Fonts/")
        }));

        let system_css =
            String::from_utf8(read(&mut archive, "EPUB/Styles/yaebook.css"))
                .unwrap();
        assert!(system_css.contains("font-family: serif"));
        let book_css =
            String::from_utf8(read(&mut archive, "EPUB/bookmate.css")).unwrap();
        assert!(!book_css.contains("@font-face"));
        assert!(
            book_css.contains("body { font-family: Fixture, serif; margin: 0; }")
        );

        let opf = read(&mut archive, "EPUB/content.opf");
        let package = Element::parse(opf.as_slice()).unwrap();
        let metadata = child(&package, "metadata");
        assert_eq!(text(child(metadata, "title")), "Тестовая книга");
        assert_eq!(text(child(metadata, "creator")), "Тест Автор");
        let manifest = child(&package, "manifest");
        let cover_item = children(manifest, "item")
            .find(|item| has_property(item, "cover-image"))
            .unwrap();
        assert_eq!(&cover_item.attributes["href"], "Images/cover.png");
        assert!(children(manifest, "item").all(|item| {
            item.attributes.get("media-type").map(String::as_str)
                != Some("font/woff2")
        }));
        let cover_page_id = children(manifest, "item")
            .find(|item| {
                item.attributes.get("href").map(String::as_str)
                    == Some("Text/cover.xhtml")
            })
            .unwrap()
            .attributes["id"]
            .clone();
        assert_eq!(
            children(child(&package, "spine"), "itemref").next().unwrap().attributes
                ["idref"],
            cover_page_id
        );

        let chapter = read(&mut archive, "EPUB/chapter.html");
        let chapter = String::from_utf8(chapter).unwrap();
        assert!(chapter.contains("xmlns=\"http://www.w3.org/1999/xhtml\""));
        assert!(chapter.contains("href=\"Styles/yaebook.css\""));
        assert!(chapter.contains("<body><p>Текст</p></body>"));
        assert!(chapter.contains("<meta charset=\"UTF-8\"/>"));

        let empty = read(&mut archive, "EPUB/empty.xhtml");
        let empty = String::from_utf8(empty).unwrap();
        assert!(empty.contains("<head>"));
        assert!(empty.contains("<body>"));
    }

    #[test]
    fn supports_explicit_opf_namespace_prefix() {
        let result =
            prepare_epub(&fixture(true), &metadata(), None, |_| {}).unwrap();
        let mut archive = ZipArchive::new(Cursor::new(result)).unwrap();
        let opf = read(&mut archive, "EPUB/content.opf");
        let package = Element::parse(opf.as_slice()).unwrap();
        assert_eq!(package.prefix.as_deref(), Some("opf"));
        assert!(
            children(child(&package, "manifest"), "item").any(|item| item
                .attributes
                .get("href")
                .map(String::as_str)
                == Some("Styles/yaebook.css"))
        );
    }

    fn fixture(prefixed: bool) -> Vec<u8> {
        let prefix = if prefixed { "opf:" } else { "" };
        let namespace = if prefixed { "xmlns:opf" } else { "xmlns" };
        let opf = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<{prefix}package {namespace}="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
  <{prefix}metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="id">fixture</dc:identifier><dc:title>Fixture</dc:title>
  </{prefix}metadata>
  <{prefix}manifest>
    <{prefix}item id="chapter" href="chapter.html" media-type="application/xhtml+xml"/>
    <{prefix}item id="empty" href="empty.xhtml" media-type="application/xhtml+xml"/>
    <{prefix}item id="css" href="bookmate.css" media-type="text/css"/>
    <{prefix}item id="font" href="Fonts/fixture.woff2" media-type="font/woff2"/>
    <{prefix}item id="image" href="Images/unchanged.jpg" media-type="image/jpeg"/>
  </{prefix}manifest>
  <{prefix}spine><{prefix}itemref idref="chapter"/></{prefix}spine>
</{prefix}package>"#
        );
        let container = br#"<?xml version="1.0" encoding="UTF-8"?>
<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0">
  <rootfiles><rootfile full-path="EPUB/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;
        let chapter = r#"<!DOCTYPE html><html><head><meta charset="UTF-8"><title></title>
<link href="bookmate.css" rel="stylesheet" type="text/css">
<body><p>Текст</p></body></html>"#;

        let output = Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(output);
        writer
            .start_file(
                "mimetype",
                SimpleFileOptions::default()
                    .compression_method(CompressionMethod::Stored),
            )
            .unwrap();
        writer.write_all(b"application/epub+zip").unwrap();
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated);
        for (name, content) in [
            ("META-INF/container.xml", container.as_slice()),
            ("EPUB/content.opf", opf.as_bytes()),
            ("EPUB/chapter.html", chapter.as_bytes()),
            ("EPUB/empty.xhtml", b"\n".as_slice()),
            (
                "EPUB/bookmate.css",
                b"@font-face { font-family: Fixture; src: url('Fonts/fixture.woff2'); }\nbody { font-family: Fixture, serif; margin: 0; }".as_slice(),
            ),
            ("EPUB/Fonts/fixture.woff2", b"fixture-font".as_slice()),
        ] {
            writer.start_file(name, options).unwrap();
            writer.write_all(content).unwrap();
        }
        writer
            .start_file(
                "EPUB/Images/unchanged.jpg",
                SimpleFileOptions::default()
                    .compression_method(CompressionMethod::Stored),
            )
            .unwrap();
        writer.write_all(b"\xff\xd8\xffunchanged").unwrap();
        writer.finish().unwrap().into_inner()
    }

    fn metadata() -> BookMetadata {
        BookMetadata {
            uuid: "test-book".into(),
            title: "Тестовая книга".into(),
            authors: vec!["Тест Автор".into()],
            annotation: "Описание".into(),
            genres: vec!["Тест".into()],
            publishers: vec!["Издательство".into()],
            source: "https://books.yandex.ru/books/test-book".into(),
            ..BookMetadata::default()
        }
    }

    fn read(archive: &mut ZipArchive<Cursor<Vec<u8>>>, name: &str) -> Vec<u8> {
        let mut file = archive.by_name(name).unwrap();
        let mut content = Vec::new();
        file.read_to_end(&mut content).unwrap();
        content
    }

    fn child<'a>(parent: &'a Element, name: &'a str) -> &'a Element {
        children(parent, name).next().unwrap()
    }

    fn children<'a>(
        parent: &'a Element,
        name: &'a str,
    ) -> impl Iterator<Item = &'a Element> {
        parent.children.iter().filter_map(move |node| match node {
            XMLNode::Element(element) if element.name == name => Some(element),
            _ => None,
        })
    }

    fn text(element: &Element) -> String {
        element
            .children
            .iter()
            .filter_map(|node| match node {
                XMLNode::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    fn has_property(element: &Element, property: &str) -> bool {
        element.attributes.get("properties").is_some_and(|values| {
            values.split_whitespace().any(|value| value == property)
        })
    }
}
