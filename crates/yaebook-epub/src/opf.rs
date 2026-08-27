use std::collections::HashSet;

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use xmltree::{Element, EmitterConfig, Namespace, XMLNode};

use crate::archive::Archive;
use crate::paths::{dirname, manifest_path, relative_path};
use crate::xhtml;
use crate::{Error, Result};
use yaebook_core::BookMetadata;

const DC_NAMESPACE: &str = "http://purl.org/dc/elements/1.1/";
const STYLE_HREF: &str = "Styles/yaebook.css";
const STYLESHEET: &str = r"html, body {
  font-family: serif;
}
h1, h2, h3, h4, h5, h6 {
  font-family: sans-serif;
}
code, pre, kbd, samp {
  font-family: monospace;
}
img, svg {
  max-width: 100%;
  height: auto;
}
";

pub(crate) fn prepare(
    archive: &mut Archive<'_>,
    values: &BookMetadata,
    cover: Option<&[u8]>,
    mut report: impl FnMut(&str),
) -> Result<()> {
    let opf_name = find_opf(archive)?;
    let source = archive.get(&opf_name)?.ok_or(Error::MissingOpf)?;
    let mut package = Element::parse(source.as_slice())?;
    if package.name != "package" || child_index(&package, "manifest").is_none() {
        return Err(Error::MissingPackage);
    }

    report("Updating metadata…");
    update_metadata(&mut package, values)?;
    report("Adding system styles…");
    add_stylesheet(archive, &opf_name, &mut package)?;
    report("Adding cover…");
    add_cover(archive, &opf_name, &mut package, cover)?;
    report("Building EPUB…");

    let mut serialized = Vec::new();
    package.write_with_config(
        &mut serialized,
        EmitterConfig::new().perform_indent(false).write_document_declaration(true),
    )?;
    archive.set(opf_name, serialized);
    Ok(())
}

fn find_opf(archive: &mut Archive<'_>) -> Result<String> {
    if let Some(container) = archive.get("META-INF/container.xml")?
        && let Ok(container) = Element::parse(container.as_slice())
        && let Some(rootfile) = find_descendant(&container, "rootfile")
        && let Some(path) = rootfile.attributes.get("full-path")
        && archive.contains(path)
    {
        return Ok(path.clone());
    }
    archive
        .names()
        .find(|name| name.to_ascii_lowercase().ends_with(".opf"))
        .map(str::to_owned)
        .ok_or(Error::MissingOpf)
}

fn update_metadata(package: &mut Element, values: &BookMetadata) -> Result<()> {
    let epub3 = package
        .attributes
        .get("version")
        .is_none_or(|version| version.starts_with('3'));
    let add_identifier = !values.uuid.is_empty()
        && child(package, "metadata")
            .is_none_or(|metadata| child_index(metadata, "identifier").is_none());
    if add_identifier {
        package.attributes.insert("unique-identifier".into(), "bookid".into());
    }
    let metadata = ensure_child(package, "metadata", true);
    let namespaces = metadata.namespaces.get_or_insert_with(Namespace::empty);
    namespaces.force_put("dc", DC_NAMESPACE);

    replace_dc(metadata, "title", nonempty([values.title.as_str()]));
    replace_dc(
        metadata,
        "creator",
        nonempty(values.authors.iter().map(String::as_str)),
    );
    replace_dc(metadata, "description", nonempty([values.annotation.as_str()]));
    replace_dc(
        metadata,
        "publisher",
        nonempty(values.publishers.iter().map(String::as_str)),
    );
    replace_dc(
        metadata,
        "subject",
        nonempty(
            values
                .genres
                .iter()
                .map(String::as_str)
                .filter(|genre| *genre != "Виртуальный рассказчик"),
        ),
    );
    replace_dc(metadata, "source", nonempty([values.source.as_str()]));
    replace_dc(
        metadata,
        "date",
        nonempty(values.original_year.iter().map(String::as_str)),
    );
    replace_dc(
        metadata,
        "rights",
        nonempty(values.rights.iter().map(String::as_str)),
    );
    let relations = values
        .series
        .iter()
        .filter(|series| !series.title.is_empty())
        .map(|series| {
            if series.position_label.is_empty() {
                format!("Series: {}", series.title)
            } else {
                format!(
                    "Series: {}, Number: {}",
                    series.title, series.position_label
                )
            }
        })
        .collect::<Vec<_>>();
    replace_dc(metadata, "relation", relations);

    if add_identifier {
        let mut identifier = dc_element("identifier", metadata);
        identifier.attributes.insert("id".into(), "bookid".into());
        identifier.children.push(XMLNode::Text(format!("urn:uuid:{}", values.uuid)));
        metadata.children.push(XMLNode::Element(identifier));
    }

    if epub3 {
        let modified = OffsetDateTime::now_utc()
            .replace_nanosecond(0)
            .map_err(|error| Error::Time(error.to_string()))?
            .format(&Rfc3339)
            .map_err(|error| Error::Time(error.to_string()))?;
        if let Some(index) = metadata.children.iter().position(|node| {
            element(node).is_some_and(|element| {
                element.name == "meta"
                    && element.attributes.get("property").map(String::as_str)
                        == Some("dcterms:modified")
            })
        }) {
            let element =
                element_mut(&mut metadata.children[index]).expect("element index");
            element.children = vec![XMLNode::Text(modified)];
        } else {
            let mut element = opf_element(metadata, "meta");
            element.attributes.insert("property".into(), "dcterms:modified".into());
            element.children.push(XMLNode::Text(modified));
            metadata.children.push(XMLNode::Element(element));
        }
    }
    Ok(())
}

fn add_stylesheet(
    archive: &mut Archive<'_>,
    opf_name: &str,
    package: &mut Element,
) -> Result<()> {
    let style_path = manifest_path(opf_name, STYLE_HREF);
    let (xhtml_hrefs, css_hrefs, font_hrefs) = {
        let manifest =
            child_mut(package, "manifest").ok_or(Error::MissingPackage)?;
        let font_hrefs = direct_elements(manifest, "item")
            .filter(|item| is_font_item(item))
            .filter_map(|item| item.attributes.get("href").cloned())
            .collect::<Vec<_>>();
        let css_hrefs = direct_elements(manifest, "item")
            .filter(|item| {
                item.attributes.get("media-type").map(String::as_str)
                    == Some("text/css")
            })
            .filter_map(|item| item.attributes.get("href").cloned())
            .collect::<Vec<_>>();
        manifest.children.retain(|node| {
            !element(node)
                .is_some_and(|item| item.name == "item" && is_font_item(item))
        });
        ensure_manifest_item(
            manifest,
            "yaebook-system-style",
            STYLE_HREF,
            "text/css",
        );
        let xhtml_hrefs = direct_elements(manifest, "item")
            .filter(|item| {
                item.attributes.get("media-type").map(String::as_str)
                    == Some("application/xhtml+xml")
            })
            .filter_map(|item| item.attributes.get("href").cloned())
            .collect::<Vec<_>>();
        (xhtml_hrefs, css_hrefs, font_hrefs)
    };

    for href in font_hrefs {
        archive.remove(&manifest_path(opf_name, &href));
    }
    for href in css_hrefs {
        let path = manifest_path(opf_name, &href);
        if let Some(content) = archive.get(&path)? {
            let without_fonts = strip_font_face_rules(&content);
            if without_fonts != content {
                archive.set(path, without_fonts);
            }
        }
    }
    archive.set(&style_path, STYLESHEET.as_bytes().to_vec());

    for href in xhtml_hrefs {
        let path = manifest_path(opf_name, &href);
        if let Some(content) = archive.get(&path)? {
            let normalized = xhtml::normalize(&content, &path, &style_path)?;
            archive.set(path, normalized);
        }
    }
    Ok(())
}

fn is_font_item(item: &Element) -> bool {
    let media_type =
        item.attributes.get("media-type").map(String::as_str).unwrap_or_default();
    if media_type.starts_with("font/")
        || matches!(
            media_type,
            "application/font-sfnt"
                | "application/font-woff"
                | "application/vnd.ms-fontobject"
                | "application/vnd.ms-opentype"
                | "application/x-font-opentype"
                | "application/x-font-ttf"
                | "application/x-font-woff"
        )
    {
        return true;
    }

    item.attributes.get("href").is_some_and(|href| {
        let href = href.split(['?', '#']).next().unwrap_or(href);
        let extension = href.rsplit_once('.').map(|(_, extension)| extension);
        extension.is_some_and(|extension| {
            ["eot", "otf", "ttf", "woff", "woff2"]
                .iter()
                .any(|font| extension.eq_ignore_ascii_case(font))
        })
    })
}

fn strip_font_face_rules(content: &[u8]) -> Vec<u8> {
    const FONT_FACE: &[u8] = b"@font-face";

    let mut output = Vec::with_capacity(content.len());
    let mut copied_until = 0;
    let mut search_from = 0;
    while let Some(start) = next_font_face(content, search_from) {
        let Some(end) = css_block_end(content, start + FONT_FACE.len()) else {
            search_from = start + FONT_FACE.len();
            continue;
        };
        output.extend_from_slice(&content[copied_until..start]);
        copied_until = end;
        search_from = end;
    }
    output.extend_from_slice(&content[copied_until..]);
    output
}

fn next_font_face(content: &[u8], mut index: usize) -> Option<usize> {
    const FONT_FACE: &[u8] = b"@font-face";

    while index < content.len() {
        if content[index..].starts_with(b"/*") {
            index = skip_css_comment(content, index + 2);
        } else if matches!(content[index], b'\'' | b'"') {
            index = skip_css_string(content, index + 1, content[index]);
        } else if content[index] == b'@'
            && content
                .get(index..index + FONT_FACE.len())
                .is_some_and(|value| value.eq_ignore_ascii_case(FONT_FACE))
            && content.get(index + FONT_FACE.len()).is_none_or(|value| {
                !value.is_ascii_alphanumeric() && !matches!(value, b'-' | b'_')
            })
        {
            return Some(index);
        } else {
            index += 1;
        }
    }
    None
}

fn css_block_end(content: &[u8], mut index: usize) -> Option<usize> {
    while index < content.len() {
        if content[index..].starts_with(b"/*") {
            index = skip_css_comment(content, index + 2);
        } else if content[index].is_ascii_whitespace() {
            index += 1;
        } else {
            break;
        }
    }
    if content.get(index) != Some(&b'{') {
        return None;
    }

    let mut depth = 1;
    index += 1;
    while index < content.len() {
        if content[index..].starts_with(b"/*") {
            index = skip_css_comment(content, index + 2);
        } else if matches!(content[index], b'\'' | b'"') {
            index = skip_css_string(content, index + 1, content[index]);
        } else {
            match content[index] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(index + 1);
                    }
                }
                _ => {}
            }
            index += 1;
        }
    }
    None
}

fn skip_css_comment(content: &[u8], mut index: usize) -> usize {
    while index + 1 < content.len() {
        if content[index..].starts_with(b"*/") {
            return index + 2;
        }
        index += 1;
    }
    content.len()
}

fn skip_css_string(content: &[u8], mut index: usize, quote: u8) -> usize {
    while index < content.len() {
        if content[index] == b'\\' {
            index = (index + 2).min(content.len());
        } else if content[index] == quote {
            return index + 1;
        } else {
            index += 1;
        }
    }
    content.len()
}

fn add_cover(
    archive: &mut Archive<'_>,
    opf_name: &str,
    package: &mut Element,
    cover: Option<&[u8]>,
) -> Result<()> {
    let metadata = child(package, "metadata").ok_or(Error::MissingPackage)?;
    let legacy_ids: HashSet<_> = direct_elements(metadata, "meta")
        .filter(|meta| {
            meta.attributes.get("name").map(String::as_str) == Some("cover")
        })
        .filter_map(|meta| meta.attributes.get("content").cloned())
        .collect();
    let manifest = child(package, "manifest").ok_or(Error::MissingPackage)?;
    let mut cover_index = find_cover_item(manifest, &legacy_ids);
    let image = cover.and_then(detect_image);

    let cover_href = if let (Some(content), Some(image)) = (cover, image) {
        let href = format!("Images/cover.{}", image.extension);
        archive.set(manifest_path(opf_name, &href), content.to_vec());
        let manifest =
            child_mut(package, "manifest").ok_or(Error::MissingPackage)?;
        if let Some(index) = cover_index {
            let item =
                element_mut(&mut manifest.children[index]).expect("cover item");
            item.attributes.insert("href".into(), href.clone());
            item.attributes.insert("media-type".into(), image.media_type.into());
        } else {
            ensure_manifest_item(
                manifest,
                "yaebook-cover-image",
                &href,
                image.media_type,
            );
            cover_index = find_item_by_href(manifest, &href);
        }
        href
    } else if let Some(index) = cover_index {
        let manifest = child(package, "manifest").ok_or(Error::MissingPackage)?;
        let item = element(&manifest.children[index]).expect("cover item");
        let Some(href) = item.attributes.get("href").cloned() else {
            return Ok(());
        };
        if !archive.contains(&manifest_path(opf_name, &href)) {
            return Ok(());
        }
        href
    } else {
        return Ok(());
    };

    let epub3 = package
        .attributes
        .get("version")
        .is_none_or(|version| version.starts_with('3'));
    let (cover_id, cover_page_id) = {
        let manifest =
            child_mut(package, "manifest").ok_or(Error::MissingPackage)?;
        let cover_index = cover_index.expect("cover item must exist");
        for item in direct_elements_mut(manifest, "item") {
            remove_property(item, "cover-image");
        }
        let needs_id = !element(&manifest.children[cover_index])
            .expect("cover item")
            .attributes
            .contains_key("id");
        if needs_id {
            let id = unique_id(manifest, "yaebook-cover-image");
            element_mut(&mut manifest.children[cover_index])
                .expect("cover item")
                .attributes
                .insert("id".into(), id);
        }
        let cover_item =
            element_mut(&mut manifest.children[cover_index]).expect("cover item");
        if epub3 {
            add_property(cover_item, "cover-image");
        }
        let cover_id = cover_item.attributes["id"].clone();
        let cover_page = ensure_manifest_item(
            manifest,
            "yaebook-cover-page",
            "Text/cover.xhtml",
            "application/xhtml+xml",
        );
        (cover_id, cover_page.attributes["id"].clone())
    };

    let metadata = child_mut(package, "metadata").ok_or(Error::MissingPackage)?;
    metadata.children.retain(|node| {
        !element(node).is_some_and(|element| {
            element.name == "meta"
                && element.attributes.get("name").map(String::as_str)
                    == Some("cover")
        })
    });
    let mut legacy_cover = opf_element(metadata, "meta");
    legacy_cover.attributes.insert("name".into(), "cover".into());
    legacy_cover.attributes.insert("content".into(), cover_id);
    metadata.children.push(XMLNode::Element(legacy_cover));

    let cover_page_path = manifest_path(opf_name, "Text/cover.xhtml");
    let image_href = relative_path(
        dirname(&cover_page_path),
        &manifest_path(opf_name, &cover_href),
    );
    archive.set(cover_page_path, cover_page(&image_href).into_bytes());

    let spine = ensure_child(package, "spine", false);
    spine.children.retain(|node| {
        !element(node).is_some_and(|element| {
            element.name == "itemref"
                && element.attributes.get("idref").map(String::as_str)
                    == Some(cover_page_id.as_str())
        })
    });
    let mut reference = opf_element(spine, "itemref");
    reference.attributes.insert("idref".into(), cover_page_id);
    spine.children.insert(0, XMLNode::Element(reference));

    let guide = ensure_child(package, "guide", false);
    guide.children.retain(|node| {
        !element(node).is_some_and(|element| {
            element.name == "reference"
                && element.attributes.get("type").map(String::as_str)
                    == Some("cover")
        })
    });
    let mut reference = opf_element(guide, "reference");
    reference.attributes.insert("type".into(), "cover".into());
    reference.attributes.insert("title".into(), "Cover".into());
    reference.attributes.insert("href".into(), "Text/cover.xhtml".into());
    guide.children.push(XMLNode::Element(reference));
    Ok(())
}

fn find_cover_item(
    manifest: &Element,
    legacy_ids: &HashSet<String>,
) -> Option<usize> {
    manifest
        .children
        .iter()
        .enumerate()
        .filter_map(|(index, node)| element(node).map(|element| (index, element)))
        .filter(|(_, item)| item.name == "item")
        .find(|(_, item)| has_property(item, "cover-image"))
        .or_else(|| {
            manifest
                .children
                .iter()
                .enumerate()
                .filter_map(|(index, node)| {
                    element(node).map(|element| (index, element))
                })
                .filter(|(_, item)| item.name == "item")
                .find(|(_, item)| {
                    item.attributes
                        .get("id")
                        .is_some_and(|id| legacy_ids.contains(id))
                })
        })
        .or_else(|| {
            manifest
                .children
                .iter()
                .enumerate()
                .filter_map(|(index, node)| {
                    element(node).map(|element| (index, element))
                })
                .filter(|(_, item)| item.name == "item")
                .find(|(_, item)| {
                    let href = item
                        .attributes
                        .get("href")
                        .map(String::as_str)
                        .unwrap_or("");
                    let basename =
                        href.rsplit('/').next().unwrap_or(href).to_ascii_lowercase();
                    basename.starts_with("cover.")
                        && item.attributes.get("media-type").is_some_and(
                            |media_type| media_type.starts_with("image/"),
                        )
                })
        })
        .map(|(index, _)| index)
}

fn detect_image(content: &[u8]) -> Option<ImageInfo> {
    if content.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(ImageInfo { extension: "jpg", media_type: "image/jpeg" })
    } else if content.starts_with(&[137, 80, 78, 71, 13, 10, 26, 10]) {
        Some(ImageInfo { extension: "png", media_type: "image/png" })
    } else if content.starts_with(b"GIF87a") || content.starts_with(b"GIF89a") {
        Some(ImageInfo { extension: "gif", media_type: "image/gif" })
    } else {
        None
    }
}

struct ImageInfo {
    extension: &'static str,
    media_type: &'static str,
}

fn cover_page(image_href: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops" lang="ru" xml:lang="ru">
  <head>
    <title>Cover</title>
    <style type="text/css">body{{margin:0;padding:0;text-align:center}}img{{display:block;width:auto;height:auto;max-width:100%;max-height:100%;margin:auto}}</style>
  </head>
  <body><div><img src="{}" alt="Cover"/></div></body>
</html>"#,
        escape_attribute(image_href)
    )
}

fn escape_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn replace_dc(metadata: &mut Element, name: &str, values: Vec<String>) {
    if values.is_empty() {
        return;
    }
    metadata
        .children
        .retain(|node| element(node).is_none_or(|element| element.name != name));
    for value in values {
        let mut element = dc_element(name, metadata);
        element.children.push(XMLNode::Text(value));
        metadata.children.push(XMLNode::Element(element));
    }
}

fn nonempty<'a>(values: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    values.into_iter().filter(|value| !value.is_empty()).map(str::to_owned).collect()
}

fn ensure_manifest_item<'a>(
    manifest: &'a mut Element,
    requested_id: &str,
    href: &str,
    media_type: &str,
) -> &'a mut Element {
    let index = find_item_by_href(manifest, href).unwrap_or_else(|| {
        let id = unique_id(manifest, requested_id);
        let mut item = opf_element(manifest, "item");
        item.attributes.insert("id".into(), id);
        item.attributes.insert("href".into(), href.into());
        manifest.children.push(XMLNode::Element(item));
        manifest.children.len() - 1
    });
    let needs_id = !element(&manifest.children[index])
        .expect("manifest item")
        .attributes
        .contains_key("id");
    if needs_id {
        let id = unique_id(manifest, requested_id);
        element_mut(&mut manifest.children[index])
            .expect("manifest item")
            .attributes
            .insert("id".into(), id);
    }
    let item = element_mut(&mut manifest.children[index]).expect("manifest item");
    item.attributes.insert("media-type".into(), media_type.into());
    item
}

fn find_item_by_href(manifest: &Element, href: &str) -> Option<usize> {
    manifest.children.iter().position(|node| {
        element(node).is_some_and(|item| {
            item.name == "item"
                && item.attributes.get("href").map(String::as_str) == Some(href)
        })
    })
}

fn unique_id(manifest: &Element, requested: &str) -> String {
    let used: HashSet<_> = direct_elements(manifest, "item")
        .filter_map(|item| item.attributes.get("id").map(String::as_str))
        .collect();
    if !used.contains(requested) {
        return requested.into();
    }
    let mut suffix = 2;
    loop {
        let candidate = format!("{requested}-{suffix}");
        if !used.contains(candidate.as_str()) {
            return candidate;
        }
        suffix += 1;
    }
}

fn add_property(item: &mut Element, property: &str) {
    let mut properties = properties(item);
    if !properties.iter().any(|value| value == property) {
        properties.push(property.into());
    }
    item.attributes.insert("properties".into(), properties.join(" "));
}

fn remove_property(item: &mut Element, property: &str) {
    let properties = properties(item)
        .into_iter()
        .filter(|value| value != property)
        .collect::<Vec<_>>();
    if properties.is_empty() {
        item.attributes.remove("properties");
    } else {
        item.attributes.insert("properties".into(), properties.join(" "));
    }
}

fn has_property(item: &Element, property: &str) -> bool {
    properties(item).iter().any(|value| value == property)
}

fn properties(item: &Element) -> Vec<String> {
    item.attributes
        .get("properties")
        .map(|values| values.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default()
}

fn ensure_child<'a>(
    parent: &'a mut Element,
    name: &str,
    prepend: bool,
) -> &'a mut Element {
    let index = child_index(parent, name).unwrap_or_else(|| {
        let element = XMLNode::Element(opf_element(parent, name));
        if prepend {
            parent.children.insert(0, element);
            0
        } else {
            parent.children.push(element);
            parent.children.len() - 1
        }
    });
    element_mut(&mut parent.children[index]).expect("child element")
}

fn opf_element(parent: &Element, name: &str) -> Element {
    let mut element = Element::new(name);
    element.prefix.clone_from(&parent.prefix);
    element.namespace.clone_from(&parent.namespace);
    element
}

fn dc_element(name: &str, metadata: &Element) -> Element {
    let mut element = Element::new(name);
    element.prefix = Some("dc".into());
    element.namespace = Some(DC_NAMESPACE.into());
    element.namespaces.clone_from(&metadata.namespaces);
    element
}

fn child_index(parent: &Element, name: &str) -> Option<usize> {
    parent
        .children
        .iter()
        .position(|node| element(node).is_some_and(|element| element.name == name))
}

fn child<'a>(parent: &'a Element, name: &str) -> Option<&'a Element> {
    child_index(parent, name).and_then(|index| element(&parent.children[index]))
}

fn child_mut<'a>(parent: &'a mut Element, name: &str) -> Option<&'a mut Element> {
    let index = child_index(parent, name)?;
    element_mut(&mut parent.children[index])
}

fn direct_elements<'a>(
    parent: &'a Element,
    name: &'a str,
) -> impl Iterator<Item = &'a Element> {
    parent
        .children
        .iter()
        .filter_map(element)
        .filter(move |element| element.name == name)
}

fn direct_elements_mut<'a>(
    parent: &'a mut Element,
    name: &'a str,
) -> impl Iterator<Item = &'a mut Element> {
    parent
        .children
        .iter_mut()
        .filter_map(element_mut)
        .filter(move |element| element.name == name)
}

fn find_descendant<'a>(parent: &'a Element, name: &str) -> Option<&'a Element> {
    if parent.name == name {
        return Some(parent);
    }
    parent
        .children
        .iter()
        .filter_map(element)
        .find_map(|element| find_descendant(element, name))
}

fn element(node: &XMLNode) -> Option<&Element> {
    match node {
        XMLNode::Element(element) => Some(element),
        _ => None,
    }
}

fn element_mut(node: &mut XMLNode) -> Option<&mut Element> {
    match node {
        XMLNode::Element(element) => Some(element),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_font_face_rules_without_touching_comments_or_strings() {
        let source = br#"/* @font-face { keep comment } */
.label::after { content: "@font-face { keep string }"; }
@FONT-FACE /* remove rule */ { font-family: Fixture; src: url('fixture.woff2'); }
body { color: black; }"#;
        let result = String::from_utf8(strip_font_face_rules(source)).unwrap();

        assert!(result.contains("/* @font-face { keep comment } */"));
        assert!(result.contains("\"@font-face { keep string }\""));
        assert!(!result.contains("@FONT-FACE"));
        assert!(result.contains("body { color: black; }"));
    }
}
