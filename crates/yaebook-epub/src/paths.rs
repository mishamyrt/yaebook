pub(crate) fn manifest_path(opf_name: &str, href: &str) -> String {
    let href = percent_decode(href.split('#').next().unwrap_or(href));
    normalize(&format!("{}/{}", dirname(opf_name), href))
}

pub(crate) fn relative_path(from_directory: &str, to: &str) -> String {
    let from = components(from_directory);
    let to = components(to);
    let common =
        from.iter().zip(&to).take_while(|(left, right)| left == right).count();
    let mut result = vec![".."; from.len() - common];
    result.extend(to[common..].iter().copied());
    if result.is_empty() { ".".into() } else { result.join("/") }
}

pub(crate) fn dirname(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(directory, _)| directory)
}

fn normalize(path: &str) -> String {
    let mut clean = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                clean.pop();
            }
            part => clean.push(part),
        }
    }
    clean.join("/")
}

fn components(path: &str) -> Vec<&str> {
    path.split('/').filter(|part| !part.is_empty() && *part != ".").collect()
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex(bytes[index + 1]), hex(bytes[index + 2]))
        {
            decoded.push(high << 4 | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_manifest_paths_and_relatives() {
        assert_eq!(
            manifest_path("EPUB/content.opf", "Text/Chapter%201.xhtml#part"),
            "EPUB/Text/Chapter 1.xhtml"
        );
        assert_eq!(
            relative_path("EPUB/Text", "EPUB/Styles/system.css"),
            "../Styles/system.css"
        );
    }
}
