use std::cell::RefCell;
use std::rc::Rc;

use html5ever::tendril::TendrilSink;
use html5ever::{
    Attribute, ParseOpts, QualName, local_name, namespace_url, ns, parse_document,
};
use markup5ever_rcdom::{Handle, Node, NodeData, RcDom};

use crate::paths::{dirname, manifest_path, relative_path};
use crate::{Error, Result};

const XHTML_NAMESPACE: &str = "http://www.w3.org/1999/xhtml";
const EPUB_NAMESPACE: &str = "http://www.idpf.org/2007/ops";
const SVG_NAMESPACE: &str = "http://www.w3.org/2000/svg";
const MATHML_NAMESPACE: &str = "http://www.w3.org/1998/Math/MathML";
const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";
const XLINK_NAMESPACE: &str = "http://www.w3.org/1999/xlink";

pub(crate) fn normalize(
    content: &[u8],
    document_path: &str,
    style_path: &str,
) -> Result<Vec<u8>> {
    let source = if content.iter().all(u8::is_ascii_whitespace) {
        "<html><head><title></title></head><body></body></html>".into()
    } else {
        String::from_utf8_lossy(content).into_owned()
    };
    let dom = parse_document(RcDom::default(), ParseOpts::default()).one(source);
    let html = find_element(&dom.document, "html").ok_or(Error::InvalidXhtml)?;
    let head = find_direct_child(&html, "head").ok_or(Error::InvalidXhtml)?;

    let document_directory = dirname(document_path);
    let has_stylesheet = head.children.borrow().iter().any(|child| {
        is_element(child, "link")
            && attribute(child, "href").is_some_and(|href| {
                manifest_path(&format!("{document_directory}/document.xhtml"), &href)
                    == style_path
            })
    });
    if !has_stylesheet {
        let href = relative_path(document_directory, style_path);
        let link = Node::new(NodeData::Element {
            name: QualName::new(None, ns!(html), local_name!("link")),
            attrs: RefCell::new(vec![
                html_attribute("rel", "stylesheet"),
                html_attribute("type", "text/css"),
                html_attribute("href", &href),
            ]),
            template_contents: RefCell::new(None),
            mathml_annotation_xml_integration_point: false,
        });
        link.parent.set(Some(Rc::downgrade(&head)));
        head.children.borrow_mut().push(link);
    }

    let mut output = String::from(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!DOCTYPE html>\n",
    );
    serialize(&html, &mut output, "", true);
    Ok(output.into_bytes())
}

fn html_attribute(name: &str, value: &str) -> Attribute {
    Attribute { name: QualName::new(None, ns!(), name.into()), value: value.into() }
}

fn find_element(handle: &Handle, name: &str) -> Option<Handle> {
    if is_element(handle, name) {
        return Some(handle.clone());
    }
    handle.children.borrow().iter().find_map(|child| find_element(child, name))
}

fn find_direct_child(handle: &Handle, name: &str) -> Option<Handle> {
    handle.children.borrow().iter().find(|child| is_element(child, name)).cloned()
}

fn is_element(handle: &Handle, expected: &str) -> bool {
    matches!(&handle.data, NodeData::Element { name, .. } if name.local.as_ref() == expected)
}

fn attribute(handle: &Handle, expected: &str) -> Option<String> {
    let NodeData::Element { attrs, .. } = &handle.data else {
        return None;
    };
    attrs
        .borrow()
        .iter()
        .find(|attribute| attribute.name.local.as_ref() == expected)
        .map(|attribute| attribute.value.to_string())
}

fn serialize(
    handle: &Handle,
    output: &mut String,
    parent_namespace: &str,
    root: bool,
) {
    match &handle.data {
        NodeData::Text { contents } => escape_text(&contents.borrow(), output),
        NodeData::Comment { contents } => {
            output.push_str("<!--");
            output.push_str(contents);
            output.push_str("-->");
        }
        NodeData::Element { name, attrs, template_contents, .. } => {
            let local = name.local.as_ref();
            let namespace = name.ns.as_ref();
            let qualified =
                qualified_name(name.prefix.as_ref().map(AsRef::as_ref), local);
            output.push('<');
            output.push_str(&qualified);

            if root {
                output.push_str(" xmlns=\"");
                output.push_str(XHTML_NAMESPACE);
                output.push_str("\" xmlns:epub=\"");
                output.push_str(EPUB_NAMESPACE);
                output.push_str("\" xmlns:xlink=\"");
                output.push_str(XLINK_NAMESPACE);
                output.push('"');
            } else if namespace != parent_namespace
                && namespace != XHTML_NAMESPACE
                && (namespace == SVG_NAMESPACE || namespace == MATHML_NAMESPACE)
            {
                output.push_str(" xmlns=\"");
                output.push_str(namespace);
                output.push('"');
            }

            for attribute in attrs.borrow().iter() {
                let local = attribute.name.local.as_ref();
                let namespace = attribute.name.ns.as_ref();
                if local == "xmlns" || local.starts_with("xmlns:") {
                    continue;
                }
                let prefix = attribute
                    .name
                    .prefix
                    .as_ref()
                    .map(AsRef::as_ref)
                    .or_else(|| (namespace == XML_NAMESPACE).then_some("xml"))
                    .or_else(|| (namespace == XLINK_NAMESPACE).then_some("xlink"));
                output.push(' ');
                output.push_str(&qualified_name(prefix, local));
                output.push_str("=\"");
                escape_attribute(&attribute.value, output);
                output.push('"');
            }

            if namespace == XHTML_NAMESPACE && is_void(local) {
                output.push_str("/>");
                return;
            }
            output.push('>');
            if let Some(contents) = template_contents.borrow().as_ref() {
                for child in contents.children.borrow().iter() {
                    serialize(child, output, namespace, false);
                }
            } else {
                for child in handle.children.borrow().iter() {
                    serialize(child, output, namespace, false);
                }
            }
            output.push_str("</");
            output.push_str(&qualified);
            output.push('>');
        }
        NodeData::Document => {
            for child in handle.children.borrow().iter() {
                serialize(child, output, parent_namespace, false);
            }
        }
        NodeData::Doctype { .. } | NodeData::ProcessingInstruction { .. } => {}
    }
}

fn qualified_name(prefix: Option<&str>, local: &str) -> String {
    prefix.map_or_else(|| local.to_owned(), |prefix| format!("{prefix}:{local}"))
}

fn is_void(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

fn escape_text(value: &str, output: &mut String) {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            character => output.push(character),
        }
    }
}

fn escape_attribute(value: &str, output: &mut String) {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '"' => output.push_str("&quot;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            character => output.push(character),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repairs_html_and_injects_relative_stylesheet() {
        let source = r#"<!DOCTYPE html><html><head><meta charset="UTF-8"><title></title>
            <body><p>До&nbsp;После</p></body></html>"#;
        let result = normalize(
            source.as_bytes(),
            "EPUB/Text/chapter.html",
            "EPUB/Styles/system.css",
        )
        .unwrap();
        let result = String::from_utf8(result).unwrap();

        assert!(result.contains("xmlns=\"http://www.w3.org/1999/xhtml\""));
        assert!(result.contains("href=\"../Styles/system.css\""));
        assert!(result.contains("До\u{a0}После"));
        assert!(result.contains("<meta charset=\"UTF-8\"/>"));
        assert!(result.contains("<body><p>"));
    }

    #[test]
    fn preserves_template_contents() {
        let source = r"<html><head><title></title></head><body>
            <template><p>Шаблон</p></template>
        </body></html>";
        let result = normalize(
            source.as_bytes(),
            "EPUB/Text/chapter.xhtml",
            "EPUB/Styles/system.css",
        )
        .unwrap();
        let result = String::from_utf8(result).unwrap();

        assert!(result.contains("<template><p>Шаблон</p></template>"));
    }
}
